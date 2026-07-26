use super::*;

pub(super) use crate::util::text::format_tokens_compact;

pub(super) const PROJECT_KEY_SEP: char = '\u{1f}';

#[derive(Debug, Clone)]
pub(super) struct ProjectBucket {
    path: String,
    count: usize,
    latest: Option<DateTime<Utc>>,
}

pub(super) fn build_status_text(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    is_busy: bool,
) -> String {
    let effective = state.effective_settings();
    let lang = state.settings.language.as_str();
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        t!(
            "commands.status.workspace",
            dir = state.foreground.workspace_dir.display().to_string(),
            locale = lang
        )
        .into_owned(),
    );
    lines.push(
        t!(
            "commands.status.model",
            summary = compact_runtime_summary(state, default_model, runtime_profile),
            locale = lang
        )
        .into_owned(),
    );
    lines.push(
        t!(
            if effective.verbose {
                "commands.status.verbose_on"
            } else {
                "commands.status.verbose_off"
            },
            locale = lang
        )
        .into_owned(),
    );
    lines.push(context_usage_line(state, runtime_profile, lang));
    if state.background.is_empty() {
        lines.push(t!("commands.status.bg_none", locale = lang).into_owned());
    } else {
        lines.push(
            t!(
                "commands.status.bg_header",
                count = state.background.len(),
                locale = lang
            )
            .into_owned(),
        );
        for alias in state.background.keys() {
            lines.push(format!("  - {alias}"));
        }
    }
    lines.push(
        t!(
            if is_busy {
                "commands.status.fg_busy"
            } else {
                "commands.status.fg_idle"
            },
            locale = lang
        )
        .into_owned(),
    );
    lines.push(t!("commands.status.lang", lang = lang, locale = lang).into_owned());
    lines.join("\n")
}

pub(super) fn context_usage_line(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
    lang: &str,
) -> String {
    let Some(usage) = state.foreground.last_usage.as_ref() else {
        return t!("commands.status.context_unknown", locale = lang).into_owned();
    };
    let Some(used_tokens) = usage.context_tokens() else {
        return t!("commands.status.context_unknown", locale = lang).into_owned();
    };
    let window = if usage.window > 0 {
        usage.window
    } else {
        effective_context_window(state, runtime_profile)
    };
    let percent = usage.percent_remaining().unwrap_or_else(|| {
        if window == 0 {
            0
        } else {
            100_u64.saturating_sub(
                ((usage.total_tokens as f64 / window as f64) * 100.0).round() as u64,
            )
        }
    });
    t!(
        "commands.status.context_usage",
        percent = percent,
        used = format_tokens_compact(used_tokens),
        total = format_tokens_compact(window),
        locale = lang
    )
    .into_owned()
}

