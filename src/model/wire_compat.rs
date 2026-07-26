//! Golden on-disk wire format for the documents in this module.
//!
//! `state.json` and `jobs.json` are written by long-running installs, so the
//! serde representation of these types is a compatibility surface, not an
//! implementation detail. The snapshot below was captured from the pre-`model/`
//! definitions; any field rename, variant rename, `rename_all`, `tag`,
//! `default` or `skip_serializing_if` change will fail this test.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::model::cron::{
    CronJob, CronKind, DeliverPolicy, InteractiveSpec, JobAction, PendingDelivery, RunStatus,
    SessionStrategy,
};
use crate::model::settings::{
    ApprovalPolicySetting, CommandAlias, ContextMode, DialogOrigin, DialogProfile, DialogState,
    ImportedSessionProfile, PendingSetting, PersistedSessionState, ReasoningEffort, ServiceTier,
    SessionSettings, SessionState, TokenUsageSnapshot, UserSessionState,
};

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(secs, 0).expect("valid timestamp")
}

fn settings() -> SessionSettings {
    SessionSettings {
        model_override: Some("gpt-5".to_string()),
        reasoning_effort: Some(ReasoningEffort::Xhigh),
        service_tier: Some(ServiceTier::Flex),
        context_mode: Some(ContextMode::OneM),
        verbose: true,
        plan_mode: true,
        approval_policy_override: Some(ApprovalPolicySetting::GuardianSubagent),
        pending_plan: Some("plan text".to_string()),
        language: "zh".to_string(),
    }
}

fn profile() -> DialogProfile {
    DialogProfile {
        model_override: Some("gpt-5-codex".to_string()),
        reasoning_effort: Some(ReasoningEffort::Minimal),
        service_tier: Some(ServiceTier::Fast),
        context_mode: Some(ContextMode::Standard),
    }
}

fn usage() -> TokenUsageSnapshot {
    TokenUsageSnapshot {
        total_tokens: 1234,
        window: 272_000,
        input_tokens: 1000,
        cached_input_tokens: 900,
        output_tokens: 234,
        updated_at: ts(1_700_000_000),
    }
}

fn cron_jobs() -> BTreeMap<String, CronJob> {
    let mut jobs = BTreeMap::new();
    jobs.insert(
        "job-recurring".to_string(),
        CronJob {
            id: "job-recurring".to_string(),
            owner_openid: "openid-1".to_string(),
            title: "daily".to_string(),
            kind: CronKind::Recurring {
                cron: "0 9 * * *".to_string(),
                tz: "Asia/Shanghai".to_string(),
            },
            action: JobAction::CodexTurn {
                prompt: "do it".to_string(),
                model: Some("gpt-5".to_string()),
                session_state: Some(SessionState {
                    session_id: Some("sess-1".to_string()),
                    settings: settings(),
                }),
                approval_policy: Some(ApprovalPolicySetting::UnlessTrusted),
                session_strategy: SessionStrategy::Persistent,
                interactive: Some(InteractiveSpec {
                    reply_ttl_secs: 60,
                    end_signal: "<<<END>>>".to_string(),
                    max_rounds_hard_cap: 3,
                }),
            },
            workspace_dir: PathBuf::from("/tmp/ws"),
            deliver: DeliverPolicy::PushTruncated { max_chars: 100 },
            created_at: ts(1_700_000_001),
            next_run_at: Some(ts(1_700_000_002)),
            run_now_at: Some(ts(1_700_000_003)),
            last_run_at: Some(ts(1_700_000_004)),
            last_run_status: Some(RunStatus::Success {
                duration_ms: 12,
                output_chars: 34,
            }),
            run_count: 7,
            failure_streak: 2,
            disabled: true,
        },
    );
    jobs.insert(
        "job-oneshot".to_string(),
        CronJob {
            id: "job-oneshot".to_string(),
            owner_openid: "openid-2".to_string(),
            title: "once".to_string(),
            kind: CronKind::OneShot {
                at: ts(1_700_000_005),
            },
            action: JobAction::Reminder {
                message: "ping".to_string(),
            },
            workspace_dir: PathBuf::from("/tmp/ws2"),
            deliver: DeliverPolicy::PushIfNonEmpty,
            created_at: ts(1_700_000_006),
            next_run_at: None,
            run_now_at: None,
            last_run_at: None,
            last_run_status: Some(RunStatus::Failure {
                error: "boom".to_string(),
                attempt: 1,
            }),
            run_count: 0,
            failure_streak: 0,
            disabled: false,
        },
    );
    jobs.insert(
        "job-exec".to_string(),
        CronJob {
            id: "job-exec".to_string(),
            owner_openid: "openid-3".to_string(),
            title: "exec".to_string(),
            kind: CronKind::OneShot {
                at: ts(1_700_000_007),
            },
            action: JobAction::CodexExec {
                prompt: "run".to_string(),
                model: None,
                extra_args: vec!["--full-auto".to_string()],
                env: BTreeMap::from([("K".to_string(), "V".to_string())]),
            },
            workspace_dir: PathBuf::from("/tmp/ws3"),
            deliver: DeliverPolicy::LogOnly,
            created_at: ts(1_700_000_008),
            next_run_at: None,
            run_now_at: None,
            last_run_at: None,
            last_run_status: Some(RunStatus::Skipped {
                reason: "no-op".to_string(),
            }),
            run_count: 0,
            failure_streak: 0,
            disabled: false,
        },
    );
    jobs.insert(
        "job-shell".to_string(),
        CronJob {
            id: "job-shell".to_string(),
            owner_openid: "openid-4".to_string(),
            title: "shell".to_string(),
            kind: CronKind::OneShot {
                at: ts(1_700_000_009),
            },
            action: JobAction::Shell {
                program: "echo".to_string(),
                args: vec!["hi".to_string()],
                env: BTreeMap::new(),
            },
            workspace_dir: PathBuf::from("/tmp/ws4"),
            deliver: DeliverPolicy::PushToOwner,
            created_at: ts(1_700_000_010),
            next_run_at: None,
            run_now_at: None,
            last_run_at: None,
            last_run_status: None,
            run_count: 0,
            failure_streak: 0,
            disabled: false,
        },
    );
    jobs
}

