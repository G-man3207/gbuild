//! Feedback manager for session-level feedback collection.
//!
//! This manager coordinates:
//! - Signal tracking via SessionSignalsHandle
//! - Heuristics evaluation to determine when to request feedback
//! - Local-only `/feedback` filing (session persistence + local telemetry
//!   events) — the xAI feedback/analytics upload path has been removed from
//!   this fork.
//!
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::session::feedback::{
    FeedbackEvaluation, FeedbackHeuristics, FeedbackRequest, FeedbackTier, TriggerCondition,
};
use crate::session::signals::{SessionSignalsActor, SessionSignalsHandle};

use prod_mc_cli_chat_proxy_types::feedback_types::{
    ClientType, FeedbackContent, FeedbackMode, FeedbackSubmission, FeedbackToolOutcome,
};

use crate::session::persistence::{LocalFeedbackEntry, PersistenceMsg, UserFeedbackEntry};

pub(crate) enum SubmitOutcome {
    /// Feedback filed locally (the only destination left in this fork).
    LocalOnly,
}

/// Shell-crate constructor: `with_content` + `shell_version`.
pub(crate) fn new_submission(
    session_id: String,
    client_type: ClientType,
    content: FeedbackContent,
) -> FeedbackSubmission {
    let mut s = FeedbackSubmission::with_content(session_id, client_type, content);
    s.shell_version = Some(gbuild_version::VERSION.to_string());
    s
}

#[derive(Debug)]
pub(crate) struct SubmitFeedbackOptions {
    pub solicited: bool,
    pub telemetry_enabled: bool,
    pub author_identity: Option<crate::util::user_identity::ResolvedUserIdentity>,
}

pub(crate) async fn submit_feedback_workflow(
    submission: &mut FeedbackSubmission,
    persistence_tx: Option<&tokio::sync::mpsc::UnboundedSender<PersistenceMsg>>,
    opts: SubmitFeedbackOptions,
) -> SubmitOutcome {
    let SubmitFeedbackOptions {
        solicited,
        telemetry_enabled,
        author_identity,
    } = opts;

    if let Some(user_meta) = crate::agent::mvp_agent::parse_json_object_env("GBUILD_USER_METADATA")
    {
        submission.merge_metadata(user_meta);
    }
    // Exhaustive destructure (no `..`) so a new field must be handled, not dropped.
    if let Some(crate::util::user_identity::ResolvedUserIdentity { name, email }) = author_identity
    {
        if let Some(name) = name {
            submission.author_name = Some(name);
        }
        if let Some(email) = email {
            submission.author_email = Some(email);
        }
    }

    if let Some(tx) = persistence_tx {
        let entry = LocalFeedbackEntry::UserFeedback(UserFeedbackEntry {
            submitted_at: chrono::Utc::now(),
            session_id: submission.session_id.clone(),
            turn_number: submission.turn_number,
            solicited,
            request_id: submission.request_id.clone(),
            dismissed: false,
            submission: Some(submission.clone()),
        });
        if tx.send(PersistenceMsg::Feedback(entry)).is_err() {
            tracing::warn!(
                session_id = %submission.session_id,
                "feedback persistence channel closed; entry dropped",
            );
        }
    }

    let telemetry_model_id = submission.model_id.clone();
    let telemetry_rating_value = submission.rating_value;
    let telemetry_session_id = submission.session_id.clone();
    let has_feedback_text = submission
        .feedback_text
        .as_ref()
        .is_some_and(|t| !t.is_empty());
    let request_id = submission.request_id.clone();
    let appearance_id = request_id.clone();

    let outcome = SubmitOutcome::LocalOnly;

    if telemetry_enabled {
        let feedback_span = tracing::info_span!(
            "feedback.survey",
            survey_type = "session",
            event_type = "responded",
            appearance_id = %appearance_id.as_deref().unwrap_or(""),
            has_feedback_text = has_feedback_text,
            rating = tracing::field::Empty,
            is_solicited = solicited,
        );
        // Record `rating` only for star ratings; text-only feedback has no
        // rating and must not export a fake 0.
        if let Some(rating) = telemetry_rating_value {
            feedback_span.record("rating", rating);
        }
        feedback_span.in_scope(|| {});
        gbuild_telemetry::session_ctx::log_event(gbuild_telemetry::events::UserFeedback {
            session_id: telemetry_session_id,
            has_feedback_text,
            model_id: telemetry_model_id,
            rating_value: telemetry_rating_value,
            is_solicited: solicited,
        });
    }

    outcome
}

