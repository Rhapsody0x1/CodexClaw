//! Codex rollout disk access: scanning a codex home's `sessions/` tree,
//! parsing rollout `.jsonl` files and the `session_index.jsonl` sidecar, and
//! copying/pruning rollout files between homes. Everything here is pure disk
//! I/O over the rollout wire format; the in-memory session state stays in
//! [`super::store`].

use std::{
    collections::{BTreeMap, HashMap},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};

use crate::session::state::{
    ContextMode, DialogOrigin, DialogState, ImportedSessionProfile, PersistedSessionState,
    ReasoningEffort, ServiceTier,
};
use crate::session::store::DiskSessionMeta;
use crate::util::{fs::walk_files, time::parse_utc};

/// The rollout format is untyped JSON; this is the ubiquitous
/// `.get(key).and_then(|item| item.as_str())` field access, shared between
/// `serde_json::Value` and `serde_json::Map` lookups.
fn as_str_field(value: Option<&serde_json::Value>) -> Option<&str> {
    value.and_then(|item| item.as_str())
}

pub(super) fn prune_session_files(codex_home: &Path, session_id: &str) -> Result<()> {
    walk_files(&codex_home.join("sessions"), |path, name| {
        if name.ends_with(".jsonl") && name.contains(session_id) {
            let _ = std::fs::remove_file(path);
        }
    })
}

pub(super) fn scan_home_sessions(codex_home: &Path) -> Result<Vec<DiskSessionMeta>> {
    let index = read_session_index(codex_home)?;
    let mut files = Vec::new();
    collect_rollout_files(&codex_home.join("sessions"), &mut files)?;
    let mut sessions = Vec::new();
    for path in files {
        if let Some(meta) = parse_rollout_meta(&path, &index)? {
            sessions.push(meta);
        }
    }
    Ok(sessions)
}

fn collect_rollout_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    walk_files(root, |path, name| {
        if name.starts_with("rollout-") && name.ends_with(".jsonl") {
            out.push(path.to_path_buf());
        }
    })
}

#[derive(Debug, Clone)]
struct IndexEntry {
    thread_name: Option<String>,
    first_user_message: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

fn read_session_index(codex_home: &Path) -> Result<HashMap<String, IndexEntry>> {
    let path = codex_home.join("session_index.jsonl");
    let Ok(file) = File::open(&path) else {
        return Ok(HashMap::new());
    };
    let reader = BufReader::new(file);
    let mut map = HashMap::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let id = as_str_field(value.get("id")).map(str::to_string);
        let Some(id) = id else {
            continue;
        };
        let thread_name = as_str_field(value.get("thread_name"))
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty());
        let first_user_message = as_str_field(value.get("first_user_message"))
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty());
        let updated_at = as_str_field(value.get("updated_at")).and_then(parse_utc);
        map.insert(
            id,
            IndexEntry {
                thread_name,
                first_user_message,
                updated_at,
            },
        );
    }
    Ok(map)
}

