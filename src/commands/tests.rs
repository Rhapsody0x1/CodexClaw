use std::path::Path;

use tempfile::{TempDir, tempdir};
use tokio::fs;

use crate::{
    codex::CodexRuntimeProfile,
    session::{
        SessionStore,
        state::{
            ContextMode, DialogProfile, PendingSetting, ReasoningEffort, ServiceTier,
            UserSessionState, fixtures::usage,
        },
    },
};

use super::{CommandOutcome, CommandReply, maybe_handle_command};

/// Every command test drives the same single user.
const USER: &str = "u1";

/// Shared harness for the command tests.
///
/// The `TempDir` handles must live as long as the store: dropping them is
/// what deletes the directories on disk, so they are owned here rather than
/// returned separately.
struct TestEnv {
    data: TempDir,
    home: TempDir,
    session: SessionStore,
    default_model: &'static str,
}

impl TestEnv {
    async fn new() -> Self {
        Self::with_default_model("default").await
    }

    async fn with_default_model(default_model: &'static str) -> Self {
        let data = tempdir().unwrap();
        let home = tempdir().unwrap();
        let session = SessionStore::load_or_init(data.path(), home.path(), home.path())
            .await
            .unwrap();
        Self {
            data,
            home,
            session,
            default_model,
        }
    }

    fn data_path(&self) -> &Path {
        self.data.path()
    }

    /// Codex home used both as the global and the system rollout root.
    fn home_path(&self) -> &Path {
        self.home.path()
    }

    async fn run(&self, text: &str) -> CommandOutcome {
        self.dispatch(text, &CodexRuntimeProfile::default(), false)
            .await
    }

    async fn run_busy(&self, text: &str) -> CommandOutcome {
        self.dispatch(text, &CodexRuntimeProfile::default(), true)
            .await
    }

    async fn run_with_runtime(&self, text: &str, runtime: &CodexRuntimeProfile) -> CommandOutcome {
        self.dispatch(text, runtime, false).await
    }

    async fn dispatch(
        &self,
        text: &str,
        runtime: &CodexRuntimeProfile,
        is_busy: bool,
    ) -> CommandOutcome {
        maybe_handle_command(
            text,
            USER,
            &self.session,
            self.default_model,
            runtime,
            is_busy,
        )
        .await
        .unwrap()
    }

    async fn reply(&self, text: &str) -> CommandReply {
        match self.run(text).await {
            CommandOutcome::Reply(reply) => reply,
            _ => panic!("expected `{text}` to reply"),
        }
    }

    async fn reply_busy(&self, text: &str) -> CommandReply {
        match self.run_busy(text).await {
            CommandOutcome::Reply(reply) => reply,
            _ => panic!("expected busy `{text}` to reply"),
        }
    }

    async fn snapshot(&self) -> UserSessionState {
        self.session.snapshot_for_user(USER).await.unwrap()
    }

    async fn set_lang(&self, lang: &str) {
        self.session
            .update_settings_for_user(USER, |state| state.language = lang.into())
            .await
            .unwrap();
    }
}

#[track_caller]
fn expect_reply(outcome: CommandOutcome) -> CommandReply {
    match outcome {
        CommandOutcome::Reply(reply) => reply,
        _ => panic!("expected a reply outcome"),
    }
}

/// Assert every needle occurs in `text` and that they occur in the listed
/// order. Panics name the offending needle so a failure is locatable.
#[track_caller]
fn assert_ordered(group: &str, text: &str, needles: &[&str]) {
    let mut previous: Option<(&str, usize)> = None;
    for needle in needles {
        let at = text
            .find(needle)
            .unwrap_or_else(|| panic!("{group}: missing {needle}"));
        if let Some((before, before_at)) = previous {
            assert!(
                before_at < at,
                "{group}: {before} must come before {needle}"
            );
        }
        previous = Some((needle, at));
    }
}