/// Chat-state fields the session actor passes to [`FeedbackManager::submit_text_feedback`].
pub(crate) struct SessionFeedbackData {
    pub model_id: Option<String>,
    pub resolved_model_id: Option<String>,
    pub client_version: Option<String>,
    pub session_cwd: String,
}

/// Feedback feature flags threaded through session spawn.
#[derive(Debug, Clone, Default)]
pub struct FeedbackFlags {
    pub enabled: bool,
    pub user: Option<crate::agent::config::FeedbackUserConfig>,
}

/// Configuration for the feedback manager.
///
/// Two concerns gated by separate flags (`feedback_enabled`, `telemetry_enabled`).
/// Both default to `false`.
#[derive(Debug, Clone)]
pub struct FeedbackManagerConfig {
    /// Interval for syncing signals to the analytics backend (default: 30s)
    pub sync_interval: Duration,
    /// Whether user-facing feedback features are enabled (popups, `/feedback`,
    /// ratings). Gated by `GBUILD_FEEDBACK_ENABLED`.
    pub feedback_enabled: bool,
    /// Whether session analytics (signal sync, turn deltas) are enabled.
    /// Gated by `GBUILD_TELEMETRY_ENABLED`. These are analytics data that
    /// flow continuously without user action.
    pub telemetry_enabled: bool,
    /// Client type (Agent, Tui, Web, Extension)
    pub client_type: ClientType,
    /// Whether LOC attribution tracking is enabled for this session.
    /// Propagated into every `SessionTurnDelta` so the server can
    /// distinguish "tracking off" (zeros are noise) from "tracking on,
    /// no code changed" (zeros are real data).
    pub loc_tracking_enabled: bool,
    /// Timeout for draining the upload queue on shutdown (default: 30s).
    /// If uploads don't complete within this time, remaining items are abandoned.
    pub drain_timeout: Duration,
    pub user: Option<crate::agent::config::FeedbackUserConfig>,
}

impl Default for FeedbackManagerConfig {
    fn default() -> Self {
        Self {
            sync_interval: Duration::from_secs(60),
            feedback_enabled: false,
            telemetry_enabled: false,
            client_type: ClientType::Agent,
            loc_tracking_enabled: false,
            drain_timeout: Duration::from_secs(30),
            user: None,
        }
    }
}

/// Manages feedback collection for a single session.
pub struct FeedbackManager {
    /// Session ID
    session_id: String,
    /// Handle for sending signals (cheap to clone)
    signals_handle: SessionSignalsHandle,
    /// Feedback heuristics evaluator
    heuristics: Arc<RwLock<FeedbackHeuristics>>,
    /// Configuration
    config: FeedbackManagerConfig,
}

impl FeedbackManager {
    /// Create a new feedback manager for a session.
    ///
    /// If `feedback_client` is None, signal syncing is disabled but local
    /// tracking and heuristics evaluation still work.
    pub fn new(session_id: impl Into<String>, config: FeedbackManagerConfig) -> Self {
        let (signals_handle, actor) = SessionSignalsActor::with_sync_interval(config.sync_interval);

        // Spawn the signals actor
        tokio::spawn(actor.run());

        let session_id = session_id.into();
        tracing::info!(
            session_id = %session_id,
            feedback_enabled = config.feedback_enabled,
            telemetry_enabled = config.telemetry_enabled,
            "FeedbackManager initialized"
        );

        Self {
            session_id,
            signals_handle,
            heuristics: Arc::new(RwLock::new(FeedbackHeuristics::new())),
            config,
        }
    }