fn persisted() -> PersistedSessionState {
    let user = UserSessionState {
        foreground: DialogState {
            session_id: Some("fg-session".to_string()),
            origin: DialogOrigin::Global,
            workspace_dir: PathBuf::from("/home/u/ws"),
            saved: true,
            profile: Some(profile()),
            last_usage: Some(usage()),
        },
        background: BTreeMap::from([(
            "bg1".to_string(),
            DialogState {
                session_id: None,
                origin: DialogOrigin::Local,
                workspace_dir: PathBuf::from("/home/u/bg"),
                saved: false,
                profile: None,
                last_usage: None,
            },
        )]),
        background_order: vec!["bg1".to_string()],
        settings: settings(),
        alias_seq: 3,
        last_projects_view: vec!["p1".to_string()],
        last_sessions_view: vec!["s1".to_string()],
        last_import_projects_view: vec!["ip1".to_string()],
        last_import_sessions_view: vec!["is1".to_string()],
        saved_local_session_ids: vec!["sl1".to_string()],
        command_aliases: BTreeMap::from([(
            "a".to_string(),
            CommandAlias {
                name: "a".to_string(),
                commands: vec!["/help".to_string()],
                created_at: ts(1_700_000_011),
            },
        )]),
        pending_setting: Some(PendingSetting::LoadbgSessions {
            project_key: "pk".to_string(),
            page: 2,
            alias: Some("al".to_string()),
        }),
    };

    PersistedSessionState {
        users: BTreeMap::from([("openid-1".to_string(), user)]),
        imported_profiles: BTreeMap::from([(
            "imp".to_string(),
            ImportedSessionProfile {
                workspace_dir: PathBuf::from("/imp/ws"),
                model_override: Some("gpt-5".to_string()),
                reasoning_effort: Some(ReasoningEffort::None),
                service_tier: Some(ServiceTier::Fast),
                context_mode: Some(ContextMode::OneM),
            },
        )]),
        cron_jobs: cron_jobs(),
    }
}

