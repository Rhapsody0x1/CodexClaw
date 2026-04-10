use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

use codex_claw::{
    app::App,
    codex::executor::{CodexExecutor, ExecutionRequest},
    config::{AppConfig, GeneralConfig, QqConfig},
    qq::api::QqApiClient,
    session::{state::SessionState, store::SessionStore},
};
use tempfile::tempdir;
use tokio::fs;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

#[tokio::test]
async fn executor_parses_mock_codex_jsonl() {
    let dir = tempdir().unwrap();
    let script_path = dir.path().join("mock-codex.sh");
    fs::write(
        &script_path,
        r#"#!/bin/sh
printf '%s\n' '{"type":"thread.started","thread_id":"thread-123"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
    )
    .await
    .unwrap();
    let mut perms = fs::metadata(&script_path).await.unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).await.unwrap();

    let executor = CodexExecutor {
        binary: script_path,
        sqlite_home: dir.path().join("sqlite-home"),
    };
    let result = executor
        .execute(ExecutionRequest {
            prompt: "hi".into(),
            workspace_dir: dir.path().to_path_buf(),
            session_state: SessionState::default(),
            model: Some("gpt-test".into()),
            service_tier: None,
            context_mode: None,
            reasoning_effort: Default::default(),
            image_paths: Vec::new(),
        }, None, None)
        .await
        .unwrap();
    assert_eq!(result.session_id.as_deref(), Some("thread-123"));
    assert_eq!(result.text, "hello");
}

#[tokio::test]
async fn app_emits_tool_summary_before_following_text() {
    let dir = tempdir().unwrap();
    let mock_server = MockServer::start().await;
    mock_access_token(&mock_server).await;
    mock_any_send_text(&mock_server, "user-1").await;

    let script_path = dir.path().join("mock-codex-stream.sh");
    fs::write(
        &script_path,
        r#"#!/bin/sh
printf '%s\n' '{"type":"thread.started","thread_id":"thread-456"}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"pwd","status":"in_progress"}}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_2","type":"command_execution","command":"ls","status":"in_progress"}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"第一段回复"}}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_4","type":"command_execution","command":"cat","status":"in_progress"}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_5","type":"agent_message","text":"第二段回复"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
    )
    .await
    .unwrap();
    let mut perms = fs::metadata(&script_path).await.unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).await.unwrap();

    let mut config = test_config(dir.path().to_path_buf(), mock_server.uri());
    config.general.codex_binary = script_path.to_string_lossy().into_owned();
    let app = build_app(config, dir.path().to_path_buf()).await;

    app.handle_c2c_event(codex_claw::qq::types::C2CMessageEvent {
        id: "msg-2".to_string(),
        content: "hello".to_string(),
        author: codex_claw::qq::types::EventAuthor {
            user_openid: "user-1".to_string(),
        },
        attachments: Vec::new(),
        message_type: Some(0),
        msg_elements: Vec::new(),
    })
    .await
    .unwrap();

    let requests = mock_server.received_requests().await.unwrap();
    let message_bodies = requests
        .iter()
        .filter(|request| request.url.path() == "/v2/users/user-1/messages")
        .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(message_bodies.len(), 4);
    assert_eq!(message_bodies[0]["msg_type"], 2);
    assert_eq!(message_bodies[0]["markdown"]["content"], "[Tool: Bash] * 2");
    assert_eq!(message_bodies[0]["msg_seq"], 1);
    assert_eq!(message_bodies[1]["markdown"]["content"], "第一段回复");
    assert_eq!(message_bodies[1]["msg_seq"], 2);
    assert_eq!(message_bodies[2]["markdown"]["content"], "[Tool: Bash]");
    assert_eq!(message_bodies[2]["msg_seq"], 3);
    assert_eq!(message_bodies[3]["markdown"]["content"], "第二段回复");
    assert_eq!(message_bodies[3]["msg_seq"], 4);
}

