//! `x.ai/feedback`, `x.ai/feedback/dismiss`, and `x.ai/btw` extension handlers.
//!
//! - `feedback`/`feedback/dismiss`: persist user ratings/text locally (the
//!   cli-chat-proxy forwarding has been removed from this fork).
//! - `btw`: dispatch a side question to the active session via
//!   `SessionCommand::SideQuestion` and return the answer.

use std::sync::Arc;

use agent_client_protocol as acp;
use tokio::sync::oneshot;

use super::{ExtResult, parse_params};
use crate::agent::MvpAgent;
use crate::session::persistence::{LocalFeedbackEntry, UserFeedbackEntry};
use crate::session::{
    ClientFeedbackInput, FeedbackRequestDismiss, FeedbackResponse, SessionCommand,
};

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        "x.ai/btw" => {
            tracing::info!("handling /btw side question");
            handle_btw(agent, args).await
        }
        "x.ai/feedback" | "x.ai/feedback/dismiss" => {
            tracing::info!("handling user feedback");
            handle_feedback(agent, args).await
        }
        _ => Err(acp::Error::method_not_found()),
    }
}

/// Handle `x.ai/btw` -- a side question that doesn't interrupt the current turn.
async fn handle_btw(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BtwRequest {
        session_id: String,
        question: String,
    }

    let req: BtwRequest = parse_params(args)?;
    let sid: acp::SessionId = req.session_id.clone().into();
    let session_handle = {
        let sessions = agent.sessions.borrow();
        sessions.get(&sid).cloned()
    };
    let Some(session) = session_handle else {
        return Err(
            acp::Error::invalid_params().data(format!("session not found: {}", req.session_id))
        );
    };
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::SideQuestion {
        question: req.question,
        respond_to: tx,
    });
    let result = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    match result {
        Ok(answer) => super::to_ext_response(Ok(serde_json::json!({
            "answer": answer,
        }))),
        Err(e) => Err(acp::Error::internal_error().data(e)),
    }
}