#[tokio::test]
async fn resume_recovery_retry_enters_retry_outcome_and_clears_pending() {
    let env = TestEnv::new().await;
    env.session
        .set_pending_setting(USER, Some(PendingSetting::ResumeRecovery))
        .await
        .unwrap();

    let outcome = env.run("/retry").await;

    assert!(matches!(outcome, CommandOutcome::RetryResume));
    assert!(env.snapshot().await.pending_setting.is_none());
}

#[tokio::test]
async fn new_command_keeps_settings() {
    let env = TestEnv::new().await;
    env.session
        .set_foreground_session_id(USER, Some("thread".into()))
        .await
        .unwrap();
    env.session
        .update_settings_for_user(USER, |state| {
            state.model_override = Some("gpt-x".into());
            state.verbose = true;
        })
        .await
        .unwrap();

    let outcome = env.run("/new").await;

    assert!(matches!(outcome, CommandOutcome::Reply(_)));
    let snapshot = env.snapshot().await;
    assert!(snapshot.foreground.session_id.is_none());
    assert_eq!(snapshot.settings.model_override.as_deref(), Some("gpt-x"));
    assert!(snapshot.settings.verbose);
}

#[tokio::test]
async fn new_command_accepts_manual_workspace() {
    let env = TestEnv::new().await;

    let reply = env.reply("/new custom folder").await;

    let snapshot = env.snapshot().await;
    let expected =
        std::fs::canonicalize(env.data_path().join("session/workspace/custom folder")).unwrap();
    assert_eq!(snapshot.foreground.workspace_dir, expected);
    assert!(reply.text.to_lowercase().contains("workdir"));
    assert!(reply.text.contains("custom folder"));
}

#[tokio::test]
async fn new_command_reports_effective_runtime_settings() {
    let env = TestEnv::new().await;
    env.session
        .update_settings_for_user(USER, |state| {
            state.model_override = Some("ignored-legacy".into());
            state.reasoning_effort = Some(ReasoningEffort::Low);
            state.context_mode = Some(ContextMode::Standard);
        })
        .await
        .unwrap();

    let runtime = CodexRuntimeProfile {
        configured_model: Some("gpt-global".into()),
        reasoning_effort: Some(ReasoningEffort::High),
        service_tier: Some(ServiceTier::Fast),
        context_mode: Some(ContextMode::OneM),
    };
    let reply = expect_reply(env.run_with_runtime("/new", &runtime).await);

    assert!(reply.text.contains("gpt-global high 1M fast"));
}