#[tokio::test]
async fn app_emits_verbose_tool_details_when_enabled() {
    let dir = tempdir().unwrap();
    let mock_server = MockServer::start().await;
    mock_access_token(&mock_server).await;
    mock_any_send_text(&mock_server, "user-1").await;

    let script_path = dir.path().join("mock-codex-verbose.sh");
    fs::write(
        &script_path,
        r#"#!/bin/sh
printf '%s\n' '{"type":"thread.started","thread_id":"thread-456"}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"pwd","status":"in_progress"}}'
printf '%s\n' '{"type":"item.started","item":{"id":"item_2","type":"command_execution","command":"ls","status":"in_progress"}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"第一段回复"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
    )
    .await
    .unwrap();
    let mut perms = fs::metadata(&script_path).await.unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).await.unwrap();

    let mut config = test_config(dir.path().to_path_buf(), mock_server.uri());
    config.general.codex_binary = script_path.to_string_lossy().into_owned();
    let app = build_app(config, dir.path().to_path_buf()).await;
    app.session
        .update_settings(|state| state.settings.verbose = true)
        .await
        .unwrap();

    app.handle_c2c_event(codex_claw::qq::types::C2CMessageEvent {
        id: "msg-verbose".to_string(),
        content: "hello".to_string(),
        author: codex_claw::qq::types::EventAuthor {
            user_openid: "user-1".to_string(),
        },
        attachments: Vec::new(),
        message_type: Some(0),
        msg_elements: Vec::new(),
    })
    .await
    .unwrap();

    let requests = mock_server.received_requests().await.unwrap();
    let message_bodies = requests
        .iter()
        .filter(|request| request.url.path() == "/v2/users/user-1/messages")
        .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        message_bodies[0]["markdown"]["content"],
        "[Tool: Bash]\n```shell\npwd\n```\n[Tool: Bash]\n```shell\nls\n```"
    );
}

#[tokio::test]
async fn qq_text_send_falls_back_to_plain_text_when_markdown_is_rejected() {
    let mock_server = MockServer::start().await;
    mock_access_token(&mock_server).await;
    Mock::given(method("POST"))
        .and(path("/v2/users/user-1/messages"))
        .and(body_json(serde_json::json!({
            "msg_type": 2,
            "msg_id": "msg-3",
            "msg_seq": 1,
            "markdown": { "content": "**hello**" },
            "message_reference": { "message_id": "msg-3" }
        })))
        .respond_with(ResponseTemplate::new(400).set_body_string("markdown rejected"))
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/users/user-1/messages"))
        .and(body_json(serde_json::json!({
            "content": "**hello**",
            "msg_type": 0,
            "msg_id": "msg-3",
            "msg_seq": 1,
            "message_reference": { "message_id": "msg-3" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&mock_server)
        .await;

    let client = QqApiClient::new(QqConfig {
        app_id: "app".into(),
        app_secret: "secret".into(),
        api_base_url: mock_server.uri(),
        token_url: format!("{}/app/getAppAccessToken", mock_server.uri()),
    })
    .unwrap();

    client
        .send_text("user-1", "msg-3", "**hello**", Some("msg-3"))
        .await
        .unwrap();

    let requests = mock_server.received_requests().await.unwrap();
    let message_bodies = requests
        .iter()
        .filter(|request| request.url.path() == "/v2/users/user-1/messages")
        .map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(message_bodies.len(), 2);
    assert_eq!(message_bodies[0]["msg_type"], 2);
    assert_eq!(message_bodies[1]["msg_type"], 0);
}

async fn build_app(config: AppConfig, data_dir: PathBuf) -> Arc<App> {
    let codex_binary = config.general.codex_binary.clone();
    let session = Arc::new(SessionStore::load_or_init(&data_dir).await.unwrap());
    let qq_client = Arc::new(QqApiClient::new(config.qq.clone()).unwrap());
    let codex = Arc::new(CodexExecutor::new(codex_binary, config.general.data_dir.clone()));
    Arc::new(App::new(config, session, qq_client, codex))
}

async fn mock_access_token(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/app/getAppAccessToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "token-1",
            "expires_in": 7200
        })))
        .mount(server)
        .await;
}

async fn mock_any_send_text(server: &MockServer, openid: &str) {
    Mock::given(method("POST"))
        .and(path(format!("/v2/users/{openid}/messages")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(server)
        .await;
}

fn test_config(data_dir: PathBuf, base_url: String) -> AppConfig {
    AppConfig {
        general: GeneralConfig {
            data_dir,
            codex_binary: "codex".into(),
            default_model: "gpt-5-codex".into(),
            default_reasoning_effort: Default::default(),
        },
        qq: QqConfig {
            app_id: "app".into(),
            app_secret: "secret".into(),
            api_base_url: base_url.clone(),
            token_url: format!("{base_url}/app/getAppAccessToken"),
        },
    }
}