    /// Create a feedback manager for local tracking only.
    pub fn local_only(session_id: impl Into<String>) -> Self {
        Self::new(session_id, FeedbackManagerConfig::default())
    }

    /// Get a clone of the signals handle for tracking events.
    pub fn signals_handle(&self) -> SessionSignalsHandle {
        self.signals_handle.clone()
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Check if feedback collection is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.feedback_enabled
    }


    /// Client type for this session (Agent, Tui, Web, etc.).
    pub fn client_type(&self) -> prod_mc_cli_chat_proxy_types::feedback_types::ClientType {
        self.config.client_type
    }

    /// Build and submit text feedback from the `/feedback` slash command.
    pub(crate) async fn submit_text_feedback(
        &self,
        text: String,
        session_data: SessionFeedbackData,
        persistence_tx: Option<&tokio::sync::mpsc::UnboundedSender<PersistenceMsg>>,
        telemetry_enabled: bool,
    ) -> SubmitOutcome {
        let sh = self.signals_handle();
        let (signals, tool_outcomes) = tokio::join!(sh.snapshot(), sh.last_turn_tool_outcomes());
        let signals = signals.unwrap_or_default();
        let turn_number = signals.turn_count.saturating_sub(1) as i64;
        let tool_outcomes: Vec<FeedbackToolOutcome> = tool_outcomes
            .into_iter()
            .map(|o| FeedbackToolOutcome {
                tool_name: o.tool_name,
                calls: o.successes + o.failures,
                failures: o.failures,
            })
            .collect();

        let mut submission = new_submission(
            self.session_id.clone(),
            self.config.client_type,
            FeedbackContent::Text(text),
        );
        submission.turn_number = Some(turn_number);
        submission.model_id = session_data.model_id;
        submission.resolved_model_id = session_data.resolved_model_id;
        submission.last_user_message = None;
        submission.last_assistant_message = None;
        submission.tool_outcomes = tool_outcomes;
        submission.session_cwd = Some(session_data.session_cwd);
        submission.compaction_count = Some(signals.compaction_count as i64);
        submission.context_window_usage = Some(signals.context_window_usage);
        submission.context_tokens_used = Some(signals.context_tokens_used);
        submission.context_window_tokens = Some(signals.context_window_tokens);
        submission.client_version = session_data.client_version;

        let author_identity =
            crate::util::user_identity::cached_identity(self.config.user.as_ref()).await;

        submit_feedback_workflow(
            &mut submission,
            persistence_tx,
            SubmitFeedbackOptions {
                solicited: false,
                telemetry_enabled,
                author_identity,
            },
        )
        .await
    }