pub(super) fn effective_context_window(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> u64 {
    match state
        .effective_settings()
        .context_mode
        .or(runtime_profile.context_mode)
        .unwrap_or(ContextMode::Standard)
    {
        ContextMode::Standard => ContextMode::STANDARD_CONTEXT_WINDOW,
        ContextMode::OneM => 1_000_000,
    }
}

pub(super) fn format_effective_runtime_text(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
    preview: Option<&str>,
) -> String {
    let lang = state.settings.language.as_str();
    let mut lines = Vec::new();
    if let Some(preview) = preview {
        lines.push(
            t!(
                "commands.runtime.last_user_message",
                preview = single_line(preview, 48),
                locale = lang
            )
            .into_owned(),
        );
    }
    lines.push(
        t!(
            "commands.runtime.model",
            summary = compact_runtime_summary(state, default_model, runtime_profile),
            locale = lang
        )
        .into_owned(),
    );
    lines.join("\n")
}

pub(super) fn compact_runtime_summary(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
) -> String {
    compact_model_summary(
        effective_model(state, default_model, runtime_profile),
        effective_reasoning(state, runtime_profile),
        effective_context_token(state, runtime_profile),
        effective_tier_token(state, runtime_profile),
    )
}

pub(super) fn compact_imported_profile_summary(
    profile: &crate::session::state::ImportedSessionProfile,
    lang: &str,
) -> String {
    let inherit_default = t!("commands.shared.inherit_default", locale = lang).into_owned();
    compact_model_summary(
        profile
            .model_override
            .clone()
            .unwrap_or_else(|| inherit_default.clone()),
        profile
            .reasoning_effort
            .map(|value| value.as_str())
            .unwrap_or(inherit_default.as_str()),
        profile.context_mode.map(|value| value.label().to_string()),
        profile.service_tier.map(|value| value.as_str().to_string()),
    )
}

pub(super) fn compact_model_summary(
    model: String,
    reasoning: &str,
    context: Option<String>,
    tier: Option<String>,
) -> String {
    let mut parts = vec![model, reasoning.to_string()];
    if let Some(context) = context {
        parts.push(context);
    }
    if let Some(tier) = tier {
        parts.push(tier);
    }
    parts.join(" ")
}

pub(super) fn effective_model(
    state: &UserSessionState,
    default_model: &str,
    runtime_profile: &CodexRuntimeProfile,
) -> String {
    let effective = state.effective_settings();
    effective
        .model_override
        .clone()
        .or_else(|| runtime_profile.configured_model.clone())
        .unwrap_or_else(|| default_model.to_string())
}

pub(super) fn effective_reasoning(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> &'static str {
    let effective = state.effective_settings();
    effective
        .reasoning_effort
        .or(runtime_profile.reasoning_effort)
        .unwrap_or(ReasoningEffort::Medium)
        .as_str()
}

pub(super) fn effective_tier_token(
    _state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> Option<String> {
    match runtime_profile.service_tier {
        Some(ServiceTier::Fast) => Some("fast".to_string()),
        Some(ServiceTier::Flex) => Some("flex".to_string()),
        None => None,
    }
}

pub(super) fn effective_context_label(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> &'static str {
    let effective = state.effective_settings();
    match effective.context_mode.or(runtime_profile.context_mode) {
        Some(mode) => mode.label(),
        None => "inherit",
    }
}

pub(super) fn effective_context_token(
    state: &UserSessionState,
    runtime_profile: &CodexRuntimeProfile,
) -> Option<String> {
    let effective = state.effective_settings();
    effective
        .context_mode
        .or(runtime_profile.context_mode)
        .map(|mode| mode.label().to_string())
}

pub(super) fn parse_scope(value: &str, lang: &str) -> Result<SessionListScope> {
    match value.to_ascii_lowercase().as_str() {
        "all" => Ok(SessionListScope::All),
        _ => Err(anyhow!(
            t!("commands.sessions.scope_invalid", locale = lang).into_owned()
        )),
    }
}

pub(super) fn is_scope_token(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "all")
}

pub(super) fn collect_projects(sessions: &[DiskSessionMeta]) -> Vec<ProjectBucket> {
    let mut map = BTreeMap::new();
    for session in sessions {
        let key = session.cwd.display().to_string();
        let entry = map.entry(key.clone()).or_insert(ProjectBucket {
            path: key,
            count: 0,
            latest: None,
        });
        entry.count += 1;
        if entry.latest < session.updated_at {
            entry.latest = session.updated_at;
        }
    }
    let mut values = map.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .latest
            .cmp(&left.latest)
            .then_with(|| left.path.cmp(&right.path))
    });
    values
}

/// The i18n keys that distinguish the `/sessions` list views from the
/// `/import` ones. The two key sets are structurally identical (verified in
/// `locales/en.yml` + `zh.yml`), so a single pair of formatters below renders
/// both flows.
pub(super) struct ListKeys {
    empty: &'static str,
    project_header: &'static str,
    project_row: &'static str,
    projects_footer: &'static str,
    project_empty: &'static str,
    page_out_of_range: &'static str,
    page_header: &'static str,
    row: &'static str,
    page_footer: &'static str,
}

pub(super) const SESSIONS_LIST_KEYS: ListKeys = ListKeys {
    empty: "commands.sessions.empty",
    project_header: "commands.sessions.project_header",
    project_row: "commands.sessions.project_row",
    projects_footer: "commands.sessions.projects_footer",
    project_empty: "commands.sessions.project_empty",
    page_out_of_range: "commands.sessions.page_out_of_range",
    page_header: "commands.sessions.page_header",
    row: "commands.sessions.row",
    page_footer: "commands.sessions.page_footer",
};

pub(super) const IMPORT_LIST_KEYS: ListKeys = ListKeys {
    empty: "commands.import.empty",
    project_header: "commands.import.project_header",
    project_row: "commands.import.project_row",
    projects_footer: "commands.import.projects_footer",
    project_empty: "commands.import.project_empty",
    page_out_of_range: "commands.import.page_out_of_range",
    page_header: "commands.import.page_header",
    row: "commands.import.row",
    page_footer: "commands.import.page_footer",
};