fn parse_rollout_meta(
    path: &Path,
    index: &HashMap<String, IndexEntry>,
) -> Result<Option<DiskSessionMeta>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line)? == 0 {
        return Ok(None);
    }
    let value = match serde_json::from_str::<serde_json::Value>(&first_line) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if as_str_field(value.get("type")) != Some("session_meta") {
        return Ok(None);
    }
    let payload = value.get("payload").and_then(|item| item.as_object());
    let Some(payload) = payload else {
        return Ok(None);
    };
    let id = as_str_field(payload.get("id"))
        .ok_or_else(|| anyhow!("session meta missing id in {}", path.display()))?
        .to_string();
    let cwd = as_str_field(payload.get("cwd"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let created_at = as_str_field(payload.get("timestamp")).and_then(parse_utc);
    let index_entry = index.get(&id);
    let updated_at = index_entry
        .and_then(|entry| entry.updated_at)
        .or(created_at);
    let last_user_message = parse_last_user_message(&mut reader);
    let title = index_entry
        .and_then(|entry| entry.thread_name.clone())
        .or_else(|| last_user_message.clone())
        .or_else(|| {
            index_entry
                .and_then(|entry| entry.first_user_message.as_deref())
                .and_then(extract_user_message_preview)
        });
    Ok(Some(DiskSessionMeta {
        id,
        cwd,
        title,
        last_user_message,
        updated_at,
        origin: DialogOrigin::Global,
        rollout_path: path.to_path_buf(),
    }))
}

pub(super) fn insert_prefer_recent(
    map: &mut BTreeMap<String, DiskSessionMeta>,
    candidate: DiskSessionMeta,
) {
    match map.get(&candidate.id) {
        Some(existing) if existing.updated_at >= candidate.updated_at => {}
        _ => {
            map.insert(candidate.id.clone(), candidate);
        }
    }
}

fn parse_last_user_message(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    let mut last_message = None;
    loop {
        line.clear();
        let size = reader.read_line(&mut line).ok()?;
        if size == 0 {
            return last_message;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if as_str_field(value.get("type")) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if as_str_field(payload.get("type")) != Some("message") {
            continue;
        }
        if as_str_field(payload.get("role")) != Some("user") {
            continue;
        }
        let Some(content) = payload.get("content").and_then(|item| item.as_array()) else {
            continue;
        };
        for item in content {
            let Some(text) = as_str_field(item.get("text")) else {
                continue;
            };
            if let Some(message) = extract_user_message_preview(text) {
                last_message = Some(message);
            }
        }
    }
}

/// `role=user` texts codex or CodexClaw injects that are not the human
/// talking. A survey of real rollouts found each of these leaking into the
/// `/sessions` preview as the "last user message"; anything matching is
/// skipped so the scan falls back to the previous genuine message.
const INJECTED_USER_TEXT_MARKERS: &[&str] = &[
    "<environment_context>",
    "<turn_aborted>",
    "<codex_internal_context",
    "<subagent_notification>",
    "<user_shell_command>",
    "<image",
    "# AGENTS.md instructions",
    ">>> APPROVAL REQUEST",
    "[CLAW SCHEDULED",
];

fn extract_user_message_preview(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let message = if let Some((_, tail)) = trimmed.rsplit_once("User message:\n") {
        tail.trim()
    } else {
        trimmed
    };
    if message.is_empty()
        || message == "(User sent no text, only attachments.)"
        || INJECTED_USER_TEXT_MARKERS
            .iter()
            .any(|marker| message.starts_with(marker))
    {
        return None;
    }
    Some(message.to_string())
}

pub(super) fn copy_session_rollout(
    source_home: &Path,
    destination_home: &Path,
    source_rollout_path: &Path,
) -> Result<bool> {
    let source_root = source_home.join("sessions");
    let rel = source_rollout_path
        .strip_prefix(&source_root)
        .with_context(|| {
            format!(
                "session rollout {} is not under {}",
                source_rollout_path.display(),
                source_root.display()
            )
        })?;
    let destination = destination_home.join("sessions").join(rel);
    if destination.exists() {
        return Ok(false);
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::copy(source_rollout_path, &destination).with_context(|| {
        format!(
            "failed to copy session rollout from {} to {}",
            source_rollout_path.display(),
            destination.display()
        )
    })?;
    Ok(true)
}

pub(super) fn copy_session_index_entry(
    source_home: &Path,
    destination_home: &Path,
    session_id: &str,
) -> Result<()> {
    let source_path = source_home.join("session_index.jsonl");
    let Ok(source_raw) = std::fs::read_to_string(&source_path) else {
        return Ok(());
    };
    let Some(line) = source_raw
        .lines()
        .find(|value| value.contains(&format!("\"id\":\"{session_id}\"")))
    else {
        return Ok(());
    };

    let destination_path = destination_home.join("session_index.jsonl");
    if let Ok(existing) = std::fs::read_to_string(&destination_path)
        && existing
            .lines()
            .any(|value| value.contains(&format!("\"id\":\"{session_id}\"")))
    {
        return Ok(());
    }
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&destination_path)
        .with_context(|| format!("failed to open {}", destination_path.display()))?;
    use std::io::Write;
    writeln!(file, "{line}")
        .with_context(|| format!("failed to append {}", destination_path.display()))?;
    Ok(())
}

/// Applies one `turn_context` payload to the profile, returning whether any
/// field was actually set (the caller's "this rollout carries a profile"
/// signal).
fn apply_turn_context(
    profile: &mut ImportedSessionProfile,
    payload: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let mut seen = false;
    if let Some(cwd) = as_str_field(payload.get("cwd")) {
        profile.workspace_dir = PathBuf::from(cwd);
        seen = true;
    }
    if let Some(model) = as_str_field(payload.get("model")) {
        let model = model.trim();
        if !model.is_empty() {
            profile.model_override = Some(model.to_string());
            seen = true;
        }
    }
    if let Some(parsed) = as_str_field(payload.get("effort")).and_then(ReasoningEffort::parse) {
        profile.reasoning_effort = Some(parsed);
        seen = true;
    }
    if let Some(parsed) = as_str_field(payload.get("service_tier")).and_then(ServiceTier::parse) {
        profile.service_tier = Some(parsed);
        seen = true;
    }
    seen
}

/// Applies one `event_msg` payload if it is a `token_count` carrying a model
/// context window, returning whether the context mode was set.
fn apply_token_count(
    profile: &mut ImportedSessionProfile,
    payload: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if as_str_field(payload.get("type")) != Some("token_count") {
        return false;
    }
    let Some(window) = payload
        .get("info")
        .and_then(|item| item.get("model_context_window"))
        .and_then(|item| item.as_u64())
    else {
        return false;
    };
    profile.context_mode = Some(ContextMode::from_model_context_window(window));
    true
}

pub(super) fn extract_session_profile(
    rollout_path: &Path,
    fallback_workspace: PathBuf,
) -> Result<Option<ImportedSessionProfile>> {
    let file = File::open(rollout_path)
        .with_context(|| format!("failed to open {}", rollout_path.display()))?;
    let reader = BufReader::new(file);
    let mut profile = ImportedSessionProfile {
        workspace_dir: fallback_workspace,
        ..ImportedSessionProfile::default()
    };
    let mut seen = false;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(item_type) = as_str_field(value.get("type")) else {
            continue;
        };
        match item_type {
            "turn_context" => {
                let Some(payload) = value.get("payload").and_then(|item| item.as_object()) else {
                    continue;
                };
                seen |= apply_turn_context(&mut profile, payload);
            }
            "event_msg" => {
                let Some(payload) = value.get("payload").and_then(|item| item.as_object()) else {
                    continue;
                };
                seen |= apply_token_count(&mut profile, payload);
            }
            _ => {}
        }
    }

    Ok(if seen { Some(profile) } else { None })
}

/// Caches a freshly resolved profile for `session_id`, keeping an existing
/// cache entry if one appeared in the meantime (`or_insert` semantics).
pub(super) fn cache_imported_profile(
    state: &mut PersistedSessionState,
    session_id: &str,
    profile: Option<&ImportedSessionProfile>,
) {
    if let Some(profile) = profile {
        state
            .imported_profiles
            .entry(session_id.to_string())
            .or_insert_with(|| profile.clone());
    }
}

/// Builds the dialog record for a session picked from disk, preferring the
/// resolved profile's workspace over the rollout `cwd`.
pub(super) fn dialog_from_disk_session(
    target: &DiskSessionMeta,
    profile: Option<&ImportedSessionProfile>,
) -> DialogState {
    DialogState {
        session_id: Some(target.id.clone()),
        origin: target.origin,
        workspace_dir: profile
            .map(|value| value.workspace_dir.clone())
            .unwrap_or_else(|| target.cwd.clone()),
        saved: true,
        profile: profile.map(|value| value.dialog_profile()),
        last_usage: None,
        // Meaningful only while installed as the foreground; the installer
        // (`install_foreground`) assigns the real value.
        generation: 0,
    }
}