#[tokio::test]
async fn resume_command_reports_profile_and_last_user_message_preview() {
    let env = TestEnv::new().await;
    let session_dir = env.home_path().join("sessions/2026/04/11");
    fs::create_dir_all(&session_dir).await.unwrap();
    fs::write(
        session_dir.join("rollout-2026-04-11T00-00-00-thread-1.jsonl"),
        r#"{"type":"session_meta","payload":{"id":"thread-1","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/project-a"}}
{"type":"turn_context","payload":{"cwd":"/tmp/project-a","model":"gpt-5.4","effort":"high"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":950000}}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"You are CodexClaw running behind QQ official bot.\n\nUser message:\n请帮我处理发布失败"}]}}
"#,
    )
    .await
    .unwrap();

    let reply = env.reply("/resume thread-1").await;

    assert!(reply.text.contains("请帮我处理发布失败"));
    assert!(reply.text.contains("gpt-5.4 high 1M"));
}

#[tokio::test]
async fn stop_command_restores_most_recent_background_dialog() {
    let env = TestEnv::new().await;
    env.session
        .bind_foreground_session_profile(
            USER,
            Some("thread-older".into()),
            DialogProfile {
                model_override: Some("gpt-older".into()),
                reasoning_effort: Some(ReasoningEffort::Low),
                service_tier: None,
                context_mode: Some(ContextMode::Standard),
            },
        )
        .await
        .unwrap();
    env.session
        .move_foreground_to_background(USER, Some("older"))
        .await
        .unwrap();
    env.session
        .bind_foreground_session_profile(
            USER,
            Some("thread-newer".into()),
            DialogProfile {
                model_override: Some("gpt-newer".into()),
                reasoning_effort: Some(ReasoningEffort::High),
                service_tier: None,
                context_mode: Some(ContextMode::OneM),
            },
        )
        .await
        .unwrap();
    env.session
        .move_foreground_to_background(USER, Some("newer"))
        .await
        .unwrap();
    env.session
        .set_foreground_session_id(USER, Some("thread-current".into()))
        .await
        .unwrap();

    let outcome = env.run("/stop").await;

    let CommandOutcome::StopCurrent(reply) = outcome else {
        panic!("expected stop current");
    };
    assert!(reply.contains("`newer`"));
    assert!(reply.contains("gpt-newer high 1M"));
}

#[tokio::test]
async fn stop_command_ends_session() {
    let env = TestEnv::new().await;
    env.session
        .set_foreground_session_id(USER, Some("thread".into()))
        .await
        .unwrap();

    let outcome = env.run("/stop").await;

    assert!(matches!(outcome, CommandOutcome::StopCurrent(_)));
    assert!(env.snapshot().await.foreground.session_id.is_none());
}

#[tokio::test]
async fn interrupt_command_does_not_end_session() {
    let env = TestEnv::new().await;
    env.session
        .set_foreground_session_id(USER, Some("thread".into()))
        .await
        .unwrap();

    let outcome = env.run_busy("/interrupt").await;

    assert!(matches!(outcome, CommandOutcome::CancelCurrent(_)));
    let snapshot = env.snapshot().await;
    assert_eq!(snapshot.foreground.session_id.as_deref(), Some("thread"));
}

#[tokio::test]
async fn busy_profile_commands_do_not_mutate_saved_foreground_profile() {
    let env = TestEnv::new().await;
    let original = DialogProfile {
        model_override: Some("gpt-original".into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        service_tier: None,
        context_mode: Some(ContextMode::Standard),
    };
    env.session
        .bind_foreground_session_profile(USER, Some("thread".into()), original.clone())
        .await
        .unwrap();
    env.session.save_foreground(USER).await.unwrap();

    for command in ["/model gpt-next", "/reasoning xhigh", "/context 1m"] {
        let reply = env.reply_busy(command).await;
        assert!(
            reply.text.to_lowercase().contains("already running"),
            "unexpected busy reply: {}",
            reply.text
        );
    }

    let snapshot = env.snapshot().await;
    assert_eq!(snapshot.foreground.profile.as_ref(), Some(&original));
}

#[tokio::test]
async fn compact_command_routes_to_manual_compaction() {
    let env = TestEnv::new().await;

    assert!(matches!(env.run("/compact").await, CommandOutcome::Compact));
    assert!(matches!(env.run("/压缩").await, CommandOutcome::Compact));
}

#[tokio::test]
async fn sessions_command_supports_project_then_session_view() {
    let env = TestEnv::new().await;
    let session_dir = env.home_path().join("sessions/2026/04/11");
    fs::create_dir_all(&session_dir).await.unwrap();
    fs::write(
        session_dir.join("rollout-2026-04-11T00-00-00-thread-a.jsonl"),
        r#"{"type":"session_meta","payload":{"id":"thread-a","timestamp":"2026-04-11T00:00:00Z","cwd":"/tmp/p1"}}"#,
    )
    .await
    .unwrap();
    env.session
        .set_foreground_session_id(USER, Some("thread-a".into()))
        .await
        .unwrap();
    env.session.save_foreground(USER).await.unwrap();

    let project_reply = env.reply("/sessions").await;
    assert!(project_reply.text.to_lowercase().contains("projects"));

    let session_reply = env.reply("/sessions 1").await;
    assert!(session_reply.text.to_lowercase().contains("no summary"));
    assert!(!session_reply.text.contains("thread-a"));
}

#[tokio::test]
async fn help_output_is_fully_localized() {
    // Headings, command names and the `/help` alias itself must all follow
    // the active language, with no leakage from the other one.
    struct Case {
        lang: &'static str,
        help_command: &'static str,
        title: &'static str,
        headings: &'static [&'static str],
        commands: &'static [&'static str],
        absent_commands: &'static [&'static str],
    }
    let cases = [
        Case {
            lang: "en",
            help_command: "/help",
            title: "# Command Guide",
            headings: &[
                "## Basic Commands",
                "## Model Settings",
                "## Session Management",
            ],
            commands: &["`/model`", "`/compact`"],
            absent_commands: &["`/模型`", "`/压缩`"],
        },
        Case {
            lang: "zh",
            help_command: "/帮助",
            title: "# 命令指南",
            headings: &["## 基础命令", "## 模型设置命令", "## 会话管理命令"],
            commands: &["`/模型`", "`/压缩`"],
            absent_commands: &["`/model`", "`/compact`"],
        },
    ];

    for case in cases {
        let env = TestEnv::new().await;
        let lang_reply = env.reply(&format!("/lang {}", case.lang)).await;
        assert!(
            lang_reply.text.contains(case.lang),
            "case: {} — /lang reply did not confirm the language: {}",
            case.lang,
            lang_reply.text
        );

        // Both the canonical `/help` and the localized alias must render
        // the same localized guide.
        for command in ["/help", case.help_command] {
            let reply = env.reply(command).await;
            assert!(
                reply.text.starts_with(case.title),
                "case: {} via {command} — unexpected title: {}",
                case.lang,
                reply.text
            );
            for heading in case.headings {
                assert!(
                    reply.text.contains(heading),
                    "case: {} via {command} — missing heading {heading}",
                    case.lang
                );
            }
            for name in case.commands {
                assert!(
                    reply.text.contains(name),
                    "case: {} via {command} — missing command {name}",
                    case.lang
                );
            }
            for name in case.absent_commands {
                assert!(
                    !reply.text.contains(name),
                    "case: {} via {command} — leaked other-language command {name}",
                    case.lang
                );
            }
        }
    }
}