pub(super) fn format_projects_list(
    keys: &ListKeys,
    projects: &[ProjectBucket],
    lang: &str,
) -> (String, Vec<String>) {
    if projects.is_empty() {
        return (t!(keys.empty, locale = lang).into_owned(), Vec::new());
    }
    let mut lines =
        vec![t!(keys.project_header, count = projects.len(), locale = lang).into_owned()];
    let mut project_keys = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let latest = project
            .latest
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| t!("commands.shared.unknown", locale = lang).into_owned());
        lines.push(
            t!(
                keys.project_row,
                index = index + 1,
                path = project.path.as_str(),
                sessions = project.count,
                latest = latest,
                locale = lang
            )
            .into_owned(),
        );
        project_keys.push(encode_project_key(SessionListScope::All, &project.path));
    }
    lines.push(String::new());
    lines.push(t!(keys.projects_footer, locale = lang).into_owned());
    (lines.join("\n"), project_keys)
}

pub(super) fn format_project_sessions_page(
    keys: &ListKeys,
    project_path: &str,
    sessions: &[DiskSessionMeta],
    page: usize,
    lang: &str,
) -> (String, Vec<String>) {
    if sessions.is_empty() {
        return (
            t!(keys.project_empty, path = project_path, locale = lang).into_owned(),
            Vec::new(),
        );
    }
    let page_size = 12usize;
    let safe_page = page.max(1);
    let start = (safe_page - 1) * page_size;
    if start >= sessions.len() {
        return (
            t!(
                keys.page_out_of_range,
                path = project_path,
                total = sessions.len(),
                size = page_size,
                page = safe_page,
                locale = lang
            )
            .into_owned(),
            Vec::new(),
        );
    }
    let end = (start + page_size).min(sessions.len());
    let total_pages = sessions.len().div_ceil(page_size);
    let mut lines = vec![
        t!(
            keys.page_header,
            path = project_path,
            page = safe_page,
            total_pages = total_pages,
            total = sessions.len(),
            locale = lang
        )
        .into_owned(),
    ];
    let mut view_ids = Vec::new();
    for (offset, session) in sessions[start..end].iter().enumerate() {
        let index = offset + 1;
        let summary = session_summary(session, lang);
        let updated = session
            .updated_at
            .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| t!("commands.shared.unknown", locale = lang).into_owned());
        lines.push(
            t!(
                keys.row,
                index = index,
                updated = updated,
                summary = summary,
                locale = lang
            )
            .into_owned(),
        );
        view_ids.push(session.id.clone());
    }
    lines.push(String::new());
    lines.push(t!(keys.page_footer, locale = lang).into_owned());
    (lines.join("\n"), view_ids)
}

pub(super) fn session_summary(session: &DiskSessionMeta, lang: &str) -> String {
    let fallback = t!("commands.sessions.no_summary", locale = lang);
    let raw = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.as_ref());
    single_line(raw, 72)
}

