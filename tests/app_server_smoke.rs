//! End-to-end smoke test: spawn a real `codex app-server` and drive one turn
//! through the new [`AppServerHandle`] / [`CodexExecutor`] pipeline.
//!
//! Requires a working codex binary on $PATH and valid `~/.codex-claw/.codex`
//! auth. Run with:
//!
//! ```
//! cargo test --test app_server_smoke -- --ignored --nocapture
//! ```

use std::{path::PathBuf, sync::Arc, time::Duration};

use codex_claw::codex::{
    app_server::{AppServerHandle, ClientInfo, TurnPolicy},
    executor::{ExecutionRequest, ExecutionUpdate, build_codex_path_env},
};
use codex_claw::session::state::{ReasoningEffort, SessionState};
use tokio::sync::mpsc;

#[tokio::test]
#[ignore = "requires real codex binary and auth; run manually"]
async fn one_shot_turn_round_trips_through_app_server() {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex-claw").join(".codex"))
        })
        .expect("CODEX_HOME resolvable");

    let path_env = build_codex_path_env(
        std::env::var_os("PATH").as_ref(),
        std::env::var_os("HOME")
            .as_deref()
            .map(std::path::Path::new),
    );

    let handle = AppServerHandle::start(
        PathBuf::from("codex"),
        codex_home.clone(),
        codex_home.join("sqlite"),
        path_env,
        ClientInfo {
            name: "codex-claw-smoke".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: None,
        },
    )
    .await
    .expect("start app-server");

    let (tx, mut rx) = mpsc::unbounded_channel::<ExecutionUpdate>();
    let update_task = tokio::spawn(async move {
        let mut seen = Vec::<ExecutionUpdate>::new();
        while let Some(u) = rx.recv().await {
            seen.push(u);
        }
        seen
    });

    let workspace = tempfile::tempdir().unwrap();
    let request = ExecutionRequest {
        prompt: "Print the literal string EXACTLY: ACK".to_string(),
        workspace_dir: workspace.path().to_path_buf(),
        codex_home: PathBuf::from("/tmp"), // ignored in app-server path
        config_overrides: Vec::new(),
        add_dirs: Vec::new(),
        session_state: SessionState::default(),
        model: Some("gpt-5.4".to_string()),
        service_tier: None,
        context_mode: None,
        reasoning_effort: ReasoningEffort::Medium,
        image_paths: Vec::new(),
    };

    // Inherit sandbox/approval from ~/.codex-claw/.codex/config.toml so the
    // smoke test matches the real bot behaviour.
    let policy = TurnPolicy::inherit_from_config();
    let result = tokio::time::timeout(
        Duration::from_secs(90),
        Arc::new(handle.clone()).execute(request, policy, None, Some(tx)),
    )
    .await
    .expect("turn did not complete in time")
    .expect("turn failed");

    let updates = update_task.await.unwrap();
    println!("session_id = {:?}", result.session_id);
    println!("text       = {:?}", result.text);
    println!("updates    = {}", updates.len());
    for u in &updates {
        println!("  {u:?}");
    }

    assert!(result.session_id.is_some(), "session id captured");
    assert!(
        !updates.is_empty(),
        "at least one update emitted (bash/agent/reasoning)"
    );
}