const EXPERT_ALIAS: &str = "/alias add expert /model gpt-5.4 | /reasoning xhigh | /verbose on";

#[tokio::test]
async fn alias_add_stores_every_step() {
    let env = TestEnv::new().await;

    let reply = env.reply(EXPERT_ALIAS).await;
    assert!(reply.text.contains("expert"));

    let aliases = env.session.list_command_aliases(USER).await.unwrap();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].commands.len(), 3);
}

#[tokio::test]
async fn alias_expansion_executes_each_step() {
    let env = TestEnv::new().await;
    let _ = env.run(EXPERT_ALIAS).await;

    env.session
        .set_foreground_session_id(USER, Some("thread-1".into()))
        .await
        .unwrap();
    env.session
        .move_foreground_to_background(USER, Some("saved"))
        .await
        .unwrap();
    env.session
        .foreground_from_background(USER, "saved")
        .await
        .unwrap();

    let reply = env.reply("/expert").await;

    let snapshot = env.snapshot().await;
    let profile = snapshot.foreground.profile.expect("saved dialog profile");
    assert!(snapshot.settings.verbose, "the /verbose step must apply");
    assert_eq!(profile.reasoning_effort, Some(ReasoningEffort::Xhigh));
    assert_eq!(profile.model_override.as_deref(), Some("gpt-5.4"));
    assert!(reply.text.contains("expert"));
}