/// Every field, every enum variant and every tagged representation of the
/// `state.json` document, including the embedded `cron_jobs` map.
const PERSISTED_STATE_JSON: &str = r#"{
  "users": {
    "openid-1": {
      "foreground": {
        "session_id": "fg-session",
        "origin": "global",
        "workspace_dir": "/home/u/ws",
        "saved": true,
        "profile": {
          "model_override": "gpt-5-codex",
          "reasoning_effort": "minimal",
          "service_tier": "fast",
          "context_mode": "standard"
        },
        "last_usage": {
          "total_tokens": 1234,
          "window": 272000,
          "input_tokens": 1000,
          "cached_input_tokens": 900,
          "output_tokens": 234,
          "updated_at": "2023-11-14T22:13:20Z"
        }
      },
      "background": {
        "bg1": {
          "session_id": null,
          "origin": "local",
          "workspace_dir": "/home/u/bg",
          "saved": false,
          "profile": null,
          "last_usage": null
        }
      },
      "background_order": [
        "bg1"
      ],
      "settings": {
        "model_override": "gpt-5",
        "reasoning_effort": "xhigh",
        "service_tier": "flex",
        "context_mode": "1m",
        "verbose": true,
        "plan_mode": true,
        "approval_policy_override": "guardian-subagent",
        "pending_plan": "plan text",
        "language": "zh"
      },
      "alias_seq": 3,
      "last_projects_view": [
        "p1"
      ],
      "last_sessions_view": [
        "s1"
      ],
      "last_import_projects_view": [
        "ip1"
      ],
      "last_import_sessions_view": [
        "is1"
      ],
      "saved_local_session_ids": [
        "sl1"
      ],
      "command_aliases": {
        "a": {
          "name": "a",
          "commands": [
            "/help"
          ],
          "created_at": "2023-11-14T22:13:31Z"
        }
      },
      "pending_setting": {
        "kind": "loadbg_sessions",
        "project_key": "pk",
        "page": 2,
        "alias": "al"
      }
    }
  },
  "imported_profiles": {
    "imp": {
      "workspace_dir": "/imp/ws",
      "model_override": "gpt-5",
      "reasoning_effort": "none",
      "service_tier": "fast",
      "context_mode": "1m"
    }
  },
  "cron_jobs": {
    "job-exec": {
      "id": "job-exec",
      "owner_openid": "openid-3",
      "title": "exec",
      "kind": {
        "type": "one-shot",
        "at": "2023-11-14T22:13:27Z"
      },
      "action": {
        "type": "codex-exec",
        "prompt": "run",
        "model": null,
        "extra_args": [
          "--full-auto"
        ],
        "env": {
          "K": "V"
        }
      },
      "workspace_dir": "/tmp/ws3",
      "deliver": {
        "type": "log-only"
      },
      "created_at": "2023-11-14T22:13:28Z",
      "next_run_at": null,
      "run_now_at": null,
      "last_run_at": null,
      "last_run_status": {
        "type": "skipped",
        "reason": "no-op"
      },
      "run_count": 0,
      "failure_streak": 0,
      "disabled": false
    },
    "job-oneshot": {
      "id": "job-oneshot",
      "owner_openid": "openid-2",
      "title": "once",
      "kind": {
        "type": "one-shot",
        "at": "2023-11-14T22:13:25Z"
      },
      "action": {
        "type": "reminder",
        "message": "ping"
      },
      "workspace_dir": "/tmp/ws2",
      "deliver": {
        "type": "push-if-non-empty"
      },
      "created_at": "2023-11-14T22:13:26Z",
      "next_run_at": null,
      "run_now_at": null,
      "last_run_at": null,
      "last_run_status": {
        "type": "failure",
        "error": "boom",
        "attempt": 1
      },
      "run_count": 0,
      "failure_streak": 0,
      "disabled": false
    },
    "job-recurring": {
      "id": "job-recurring",
      "owner_openid": "openid-1",
      "title": "daily",
      "kind": {
        "type": "recurring",
        "cron": "0 9 * * *",
        "tz": "Asia/Shanghai"
      },
      "action": {
        "type": "codex-turn",
        "prompt": "do it",
        "model": "gpt-5",
        "session_state": {
          "session_id": "sess-1",
          "settings": {
            "model_override": "gpt-5",
            "reasoning_effort": "xhigh",
            "service_tier": "flex",
            "context_mode": "1m",
            "verbose": true,
            "plan_mode": true,
            "approval_policy_override": "guardian-subagent",
            "pending_plan": "plan text",
            "language": "zh"
          }
        },
        "approval_policy": "unless-trusted",
        "session_strategy": "persistent",
        "interactive": {
          "reply_ttl_secs": 60,
          "end_signal": "<<<END>>>",
          "max_rounds_hard_cap": 3
        }
      },
      "workspace_dir": "/tmp/ws",
      "deliver": {
        "type": "push-truncated",
        "max_chars": 100
      },
      "created_at": "2023-11-14T22:13:21Z",
      "next_run_at": "2023-11-14T22:13:22Z",
      "run_now_at": "2023-11-14T22:13:23Z",
      "last_run_at": "2023-11-14T22:13:24Z",
      "last_run_status": {
        "type": "success",
        "duration_ms": 12,
        "output_chars": 34
      },
      "run_count": 7,
      "failure_streak": 2,
      "disabled": true
    },
    "job-shell": {
      "id": "job-shell",
      "owner_openid": "openid-4",
      "title": "shell",
      "kind": {
        "type": "one-shot",
        "at": "2023-11-14T22:13:29Z"
      },
      "action": {
        "type": "shell",
        "program": "echo",
        "args": [
          "hi"
        ],
        "env": {}
      },
      "workspace_dir": "/tmp/ws4",
      "deliver": {
        "type": "push-to-owner"
      },
      "created_at": "2023-11-14T22:13:30Z",
      "next_run_at": null,
      "run_now_at": null,
      "last_run_at": null,
      "last_run_status": null,
      "run_count": 0,
      "failure_streak": 0,
      "disabled": false
    }
  }
}"#;