async fn handle_feedback(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    if !agent.cfg.borrow().is_feedback_enabled() {
        return Err(acp::Error::internal_error().data(
            "Feedback is disabled. To enable, set GBUILD_FEEDBACK_ENABLED=true or \
             [features] feedback = true in config.toml.",
        ));
    }

    match args.method.as_ref() {
        "x.ai/feedback" => {
            // Parse the input -- try the full ClientFeedbackInput first,
            // then fall back to the simple FeedbackRequest (from /feedback slash command)
            // which only has {session_id, feedback_text} and no client_type.
            let feedback_input: ClientFeedbackInput =
                match serde_json::from_str::<ClientFeedbackInput>(args.params.get()) {
                    Ok(input) => input,
                    Err(_) => {
                        // Fallback: parse simple FeedbackRequest from /feedback command
                        let simple: crate::session::FeedbackRequest = parse_params(args)?;
                        ClientFeedbackInput {
                            session_id: simple.session_id,
                            client_type:
                                prod_mc_cli_chat_proxy_types::feedback_types::ClientType::Tui,
                            rating_type: None,
                            rating_value: None,
                            feedback_text: Some(simple.feedback_text),
                            feedback_categories: vec![],
                            context_type: None,
                            turn_number: None,
                            request_id: None,
                            client_version: None,
                            metadata: None,
                            terminal_info: None,
                        }
                    }
                };

            let session_id = acp::SessionId::new(feedback_input.session_id.clone());
            let session_handle = agent.sessions.borrow().get(&session_id).cloned();

            let (model_id, model_metadata) = if let Some(ref session) = session_handle {
                let (tx1, rx1) = tokio::sync::oneshot::channel();
                let _ = session
                    .cmd_tx
                    .send(SessionCommand::GetCurrentModel { responds_to: tx1 });
                let model_id = rx1.await.ok();

                let model_metadata = session.get_model_metadata().await;

                (model_id, model_metadata)
            } else {
                let sampling_config = agent.sampling_config.borrow().clone();
                (Some(sampling_config.model.clone()), Default::default())
            };

            let turn_number = feedback_input.turn_number.or_else(|| {
                agent
                    .session_turn_number(&session_id)
                    .map(|t| t.saturating_sub(1) as i64)
            });

            let mut submission = feedback_input.to_submission(
                model_id.clone(),
                model_metadata.resolved_model_id,
                model_metadata.model_fingerprint,
                turn_number,
            );
            let turn_number = submission.turn_number;

            // Enrich with session context for Slack notifications (best-effort).
            if let Some(ref session_handle) = session_handle {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = session_handle
                    .cmd_tx
                    .send(SessionCommand::GetFeedbackContext {
                        turn_number,
                        responds_to: tx,
                    });
                if let Ok(ctx) = rx.await {
                    submission.tool_outcomes = ctx.tool_outcomes;
                    submission.session_cwd = Some(ctx.session_cwd);
                    submission.compaction_count = Some(ctx.compaction_count);
                    submission.context_window_usage = Some(ctx.context_window_usage);
                    submission.context_tokens_used = Some(ctx.context_tokens_used);
                    submission.context_window_tokens = Some(ctx.context_window_tokens);
                }
            }

            // Track rating in session signals
            if let (Some(session_handle), Some(rating_value)) =
                (&session_handle, feedback_input.rating_value)
            {
                use prod_mc_cli_chat_proxy_types::feedback_types::RatingType;
                let (is_positive, is_negative) = match feedback_input.rating_type {
                    // Thumbs: -1 = down, 0 = neutral, 1 = up
                    Some(RatingType::Thumbs) | None => (rating_value > 0, rating_value < 0),
                    // Stars (1-5): >= 4 positive, <= 2 negative, 3 neutral
                    Some(RatingType::Stars) => (rating_value >= 4, rating_value <= 2),
                    // NPS (0-10): 9-10 promoter, 0-6 detractor, 7-8 passive
                    Some(RatingType::Nps) => (rating_value >= 9, rating_value <= 6),
                };
                if is_positive {
                    session_handle.signals_handle.record_positive_rating();
                } else if is_negative {
                    session_handle.signals_handle.record_negative_rating();
                }
            }

            // Log feedback type for debugging
            if feedback_input.is_solicited() {
                tracing::info!(
                    session_id = %feedback_input.session_id,
                    request_id = ?feedback_input.request_id(),
                    turn_number = ?turn_number,
                    "Solicited feedback received (response to feedback request)"
                );
            } else {
                tracing::info!(
                    session_id = %feedback_input.session_id,
                    turn_number = ?turn_number,
                    "Spontaneous user feedback received"
                );
            }

            let telemetry_enabled = {
                let cfg = agent.cfg.borrow();
                cfg.is_telemetry_enabled()
                    && !agent
                        .auth_manager
                        .current_or_expired()
                        .is_some_and(|a| a.is_zdr_team())
            };
            // Read the live feedback.user config (the session-actor path uses its
            // spawn-time snapshot); both dedupe through the same process-wide
            // identity cache, so a stable config resolves identically either way.
            // Clone out so the RefCell borrow doesn't span an await.
            let user_cfg = agent.cfg.borrow().feedback.user.clone();
            let author_identity =
                crate::util::user_identity::cached_identity(user_cfg.as_ref()).await;
            crate::session::feedback_manager::submit_feedback_workflow(
                &mut submission,
                session_handle.as_ref().map(|h| &h.persistence_tx),
                crate::session::feedback_manager::SubmitFeedbackOptions {
                    solicited: feedback_input.is_solicited(),
                    telemetry_enabled,
                    author_identity,
                },
            )
            .await;

            let value = serde_json::to_value(FeedbackResponse { success: true })
                .map(|value| serde_json::value::to_raw_value(&value).map(Arc::from))
                .expect("to work")
                .expect("to work");
            Ok(acp::ExtResponse::new(value))
        }
        "x.ai/feedback/dismiss" => {
            let dismiss_input: FeedbackRequestDismiss = parse_params(args)?;

            tracing::info!(
                session_id = %dismiss_input.session_id,
                request_id = %dismiss_input.request_id,
                "Feedback request dismissed by user"
            );

            // Count dismissals too (else event_type is always "responded" and
            // response-rate is unknowable), gated like the responded path so a
            // ZDR team emits no survey data and the ratio stays comparable.
            let telemetry_enabled = {
                let cfg = agent.cfg.borrow();
                cfg.is_telemetry_enabled()
                    && !agent
                        .auth_manager
                        .current_or_expired()
                        .is_some_and(|a| a.is_zdr_team())
            };
            if telemetry_enabled {
                tracing::info_span!(
                    "feedback.survey",
                    survey_type = "session",
                    event_type = "dismissed",
                    appearance_id = %dismiss_input.request_id,
                    has_feedback_text = false,
                    is_solicited = true,
                )
                .in_scope(|| {});
            }

            // Persist dismiss locally; flushed before storage CopyFile by the persistence actor.
            {
                let session_id = acp::SessionId::new(dismiss_input.session_id.clone());
                if let Some(session_handle) = agent.sessions.borrow().get(&session_id) {
                    session_handle.persist_feedback(LocalFeedbackEntry::UserFeedback(
                        UserFeedbackEntry {
                            submitted_at: chrono::Utc::now(),
                            session_id: dismiss_input.session_id.clone(),
                            turn_number: None,
                            solicited: true,
                            request_id: Some(dismiss_input.request_id.clone()),
                            dismissed: true,
                            submission: None,
                        },
                    ));
                }
            }

            let value = serde_json::to_value(FeedbackResponse { success: true })
                .map(|value| serde_json::value::to_raw_value(&value).map(Arc::from))
                .expect("to work")
                .expect("to work");
            Ok(acp::ExtResponse::new(value))
        }
        _ => Err(acp::Error::method_not_found()),
    }
}