#[tokio::test]
async fn alias_add_rejects_built_in_command_names() {
    let env = TestEnv::new().await;

    for name in ["help", "compact"] {
        let reply = env.reply(&format!("/alias add {name} /status")).await;
        assert!(
            reply.text.to_lowercase().contains("built-in") || reply.text.contains("内置命令"),
            "case: {name} — unexpected reply: {}",
            reply.text
        );
    }
}

#[tokio::test]
async fn alias_names_are_normalized_to_lowercase() {
    let env = TestEnv::new().await;

    let reply = env.reply("/alias add Expert /verbose on").await;
    assert!(reply.text.contains("/expert"));

    let invoke = env.run("/EXPERT").await;
    assert!(matches!(invoke, CommandOutcome::Reply(_)));
    assert!(env.snapshot().await.settings.verbose);
}

#[tokio::test]
async fn lang_switch_affects_foreground_switch_messages() {
    let env = TestEnv::new().await;
    env.session
        .set_foreground_session_id(USER, Some("thread".into()))
        .await
        .unwrap();
    let _ = env.run("/bg focus").await;
    let _ = env.run("/lang en").await;

    let reply = env.reply("/fg focus").await;

    assert!(reply.text.contains("Switched to background session"));
}

#[tokio::test]
async fn alias_recursion_capped_at_max_depth() {
    let env = TestEnv::new().await;

    // Three-step recursion chain: a -> b -> c -> a (cycle)
    for (name, step) in [("a", "/b"), ("b", "/c"), ("c", "/a")] {
        let _ = env.run(&format!("/alias add {name} {step}")).await;
    }

    let reply = env.reply("/a").await;

    // Must have produced some text and terminated (no panic / stack overflow)
    assert!(!reply.text.is_empty());
}

#[tokio::test]
async fn model_empty_args_enters_interactive_prompt() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let reply = env.reply("/model").await;

    assert!(reply.text.to_lowercase().contains("gpt-5.4"));
    assert!(reply.text.contains("`gpt-5.4`"));
    assert!(reply.text.contains("aliases") || reply.text.contains("别名"));
    assert!(
        reply.text.contains("Latest flagship GPT-5.6 model")
            || reply.text.contains("最新旗舰 GPT-5.6 模型")
    );
    assert!(matches!(
        env.snapshot().await.pending_setting,
        Some(PendingSetting::Model)
    ));
}

#[tokio::test]
async fn model_prompt_keeps_hint_out_of_markdown_sublist() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.set_lang("zh").await;

    let reply = env.reply("/model").await;

    assert!(
        reply.text.contains("\n\n请输入一个值，或 `/返回` 取消。"),
        "hint should be separated from the markdown list: {}",
        reply.text
    );
}

#[tokio::test]
async fn pending_model_fuzzy_match_applies_and_clears() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let _ = env.run("/model").await;

    // Ambiguous prefix: "gpt-5." hits multiple canonical models.
    let reply = env.reply("gpt-5.").await;
    assert!(reply.text.to_lowercase().contains("multiple") || reply.text.contains("匹配到多个"));
    assert!(
        env.snapshot().await.pending_setting.is_some(),
        "pending must stay on ambiguous input"
    );

    // Unique fuzzy: "mini" hits only gpt-5.4-mini
    let outcome = env.run("mini").await;
    let CommandOutcome::SetGlobalModel(Some(model)) = outcome else {
        panic!("expected apply reply");
    };
    assert_eq!(model, "gpt-5.4-mini");
    let snapshot = env.snapshot().await;
    assert!(
        snapshot.pending_setting.is_none(),
        "pending should clear on apply"
    );
    assert_eq!(snapshot.settings.model_override.as_deref(), None);
}