#[test]
fn persisted_state_matches_golden_wire_format() {
    assert_eq!(
        serde_json::to_string_pretty(&persisted()).unwrap(),
        PERSISTED_STATE_JSON
    );
}

#[test]
fn persisted_state_round_trips_through_the_golden_wire_format() {
    let parsed: PersistedSessionState = serde_json::from_str(PERSISTED_STATE_JSON).unwrap();
    assert_eq!(parsed, persisted());
}

#[test]
fn pending_delivery_matches_golden_wire_format() {
    let delivery = PendingDelivery {
        job_id: "j".to_string(),
        title: "t".to_string(),
        text: "x".to_string(),
        failed_at: ts(1_700_000_012),
        error: "e".to_string(),
    };
    assert_eq!(
        serde_json::to_string_pretty(&delivery).unwrap(),
        r#"{
  "job_id": "j",
  "title": "t",
  "text": "x",
  "failed_at": "2023-11-14T22:13:32Z",
  "error": "e"
}"#
    );
}

/// The `#[serde(default = ..)]` helpers and `#[derive(Default)]` choices decide
/// what an older document deserializes into, so they are pinned too.
#[test]
fn defaults_match_golden_wire_format() {
    let cases: Vec<(&str, String)> = vec![
        (
            r#"{
  "reply_ttl_secs": 86400,
  "end_signal": "<<<CLAW_END>>>",
  "max_rounds_hard_cap": 10
}"#,
            serde_json::to_string_pretty(&InteractiveSpec::default()).unwrap(),
        ),
        (
            r#"{
  "model_override": null,
  "reasoning_effort": null,
  "service_tier": null,
  "context_mode": null,
  "verbose": false,
  "plan_mode": false,
  "approval_policy_override": null,
  "pending_plan": null,
  "language": "en"
}"#,
            serde_json::to_string_pretty(&SessionSettings::default()).unwrap(),
        ),
        (
            r#"{
  "users": {},
  "imported_profiles": {},
  "cron_jobs": {}
}"#,
            serde_json::to_string_pretty(&PersistedSessionState::default()).unwrap(),
        ),
        (
            r#""per-invocation""#,
            serde_json::to_string_pretty(&SessionStrategy::default()).unwrap(),
        ),
        (
            r#"{
  "type": "push-to-owner"
}"#,
            serde_json::to_string_pretty(&DeliverPolicy::default()).unwrap(),
        ),
        (
            r#""medium""#,
            serde_json::to_string_pretty(&ReasoningEffort::default()).unwrap(),
        ),
        (
            r#""local""#,
            serde_json::to_string_pretty(&DialogOrigin::default()).unwrap(),
        ),
    ];
    for (expected, actual) in cases {
        assert_eq!(actual, expected);
    }
}

/// An empty document must still load: every field of `PersistedSessionState`
/// and every optional field the older on-disk layouts omitted is `default`ed.
#[test]
fn empty_documents_still_deserialize() {
    let state: PersistedSessionState = serde_json::from_str("{}").unwrap();
    assert_eq!(state, PersistedSessionState::default());

    let spec: InteractiveSpec = serde_json::from_str("{}").unwrap();
    assert_eq!(spec, InteractiveSpec::default());

    let settings: SessionSettings = serde_json::from_str(
        r#"{"model_override":null,"reasoning_effort":null,"service_tier":null,"context_mode":null}"#,
    )
    .unwrap();
    assert_eq!(settings, SessionSettings::default());
}