pub(super) fn single_line(input: &str, max_chars: usize) -> String {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output = String::new();
    for (idx, ch) in normalized.chars().enumerate() {
        if idx >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

pub(super) fn scope_label(scope: SessionListScope) -> &'static str {
    match scope {
        SessionListScope::All => "all",
        SessionListScope::Local => "local",
        SessionListScope::Global => "global",
    }
}

pub(super) fn encode_project_key(scope: SessionListScope, path: &str) -> String {
    format!("{}{}{}", scope_label(scope), PROJECT_KEY_SEP, path)
}

pub(super) fn decode_project_key(key: &str) -> Result<(SessionListScope, String)> {
    if let Some((scope_raw, path)) = key.split_once(PROJECT_KEY_SEP) {
        return Ok((parse_scope(scope_raw, "en")?, path.to_string()));
    }
    Ok((SessionListScope::All, key.to_string()))
}

pub(super) fn resolve_project_selector(
    selector: &str,
    last_projects_view: &[String],
    lang: &str,
) -> Result<String> {
    if let Ok(index) = selector.parse::<usize>()
        && index >= 1
        && index <= last_projects_view.len()
    {
        return Ok(last_projects_view[index - 1].clone());
    }
    let normalized = selector.trim();
    if normalized.is_empty() {
        return Err(anyhow!(
            t!("commands.sessions.project_selector_empty", locale = lang).into_owned()
        ));
    }
    let matched = last_projects_view
        .iter()
        .filter(|key| {
            decode_project_key(key)
                .map(|(_, path)| path == normalized || path.starts_with(normalized))
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    match matched.as_slice() {
        [single] => Ok(single.clone()),
        [] => Err(anyhow!(
            t!(
                "commands.sessions.project_not_found",
                selector = selector,
                locale = lang
            )
            .into_owned()
        )),
        _ => Err(anyhow!(
            t!("commands.sessions.project_ambiguous", locale = lang).into_owned()
        )),
    }
}

pub(super) fn resolve_selector(
    selector: &str,
    sessions: &[DiskSessionMeta],
    last_view: &[String],
    lang: &str,
) -> Result<DiskSessionMeta> {
    if let Ok(index) = selector.parse::<usize>()
        && index >= 1
        && index <= last_view.len()
    {
        let target_id = &last_view[index - 1];
        if let Some(target) = sessions.iter().find(|value| value.id == *target_id) {
            return Ok(target.clone());
        }
    }
    if let Some(target) = sessions.iter().find(|value| value.id == selector) {
        return Ok(target.clone());
    }
    let matched = sessions
        .iter()
        .filter(|value| value.id.starts_with(selector))
        .cloned()
        .collect::<Vec<_>>();
    match matched.as_slice() {
        [single] => Ok(single.clone()),
        [] => Err(anyhow!(
            t!(
                "commands.sessions.session_not_found",
                selector = selector,
                locale = lang
            )
            .into_owned()
        )),
        _ => Err(anyhow!(
            t!("commands.sessions.session_ambiguous", locale = lang).into_owned()
        )),
    }
}

pub(super) fn help_text(lang: &str) -> String {
    let lang = normalize_lang(lang);
    let lines = vec![
        t!("commands.help.header", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_basic", locale = lang).into_owned(),
        t!("commands.help.entry_help", locale = lang).into_owned(),
        t!("commands.help.entry_status", locale = lang).into_owned(),
        t!("commands.help.entry_new", locale = lang).into_owned(),
        t!("commands.help.entry_stop", locale = lang).into_owned(),
        t!("commands.help.entry_interrupt", locale = lang).into_owned(),
        t!("commands.help.entry_lang", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_model_settings", locale = lang).into_owned(),
        t!("commands.help.entry_model", locale = lang).into_owned(),
        t!("commands.help.entry_reasoning", locale = lang).into_owned(),
        t!("commands.help.entry_fast", locale = lang).into_owned(),
        t!("commands.help.entry_context", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_approval_settings", locale = lang).into_owned(),
        t!("commands.help.entry_approvals", locale = lang).into_owned(),
        t!("commands.help.entry_plan", locale = lang).into_owned(),
        t!("commands.help.entry_cron", locale = lang).into_owned(),
        t!("commands.help.entry_execute_plan", locale = lang).into_owned(),
        t!("commands.help.entry_keep_planning", locale = lang).into_owned(),
        t!("commands.help.entry_cancel_plan", locale = lang).into_owned(),
        t!("commands.help.entry_approve", locale = lang).into_owned(),
        t!("commands.help.entry_approve_session", locale = lang).into_owned(),
        t!("commands.help.entry_deny", locale = lang).into_owned(),
        t!("commands.help.entry_cancel", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_session_management", locale = lang).into_owned(),
        t!("commands.help.entry_sessions", locale = lang).into_owned(),
        t!("commands.help.entry_import", locale = lang).into_owned(),
        t!("commands.help.entry_resume", locale = lang).into_owned(),
        t!("commands.help.entry_save", locale = lang).into_owned(),
        String::new(),
        t!("commands.help.section_advanced", locale = lang).into_owned(),
        t!("commands.help.entry_compact", locale = lang).into_owned(),
        t!("commands.help.entry_fg", locale = lang).into_owned(),
        t!("commands.help.entry_bg", locale = lang).into_owned(),
        t!("commands.help.entry_loadbg", locale = lang).into_owned(),
        t!("commands.help.entry_rename", locale = lang).into_owned(),
        t!("commands.help.entry_alias", locale = lang).into_owned(),
        t!("commands.help.entry_verbose", locale = lang).into_owned(),
        t!("commands.help.entry_self_update", locale = lang).into_owned(),
    ];
    lines.join("\n")
}