#[tokio::test]
async fn busy_pending_profile_input_does_not_apply_or_clear_picker() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    let original = DialogProfile {
        model_override: Some("gpt-original".into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        service_tier: None,
        context_mode: Some(ContextMode::Standard),
    };
    env.session
        .bind_foreground_session_profile(USER, Some("thread".into()), original.clone())
        .await
        .unwrap();
    env.session.save_foreground(USER).await.unwrap();

    let _ = env.run("/model").await;

    let reply = env.reply_busy("gpt-next").await;
    assert!(
        reply.text.to_lowercase().contains("already running"),
        "unexpected busy reply: {}",
        reply.text
    );

    let snapshot = env.snapshot().await;
    assert_eq!(snapshot.foreground.profile.as_ref(), Some(&original));
    assert!(matches!(
        snapshot.pending_setting,
        Some(PendingSetting::Model)
    ));
}

#[tokio::test]
async fn back_when_idle_reports_no_interactive_setting() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let reply = env.reply("/back").await;

    assert!(
        reply.text.to_lowercase().contains("not currently") || reply.text.contains("当前没有"),
        "unexpected idle /back reply: {}",
        reply.text
    );
}

#[tokio::test]
async fn back_exits_the_pending_setting_and_names_it() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    let _ = env.run("/reasoning").await;
    assert!(env.snapshot().await.pending_setting.is_some());

    let reply = env.reply("/back").await;

    assert!(reply.text.contains("/reasoning"));
    assert!(env.snapshot().await.pending_setting.is_none());
}

#[tokio::test]
async fn other_command_during_pending_clears_and_prefixes() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let _ = env.run("/model").await;

    let reply = env.reply("/status").await;

    assert!(
        reply.text.contains("/model"),
        "exit notice must name the prior command"
    );
    assert!(reply.text.to_lowercase().contains("workdir"));
    assert!(env.snapshot().await.pending_setting.is_none());
}

#[tokio::test]
async fn status_uses_context_window_remaining_format() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.session
        .set_foreground_usage(USER, usage(13_700, 272_000))
        .await
        .unwrap();

    let reply = env.reply("/status").await;

    assert!(
        reply.text.contains("99% left"),
        "unexpected status: {}",
        reply.text
    );
    assert!(reply.text.contains("14K used / 272K"));
}

#[tokio::test]
async fn status_hides_implausible_legacy_cumulative_usage() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.session
        .set_foreground_usage(
            USER,
            crate::session::state::fixtures::legacy_cumulative_usage(),
        )
        .await
        .unwrap();

    let reply = env.reply("/status").await;

    assert!(
        reply.text.contains("context window: —"),
        "unexpected status: {}",
        reply.text
    );
}

#[tokio::test]
async fn chinese_model_alias_enters_the_same_pending_as_model() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let outcome = env.run("/模型").await;

    assert!(matches!(outcome, CommandOutcome::Reply(_)));
    assert!(matches!(
        env.snapshot().await.pending_setting,
        Some(PendingSetting::Model)
    ));
}

#[tokio::test]
async fn chinese_back_alias_clears_pending_and_names_the_chinese_command() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    let _ = env.run("/语言 zh").await;
    let _ = env.run("/模型").await;

    let reply = env.reply("/返回").await;

    assert!(reply.text.contains("/模型"));
    assert!(!reply.text.contains("/model"));
    assert!(env.snapshot().await.pending_setting.is_none());
}

#[tokio::test]
async fn reasoning_prompt_uses_supported_values_and_aliases() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.set_lang("zh").await;

    let reply = env.reply("/reasoning").await;

    assert!(reply.text.contains("当前思考深度：medium"));
    assert!(
        reply
            .text
            .contains("low (低), medium (中), high (高), xhigh (超高), inherit (默认)")
    );
    assert!(reply.text.contains("\n请输入一个值，或 `/返回` 取消。"));
    assert!(!reply.text.contains("- `low`"));
    assert!(!reply.text.contains("恢复默认"));
}

#[tokio::test]
async fn reasoning_prompt_uses_compact_three_line_layout() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.set_lang("zh").await;

    let reply = env.reply("/reasoning").await;

    assert_eq!(
        reply.text.lines().count(),
        3,
        "unexpected prompt: {}",
        reply.text
    );
}