    /// Check if config has been loaded from the server.
    /// Evaluate heuristics and return a FeedbackRequest if one should be sent.
    ///
    /// Call this after each turn to check if feedback should be requested.
    /// Returns None if:
    /// - No tier criteria are met
    /// - The tier was already triggered this session
    /// - Probabilistic sampling says no
    ///
    /// When a request is triggered, this method also creates a record via the
    /// feedback API for tracking and analytics.
    #[tracing::instrument(name = "feedback.maybe_request_feedback", skip_all, fields(
        session_id = %self.session_id,
    ))]
    pub async fn maybe_request_feedback(
        &self,
        prompt_id: Option<String>,
    ) -> Option<FeedbackRequest> {
        if !self.config.feedback_enabled {
            return None;
        }

        let signals = self.signals_handle.snapshot().await?;
        let mut heuristics = self.heuristics.write().await;

        // Check if heuristics are globally enabled (from server config)
        if !heuristics.is_enabled() {
            return None;
        }

        let eval = heuristics.evaluate(&signals);

        if let (true, Some(trigger_condition)) =
            (eval.should_request, eval.trigger_condition.as_ref())
        {
            let tier = trigger_condition.tier;
            // Use the feedback mode configured for this tier
            let feedback_mode = heuristics.feedback_mode(tier);
            let dismissible = heuristics.dismissible(tier);
            let prompt = heuristics.prompt(tier);
            let request = FeedbackRequest::with_mode(
                self.session_id.clone(),
                trigger_condition.clone(),
                feedback_mode,
                dismissible,
                Some(prompt),
            );
            tracing::info!(
                session_id = %self.session_id,
                tier = ?request.tier,
                trigger_type = %request.trigger_type,
                feedback_mode = ?request.feedback_mode,
                "Feedback request triggered"
            );

            let _ = prompt_id;
            return Some(request);
        }

        None
    }

    /// Force check heuristics without sampling (for testing).
    /// Returns the evaluation result.
    pub async fn evaluate_heuristics(&self) -> Option<FeedbackEvaluation> {
        let signals = self.signals_handle.snapshot().await?;
        let mut heuristics = self.heuristics.write().await;
        Some(heuristics.evaluate(&signals))
    }

    /// Force-generate a feedback request for local testing, bypassing all
    /// heuristics, sampling, cooldown, and enabled checks.
    ///
    /// Engineers developing clients can call this via the
    /// `x.ai/debug/trigger_feedback` ACP extension method to exercise
    /// the full feedback notification ↔ response flow without needing a
    /// real session that meets tier criteria.
    ///
    /// When a `feedback_client` is configured, the request is also recorded
    /// via the feedback API — exactly like a real trigger — so that the
    /// subsequent `complete_request` / `dismiss_request` round-trip from the
    /// client works end-to-end.
    #[tracing::instrument(name = "feedback.force_feedback_request", skip_all, fields(
        session_id = %self.session_id,
    ))]
    pub async fn force_feedback_request(
        &self,
        tier: FeedbackTier,
        mode: FeedbackMode,
    ) -> FeedbackRequest {
        use crate::session::feedback::TriggerSignalSnapshot;

        // Build a synthetic trigger condition that makes it obvious this was
        // manually triggered for testing purposes.
        let condition = TriggerCondition {
            tier,
            condition: "debug/trigger_feedback (manual test trigger)".to_string(),
            signal_snapshot: TriggerSignalSnapshot {
                turn_count: 0,
                tool_calls_count: 0,
                compactions_count: 0,
                errors_count: 0,
                cancellations_count: 0,
                has_reverted: false,
            },
        };

        // Manual/debug triggers are always dismissible regardless of tier config,
        // since they exist for developer testing, not real user feedback collection.
        let request = FeedbackRequest::with_mode(
            self.session_id.clone(),
            condition.clone(),
            mode,
            true,
            None,
        );

        request
    }

    /// Shutdown the manager (shuts down the signals actor).
    pub async fn shutdown(&self) {
        self.signals_handle.shutdown();
    }
}
fn tier_to_priority(tier: crate::session::feedback::FeedbackTier) -> i32 {
    use crate::session::feedback::FeedbackTier;
    match tier {
        FeedbackTier::Tier1 => 5, // Standard engagement
        FeedbackTier::Tier2 => 6, // Complex session with recovery
        FeedbackTier::Tier3 => 7, // Recovery from friction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_feedback_manager_local_only() {
        let manager = FeedbackManager::local_only("test-session-123");

        // Track some events
        let signals = manager.signals_handle();
        for _ in 0..10 {
            signals.increment_turn();
        }
        for _ in 0..5 {
            signals.record_tool_call("read_file");
        }
        for _ in 0..2 {
            signals.record_compaction(10_000);
        }

        // Give time for actor to process
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Check signals were tracked
        let snapshot = signals.snapshot().await.unwrap();
        assert_eq!(snapshot.turn_count, 10);
        assert_eq!(snapshot.tool_call_count, 5);
        assert_eq!(snapshot.compaction_count, 2);

        // Evaluate heuristics - should trigger Tier 1
        let eval = manager.evaluate_heuristics().await.unwrap();
        assert!(eval.trigger_condition.is_some());
        assert_eq!(
            eval.trigger_condition.as_ref().unwrap().tier,
            crate::session::feedback::FeedbackTier::Tier1
        );

        manager.shutdown().await;
    }

}
