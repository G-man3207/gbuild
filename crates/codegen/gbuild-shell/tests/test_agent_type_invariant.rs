//! Agent-type invariant integration tests.
//!
//! These tests exercise the full shell lifecycle via ACP stdio against a mock
//! inference server, verifying that `agent_type = f(model)` holds across:
//!
//! - Session creation with default models
//! - Zero-turn model switching (harness rebuild)
//! - Mid-session model switching (rejection)
//! - Same-type model switching (no rebuild)
//! - Session resume
//!
//! Each test spawns a real `gbuild agent stdio` process, speaks the full ACP
//! protocol, and asserts on the inference request bodies (system prompt) and
//! stderr tracing output to verify the correct harness was used.
//!
//! Run locally:
//! ```bash
//! cargo test -p gbuild-shell --test test_agent_type_invariant -- --ignored
//! ```
use gbuild_test_support::*;
use std::future::Future;
async fn with_local_set<F, Fut>(f: F)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::task::LocalSet::new().run_until(f()).await;
}
/// Delete the shell's on-disk models cache so the next process is forced to
/// re-fetch from the mock server's `/v1/models` endpoint. Without this, the
/// second spawn in a resume test reads the stale cache written by phase 1
/// and never sees the updated model list.
fn invalidate_models_cache(home: &std::path::Path) {
    let cache = home.join(".gbuild").join("models_cache.json");
    if cache.exists() {
        std::fs::remove_file(&cache).expect("failed to delete models_cache.json");
    }
}
/// Start a mock server with two models:
/// - `default-model`: no agent_type (→ defaults to "gbuild")
async fn dual_model_server() -> MockInferenceServer {
    MockInferenceServer::start_with_models(vec![
        MockModelEntry::new("default-model"),
        MockModelEntry::with_agent_type("cursor-model", "cursor"),
    ])
    .await
    .expect("start mock server")
}
/// Start a mock server with two models that share the same agent_type:
/// - `model-a`: no agent_type (→ "gbuild")
/// - `model-b`: no agent_type (→ "gbuild")
async fn same_type_server() -> MockInferenceServer {
    MockInferenceServer::start_with_models(vec![
        MockModelEntry::new("model-a"),
        MockModelEntry::new("model-b"),
    ])
    .await
    .expect("start mock server")
}
/// Session created with a model that has no `agent_type` should use the
/// `gbuild` harness. The system prompt sent to the LLM should contain
/// the gbuild identity string.
#[tokio::test]
#[ignore]
async fn test_default_model_uses_gbuild_harness() {
    with_local_set(|| async {
        let server = MockInferenceServer::start()
            .await
            .expect("start mock server");
        let workdir = git_workdir();
        let client = GBuildStdioClient::spawn(&server, workdir.workspace()).await;
        client.initialize_with_timeout().await;
        let session_id = client
            .create_session_with_timeout(workdir.workspace())
            .await;
        let result = client.prompt_with_timeout(&session_id, "say hello").await;
        assert!(result.is_ok(), "prompt failed: {:?}", result.err());
        let sys_prompt = server
            .last_system_prompt()
            .expect("should have at least one inference request");
        assert!(
            sys_prompt.contains("gBuild") || sys_prompt.contains("gbuild"),
            "default model should use gbuild harness\nsystem prompt preview: {}",
            &sys_prompt[..sys_prompt.len().min(500)]
        );
    })
    .await;
}
/// Switching between two models with the same agent_type should succeed
/// without a harness rebuild.
#[tokio::test]
#[ignore]
async fn test_same_type_model_switch_no_rebuild() {
    with_local_set(|| async {
        let server = same_type_server().await;
        let workdir = git_workdir();
        let client = GBuildStdioClient::spawn(&server, workdir.workspace()).await;
        client.initialize_with_timeout().await;
        let session_id = client
            .create_session_with_model_timeout(workdir.workspace(), "model-a")
            .await;
        let result = client.prompt_with_timeout(&session_id, "say hello").await;
        assert!(result.is_ok(), "first prompt failed: {:?}", result.err());
        let switch_result = client.set_model_with_timeout(&session_id, "model-b").await;
        assert!(
            switch_result.is_ok(),
            "same-type model switch should succeed\nerror: {:?}\nstderr: {}",
            switch_result.err(),
            stderr_tail(&client.stderr(), 2000)
        );
        let result2 = client.prompt_with_timeout(&session_id, "say goodbye").await;
        assert!(
            result2.is_ok(),
            "second prompt after model switch failed: {:?}",
            result2.err()
        );
    })
    .await;
}
/// A session created with the default model, persisted, and reloaded should
/// still use the same harness — the system prompt in the resumed session's
/// first inference request should match the original.
#[tokio::test]
#[ignore]
async fn test_session_resume_preserves_harness() {
    with_local_set(|| async {
        let server = MockInferenceServer::start()
            .await
            .expect("start mock server");
        let workdir = git_workdir();
        let mut writer = GBuildStdioClient::spawn(&server, workdir.workspace()).await;
        writer.initialize_with_timeout().await;
        let session_id = writer
            .create_session_with_timeout(workdir.workspace())
            .await;
        let result = writer.prompt_with_timeout(&session_id, "say hello").await;
        assert!(result.is_ok(), "prompt failed: {:?}", result.err());
        let original_sys_prompt = server
            .last_system_prompt()
            .expect("should have captured system prompt");
        let shared_sandbox = writer.take_sandbox();
        invalidate_models_cache(shared_sandbox.home());
        drop(writer);
        let reader =
            GBuildStdioClient::spawn_with_sandbox(&server, workdir.workspace(), shared_sandbox)
                .await;
        reader.initialize_with_timeout().await;
        let _ = reader
            .load_session_with_timeout(&session_id, workdir.workspace())
            .await;
        let result2 = reader.prompt_with_timeout(&session_id, "say goodbye").await;
        assert!(
            result2.is_ok(),
            "resumed prompt failed: {:?}",
            result2.err()
        );
        let resumed_sys_prompt = server
            .last_system_prompt()
            .expect("should have captured resumed system prompt");
        let original_has_gbuild =
            original_sys_prompt.contains("gBuild") || original_sys_prompt.contains("gbuild");
        let resumed_has_gbuild =
            resumed_sys_prompt.contains("gBuild") || resumed_sys_prompt.contains("gbuild");
        assert_eq!(
            original_has_gbuild,
            resumed_has_gbuild,
            "resumed session should use the same harness as the original\n\
             original identity markers: gbuild={original_has_gbuild}\n\
             resumed identity markers: gbuild={resumed_has_gbuild}\n\
             original prompt (first 300): {}\n\
             resumed prompt (first 300): {}",
            &original_sys_prompt[..original_sys_prompt.len().min(300)],
            &resumed_sys_prompt[..resumed_sys_prompt.len().min(300)],
        );
    })
    .await;
}
/// A model that doesn't declare `agent_type` in its metadata should
/// default to `"gbuild"`. This exercises the serde default.
#[tokio::test]
#[ignore]
async fn test_model_without_agent_type_defaults_to_gbuild() {
    with_local_set(|| async {
        let server = MockInferenceServer::start_with_models(vec![MockModelEntry::new(
            "no-agent-type-model",
        )])
        .await
        .expect("start mock server");
        let workdir = git_workdir();
        let client = GBuildStdioClient::spawn(&server, workdir.workspace()).await;
        client.initialize_with_timeout().await;
        let session_id = client
            .create_session_with_model_timeout(workdir.workspace(), "no-agent-type-model")
            .await;
        let result = client.prompt_with_timeout(&session_id, "say hello").await;
        assert!(result.is_ok(), "prompt failed: {:?}", result.err());
        let sys_prompt = server
            .last_system_prompt()
            .expect("should have at least one inference request");
        assert!(
            sys_prompt.contains("gBuild") || sys_prompt.contains("gbuild"),
            "model without agent_type should default to gbuild harness\nsystem prompt preview: {}",
            &sys_prompt[..sys_prompt.len().min(500)]
        );
    })
    .await;
}
/// The `GBUILD_AGENT` escape hatch should override the model's agent_type.
/// Setting `GBUILD_AGENT=gbuild` with an alternate-agent model should use
/// gbuild harness.
#[tokio::test]
#[ignore]
async fn test_gbuild_agent_env_overrides_model_agent_type() {
    with_local_set(|| async {
            let server = dual_model_server().await;
            let workdir = git_workdir();
            let sandbox = TestSandbox::builder().mock_url(server.url()).build();
            let client = GBuildStdioClient::spawn_with_sandbox_env_and_args(
                    &server,
                    workdir.workspace(),
                    sandbox,
                    &[("GBUILD_AGENT", "gbuild")],
                    &[],
                )
                .await;
            client.initialize_with_timeout().await;
            let session_id = client
                .create_session_with_model_timeout(workdir.workspace(), "cursor-model")
                .await;
            let result = client.prompt_with_timeout(&session_id, "say hello").await;
            assert!(
            result.is_ok(),
            "prompt with GBUILD_AGENT override failed: {:?}\nstderr:\n{}",
            result.err(),
            client.stderr()
        );
            let sys_prompt = server
                .last_system_prompt()
                .expect("should have inference request");
            assert!(
            sys_prompt.contains("gBuild") || sys_prompt.contains("gbuild"),
            "GBUILD_AGENT=gbuild should override catalog model agent_type\nsystem prompt preview: {}",
            &sys_prompt[..sys_prompt.len().min(500)]
        );
        })
        .await;
}