#[tokio::test]
async fn pending_reasoning_alias_applies_supported_value() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let _ = env.run("/reasoning").await;

    let outcome = env.run("高").await;

    let CommandOutcome::SetGlobalReasoning(Some(ReasoningEffort::High)) = outcome else {
        panic!("expected global reasoning update");
    };
    let snapshot = env.snapshot().await;
    assert_eq!(snapshot.settings.reasoning_effort, None);
    assert!(snapshot.pending_setting.is_none());
}

#[tokio::test]
async fn direct_fast_and_context_commands_accept_chinese_aliases() {
    let env = TestEnv::with_default_model("gpt-5.4").await;

    let fast = env.run("/fast 开").await;
    let CommandOutcome::SetGlobalFast(Some(ServiceTier::Fast)) = fast else {
        panic!("expected /fast 开 to request a global fast update");
    };

    let context = env.run("/context 长").await;
    let CommandOutcome::SetGlobalContext(Some(ContextMode::OneM)) = context else {
        panic!("expected /context 长 to request a global context update");
    };

    let snapshot = env.snapshot().await;
    assert_eq!(snapshot.settings.service_tier, None);
    assert_eq!(snapshot.settings.context_mode, None);
}

#[tokio::test]
async fn bare_fg_switches_to_most_recent_background() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.session
        .set_foreground_session_id(USER, Some("thread".into()))
        .await
        .unwrap();
    let _ = env.run("/bg focus").await;

    let reply = env.reply("/fg").await;

    assert!(
        reply.text.contains("`focus`"),
        "bare /fg should return to the most recently parked dialog: {}",
        reply.text
    );
    let snapshot = env.snapshot().await;
    assert_eq!(snapshot.foreground.session_id.as_deref(), Some("thread"));
}

#[tokio::test]
async fn bare_fg_without_background_reports_empty() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.set_lang("zh").await;

    let reply = env.reply("/fg").await;

    assert!(
        reply.text.contains("暂无后台会话"),
        "empty background should be reported, not a picker: {}",
        reply.text
    );
}

#[tokio::test]
async fn fg_argument_fuzzy_matches_an_alias() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.session
        .set_foreground_session_id(USER, Some("thread".into()))
        .await
        .unwrap();
    let _ = env.run("/bg harbor").await;

    let reply = env.reply("/fg harb").await;

    assert!(
        reply.text.contains("`harbor`"),
        "a unique substring should resolve to the alias: {}",
        reply.text
    );
}

#[tokio::test]
async fn help_groups_commands_in_requested_order() {
    let env = TestEnv::with_default_model("gpt-5.4").await;
    env.set_lang("zh").await;

    let reply = env.reply("/帮助").await;

    assert_ordered(
        "sections",
        &reply.text,
        &[
            "## 模型设置命令",
            "## 审批设置命令",
            "## 会话管理命令",
            "## 高级命令",
        ],
    );
    assert_ordered(
        "model settings",
        &reply.text,
        &["`/模型`", "`/思考`", "`/快速`", "`/上下文`"],
    );
    assert_ordered(
        "approval settings",
        &reply.text,
        &[
            "`/审批`",
            "`/计划`",
            "`/实施`",
            "`/继续规划`",
            "`/取消计划`",
            "`/同意`",
            "`/同意本会话`",
            "`/拒绝`",
            "`/取消`",
        ],
    );
    assert_ordered(
        "session management",
        &reply.text,
        &["`/会话`", "`/导入`", "`/恢复`", "`/保存`"],
    );
    assert_ordered(
        "advanced",
        &reply.text,
        &[
            "`/压缩`",
            "`/前台`",
            "`/后台`",
            "`/载入后台`",
            "`/重命名`",
            "`/别名`",
            "`/详细`",
            "`/自更新`",
        ],
    );
}
