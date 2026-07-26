//! Helpers backing the multi-step `/model`, `/reasoning`, `/fast`,
//! `/context`, `/verbose`, `/lang`, `/sessions`, `/import`, `/fg`,
//! `/resume` and `/loadbg` pickers. These functions are invoked both
//! when the user enters an interactive command with no arguments
//! (`enter_*_prompt`) and when the user's next message is consumed while
//! the pending state is active (`consume_pending_input`).
use super::*;

struct ReasoningChoice {
    value: &'static str,
    aliases: &'static [&'static str],
}

const REASONING_CHOICES: &[ReasoningChoice] = &[
    ReasoningChoice {
        value: "low",
        aliases: &["低"],
    },
    ReasoningChoice {
        value: "medium",
        aliases: &["中"],
    },
    ReasoningChoice {
        value: "high",
        aliases: &["高"],
    },
    ReasoningChoice {
        value: "xhigh",
        aliases: &["超高"],
    },
];

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FuzzyOutcome {
    Exact(String),
    Ambiguous(Vec<String>),
    None,
}

/// Case-insensitive exact / substring match with a uniqueness requirement.
pub(super) fn fuzzy_match_unique(input: &str, candidates: &[String]) -> FuzzyOutcome {
    let needle = input.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return FuzzyOutcome::None;
    }
    for candidate in candidates {
        if candidate.to_ascii_lowercase() == needle {
            return FuzzyOutcome::Exact(candidate.clone());
        }
    }
    let matches: Vec<String> = candidates
        .iter()
        .filter(|c| c.to_ascii_lowercase().contains(&needle))
        .cloned()
        .collect();
    match matches.len() {
        0 => FuzzyOutcome::None,
        1 => FuzzyOutcome::Exact(matches.into_iter().next().unwrap()),
        _ => FuzzyOutcome::Ambiguous(matches),
    }
}

fn dedupe_matches(values: Vec<String>) -> Vec<String> {
    values.into_iter().fold(Vec::new(), |mut acc, value| {
        if !acc.iter().any(|existing| existing == &value) {
            acc.push(value);
        }
        acc
    })
}

fn model_matches_exact(entry: &CodexModelEntry, needle: &str) -> bool {
    entry.name.eq_ignore_ascii_case(needle)
        || entry
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(needle))
}

fn model_matches_prefix(entry: &CodexModelEntry, needle: &str) -> bool {
    let needle = needle.to_ascii_lowercase();
    entry.name.to_ascii_lowercase().starts_with(&needle)
        || entry
            .aliases
            .iter()
            .any(|alias| alias.to_ascii_lowercase().starts_with(&needle))
}

fn match_model_input(input: &str, models: &[CodexModelEntry]) -> FuzzyOutcome {
    let needle = input.trim();
    if needle.is_empty() {
        return FuzzyOutcome::None;
    }

    let exact = dedupe_matches(
        models
            .iter()
            .filter(|entry| model_matches_exact(entry, needle))
            .map(|entry| entry.name.clone())
            .collect(),
    );
    match exact.len() {
        1 => return FuzzyOutcome::Exact(exact.into_iter().next().unwrap()),
        value if value > 1 => return FuzzyOutcome::Ambiguous(exact),
        _ => {}
    }

    let prefixes = dedupe_matches(
        models
            .iter()
            .filter(|entry| model_matches_prefix(entry, needle))
            .map(|entry| entry.name.clone())
            .collect(),
    );
    match prefixes.len() {
        0 => FuzzyOutcome::None,
        1 => FuzzyOutcome::Exact(prefixes.into_iter().next().unwrap()),
        _ => FuzzyOutcome::Ambiguous(prefixes),
    }
}

pub(super) fn resolve_model_input(input: &str, models: &[CodexModelEntry]) -> Option<String> {
    match match_model_input(input, models) {
        FuzzyOutcome::Exact(choice) => Some(choice),
        FuzzyOutcome::Ambiguous(_) | FuzzyOutcome::None => None,
    }
}

fn choice_matches_exact(value: &str, aliases: &[&str], needle: &str) -> bool {
    value.eq_ignore_ascii_case(needle)
        || aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(needle))
}

fn match_reasoning_input(input: &str) -> FuzzyOutcome {
    let needle = input.trim();
    if needle.is_empty() {
        return FuzzyOutcome::None;
    }

    if needle.eq_ignore_ascii_case("inherit")
        || needle.eq_ignore_ascii_case("default")
        || needle == "继承"
        || needle == "默认"
    {
        return FuzzyOutcome::Exact("inherit".to_string());
    }

    let exact = dedupe_matches(
        REASONING_CHOICES
            .iter()
            .filter(|choice| choice_matches_exact(choice.value, choice.aliases, needle))
            .map(|choice| choice.value.to_string())
            .collect(),
    );
    match exact.len() {
        0 => FuzzyOutcome::None,
        1 => FuzzyOutcome::Exact(exact.into_iter().next().unwrap()),
        _ => FuzzyOutcome::Ambiguous(exact),
    }
}

pub(super) fn resolve_reasoning_input(input: &str) -> Option<Option<ReasoningEffort>> {
    match match_reasoning_input(input) {
        FuzzyOutcome::Exact(choice) if choice == "inherit" => Some(None),
        FuzzyOutcome::Exact(choice) => ReasoningEffort::parse_supported(&choice).map(Some),
        FuzzyOutcome::Ambiguous(_) | FuzzyOutcome::None => None,
    }
}

pub(super) fn resolve_fast_input(input: &str) -> Option<Option<ServiceTier>> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    match value.to_ascii_lowercase().as_str() {
        "inherit" | "default" => Some(None),
        "on" => Some(Some(ServiceTier::Fast)),
        "off" => Some(Some(ServiceTier::Flex)),
        _ => match value {
            "默认" => Some(None),
            "开" => Some(Some(ServiceTier::Fast)),
            "关" => Some(Some(ServiceTier::Flex)),
            _ => None,
        },
    }
}

pub(super) fn resolve_context_input(input: &str) -> Option<Option<ContextMode>> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    match value.to_ascii_lowercase().as_str() {
        "inherit" | "default" => Some(None),
        "standard" | "272k" => Some(Some(ContextMode::Standard)),
        "1m" => Some(Some(ContextMode::OneM)),
        _ => match value {
            "默认" => Some(None),
            "标准" => Some(Some(ContextMode::Standard)),
            "长" => Some(Some(ContextMode::OneM)),
            _ => None,
        },
    }
}

fn verbose_options() -> &'static [&'static str] {
    &["on", "off"]
}

fn lang_options() -> &'static [&'static str] {
    &["en", "zh"]
}

fn as_string_vec(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

fn hint(locale: &str) -> String {
    t!("commands.interactive.hint", locale = locale).into_owned()
}

fn paragraph_hint_block(locale: &str) -> Vec<String> {
    vec![String::new(), hint(locale)]
}

fn format_markdown_aliases(aliases: &[String], locale: &str) -> String {
    let separator = if locale.eq_ignore_ascii_case("zh") {
        "、"
    } else {
        ", "
    };
    aliases
        .iter()
        .map(|alias| format!("*{alias}*"))
        .collect::<Vec<_>>()
        .join(separator)
}

fn format_model_prompt_item(entry: &CodexModelEntry, locale: &str) -> Vec<String> {
    let mut lines = vec![
        t!(
            "commands.model.prompt_item_name",
            name = entry.name.as_str(),
            locale = locale
        )
        .into_owned(),
    ];
    if let Some(description) = entry.description_for_locale(locale) {
        lines.push(
            t!(
                "commands.model.prompt_item_description",
                description = description,
                locale = locale
            )
            .into_owned(),
        );
    }
    if !entry.aliases.is_empty() {
        lines.push(
            t!(
                "commands.model.prompt_item_aliases",
                aliases = format_markdown_aliases(&entry.aliases, locale),
                locale = locale
            )
            .into_owned(),
        );
    }
    lines
}

fn join_prompt_blocks(blocks: Vec<Vec<String>>) -> String {
    blocks
        .into_iter()
        .map(|block| block.join("\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ambiguous_reply(locale: &str, input: &str, matches: &[String]) -> String {
    t!(
        "commands.interactive.ambiguous",
        input = input,
        matches = matches.join(", "),
        locale = locale
    )
    .into_owned()
}

fn no_match_reply(locale: &str, input: &str) -> String {
    t!(
        "commands.interactive.no_match",
        input = input,
        locale = locale
    )
    .into_owned()
}

/// Renders the shared `Ambiguous` / `None` fallback replies of the fuzzy
/// pickers. Callers handle the `Exact` arm before delegating here.
fn fuzzy_fallback(outcome: FuzzyOutcome, locale: &str, input: &str) -> CommandOutcome {
    match outcome {
        FuzzyOutcome::Ambiguous(matches) => {
            CommandOutcome::reply(ambiguous_reply(locale, input, &matches))
        }
        FuzzyOutcome::Exact(_) | FuzzyOutcome::None => {
            CommandOutcome::reply(no_match_reply(locale, input))
        }
    }
}

pub(super) fn model_extras(snapshot: &UserSessionState) -> Vec<String> {
    snapshot
        .effective_settings()
        .model_override
        .map(|value| vec![value])
        .unwrap_or_default()
}

// ---- Entry points (command with no args) -------------------------------

pub(super) async fn enter_model_prompt(
    snapshot: &UserSessionState,
    ctx: CmdCtx<'_>,
    locale: &str,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        ..
    } = ctx;
    let models = list_codex_model_entries(runtime_profile, &model_extras(snapshot));
    let current = effective_model(snapshot, default_model, runtime_profile);
    let mut sections = vec![vec![
        t!(
            "commands.model.prompt_current",
            current = format!("`{}`", current),
            locale = locale
        )
        .into_owned(),
        t!("commands.model.prompt_header", locale = locale).into_owned(),
    ]];
    for entry in &models {
        sections.push(format_model_prompt_item(entry, locale));
    }
    sections.push(paragraph_hint_block(locale));
    session
        .set_pending_setting(openid, Some(PendingSetting::Model))
        .await?;
    Ok(CommandOutcome::reply(join_prompt_blocks(sections)))
}

/// Shared skeleton for the single-value prompts (`/reasoning`, `/fast`,
/// `/context`, `/verbose`, `/lang`): a current-value line, an options
/// header, the input hint, then park the matching pending state.
pub(super) async fn enter_simple_prompt(
    ctx: CmdCtx<'_>,
    locale: &str,
    current_key: &'static str,
    header_key: &'static str,
    current: &str,
    pending: PendingSetting,
) -> Result<CommandOutcome> {
    let text = format!(
        "{}\n{}\n{}",
        t!(current_key, current = current, locale = locale),
        t!(header_key, locale = locale),
        hint(locale),
    );
    ctx.session
        .set_pending_setting(ctx.openid, Some(pending))
        .await?;
    Ok(CommandOutcome::reply(text))
}

pub(super) async fn enter_fg_prompt(
    snapshot: &UserSessionState,
    ctx: CmdCtx<'_>,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let locale = snapshot.settings.language.as_str();
    if snapshot.background.is_empty() {
        return Ok(CommandOutcome::reply_t("commands.fg.prompt_empty", locale));
    }
    let mut lines = vec![t!("commands.fg.prompt_header", locale = locale).into_owned()];
    for alias in snapshot.background.keys() {
        lines.push(
            t!(
                "commands.fg.prompt_item",
                alias = alias.as_str(),
                locale = locale
            )
            .into_owned(),
        );
    }
    lines.push(String::new());
    lines.push(hint(locale));
    session
        .set_pending_setting(openid, Some(PendingSetting::Fg))
        .await?;
    Ok(CommandOutcome::reply(lines.join("\n")))
}

pub(super) async fn enter_restore_projects_prompt(
    mode: RestoreMode,
    ctx: CmdCtx<'_>,
    locale: &str,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let sessions = session.list_disk_sessions(SessionListScope::All).await?;
    let projects = collect_projects(&sessions);
    let (text, project_keys) = format_projects_list(&SESSIONS_LIST_KEYS, &projects, locale);
    let has_projects = !project_keys.is_empty();
    session.set_last_projects_view(openid, project_keys).await?;
    session.set_last_sessions_view(openid, Vec::new()).await?;
    if has_projects {
        session
            .set_pending_setting(openid, Some(mode.projects_pending()))
            .await?;
    }
    Ok(CommandOutcome::reply(text))
}

pub(super) async fn enter_restore_sessions_prompt(
    mode: RestoreMode,
    ctx: CmdCtx<'_>,
    project_key: String,
    page: usize,
    alias: Option<String>,
    locale: &str,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let (scope, project_path) = decode_project_key(&project_key)?;
    let all_sessions = session.list_disk_sessions(scope).await?;
    let project_sessions = all_sessions
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text, ids) = format_project_sessions_page(
        &SESSIONS_LIST_KEYS,
        &project_path,
        &project_sessions,
        page,
        locale,
    );
    session.set_last_sessions_view(openid, ids).await?;
    session
        .set_pending_setting(
            openid,
            Some(mode.sessions_pending(project_key, page, alias)),
        )
        .await?;
    Ok(CommandOutcome::reply(text))
}

// ---- Actions -----------------------------------------------------------

pub(super) async fn switch_foreground(
    alias: &str,
    ctx: CmdCtx<'_>,
    locale: &str,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        ..
    } = ctx;
    let switched = session.foreground_from_background(openid, alias).await?;
    let snapshot = session.snapshot_for_user(openid).await?;
    let preview = foreground_last_user_message(session, &snapshot).await?;
    let parked = switched
        .parked_alias
        .map(|value| {
            t!(
                "commands.fg.parked",
                alias = value.as_str(),
                locale = locale
            )
            .into_owned()
        })
        .unwrap_or_default();
    session.set_pending_setting(openid, None).await?;
    Ok(CommandOutcome::reply(t!(
        "commands.fg.switched",
        parked = parked,
        alias = alias,
        runtime = format_effective_runtime_text(
            &snapshot,
            default_model,
            runtime_profile,
            preview.as_deref()
        ),
        locale = locale
    )))
}

pub(super) async fn execute_resume(
    ctx: CmdCtx<'_>,
    target: &DiskSessionMeta,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let switched = session.resume_disk_session(openid, target).await?;
    let imported_profile = session.imported_profile_for_session(&target.id).await?;
    let workspace_display = imported_profile
        .as_ref()
        .map(|value| value.workspace_dir.display().to_string())
        .unwrap_or_else(|| target.cwd.display().to_string());
    let snapshot = session.snapshot_for_user(openid).await?;
    let parked = switched
        .parked_alias
        .map(|value| {
            t!(
                "commands.resume.parked",
                alias = value.as_str(),
                locale = lang.as_str()
            )
            .into_owned()
        })
        .unwrap_or_default();
    session.set_pending_setting(openid, None).await?;
    Ok(CommandOutcome::reply(t!(
        "commands.resume.restored",
        parked = parked,
        summary = session_summary(target, lang.as_str()),
        workspace = workspace_display,
        runtime = format_effective_runtime_text(
            &snapshot,
            default_model,
            runtime_profile,
            target.last_user_message.as_deref(),
        ),
        locale = lang.as_str()
    )))
}

pub(super) async fn execute_loadbg(
    ctx: CmdCtx<'_>,
    target: &DiskSessionMeta,
    alias: Option<&str>,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let lang = user_locale(session, openid).await;
    let new_alias = session
        .load_disk_session_to_background(openid, target, alias)
        .await?;
    session.set_pending_setting(openid, None).await?;
    Ok(CommandOutcome::reply(t!(
        "commands.loadbg.loaded",
        alias = new_alias.as_str(),
        summary = session_summary(target, lang.as_str()),
        locale = lang.as_str()
    )))
}

// ---- Pending-input consumption ----------------------------------------

pub(super) async fn consume_pending_input(
    pending: PendingSetting,
    text: &str,
    ctx: CmdCtx<'_>,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        is_busy,
        ..
    } = ctx;
    if is_busy
        && matches!(
            &pending,
            PendingSetting::Model | PendingSetting::Reasoning | PendingSetting::Context
        )
    {
        let locale = user_locale(session, openid).await;
        return Ok(super::busy_reply(locale.as_str()));
    }
    match pending {
        PendingSetting::Model => consume_model(text, ctx).await,
        PendingSetting::Reasoning => consume_reasoning(text, ctx).await,
        PendingSetting::Fast => consume_fast(text, ctx).await,
        PendingSetting::Context => consume_context(text, ctx).await,
        PendingSetting::Verbose => consume_verbose(text, ctx).await,
        PendingSetting::Lang => consume_lang(text, ctx).await,
        PendingSetting::SessionsProjects => consume_sessions_projects(text, ctx).await,
        PendingSetting::SessionsSessions { project_key, page } => {
            consume_sessions_sessions(text, ctx, project_key, page).await
        }
        PendingSetting::ImportProjects => consume_import_projects(text, ctx).await,
        PendingSetting::ImportSessions { project_key, page } => {
            consume_import_sessions(text, ctx, project_key, page).await
        }
        PendingSetting::Fg => consume_fg(text, ctx).await,
        PendingSetting::ResumeProjects => {
            consume_restore_projects(RestoreMode::Resume, text, ctx).await
        }
        PendingSetting::ResumeSessions { project_key, page } => {
            consume_restore_sessions(RestoreMode::Resume, text, ctx, project_key, page, None).await
        }
        PendingSetting::LoadbgProjects => {
            consume_restore_projects(RestoreMode::Loadbg, text, ctx).await
        }
        PendingSetting::LoadbgSessions {
            project_key,
            page,
            alias,
        } => {
            consume_restore_sessions(RestoreMode::Loadbg, text, ctx, project_key, page, alias).await
        }
        PendingSetting::Approvals => consume_approvals(text, ctx).await,
        PendingSetting::Plan => consume_plan(text, ctx).await,
        PendingSetting::ResumeRecovery => Ok(CommandOutcome::reply_t(
            "commands.resume.recovery_prompt",
            user_locale(session, openid).await.as_str(),
        )),
    }
}

async fn consume_approvals(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    super::handle_approvals_arg(text, ctx.openid, ctx.session).await
}

async fn consume_plan(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    super::handle_plan_arg(text, ctx.openid, ctx.session).await
}

async fn consume_model(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        default_model,
        runtime_profile,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let locale = snapshot.settings.language.clone();
    let locale = locale.as_str();
    let input = text.trim();
    let models = list_codex_model_entries(runtime_profile, &model_extras(&snapshot));
    match if input.eq_ignore_ascii_case("inherit") || input.eq_ignore_ascii_case("default") {
        FuzzyOutcome::Exact("inherit".to_string())
    } else {
        match_model_input(input, &models)
    } {
        FuzzyOutcome::Exact(choice) => {
            let next = if choice.eq_ignore_ascii_case("inherit") {
                None
            } else {
                Some(choice.clone())
            };
            super::apply_active_or_global(
                ctx,
                &snapshot,
                true,
                next,
                CommandOutcome::SetGlobalModel,
                |value| session.set_model_override_for_active(openid, value),
                |snap: &UserSessionState| {
                    t!(
                        "commands.model.updated",
                        model = effective_model(snap, default_model, runtime_profile),
                        locale = locale
                    )
                    .into_owned()
                },
            )
            .await
        }
        other => Ok(fuzzy_fallback(other, locale, input)),
    }
}

async fn consume_reasoning(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        runtime_profile,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let locale = snapshot.settings.language.clone();
    let locale = locale.as_str();
    let input = text.trim();
    match match_reasoning_input(input) {
        FuzzyOutcome::Exact(choice) => {
            let next = if choice == "inherit" {
                None
            } else {
                ReasoningEffort::parse_supported(&choice)
            };
            super::apply_active_or_global(
                ctx,
                &snapshot,
                true,
                next,
                CommandOutcome::SetGlobalReasoning,
                |value| session.set_reasoning_for_active(openid, value),
                |snap: &UserSessionState| {
                    t!(
                        "commands.reasoning.updated",
                        value = effective_reasoning(snap, runtime_profile),
                        locale = locale
                    )
                    .into_owned()
                },
            )
            .await
        }
        other => Ok(fuzzy_fallback(other, locale, input)),
    }
}

async fn consume_fast(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let locale = snapshot.settings.language.clone();
    let locale = locale.as_str();
    let input = text.trim();
    match resolve_fast_input(input) {
        Some(next) => {
            session.set_pending_setting(openid, None).await?;
            Ok(CommandOutcome::SetGlobalFast(next))
        }
        None => Ok(CommandOutcome::reply(no_match_reply(locale, input))),
    }
}

async fn consume_context(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid,
        session,
        runtime_profile,
        ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let locale = snapshot.settings.language.clone();
    let locale = locale.as_str();
    let input = text.trim();
    match resolve_context_input(input) {
        Some(next) => {
            super::apply_active_or_global(
                ctx,
                &snapshot,
                true,
                next,
                CommandOutcome::SetGlobalContext,
                |value| session.set_context_mode_for_active(openid, value),
                |snap: &UserSessionState| {
                    t!(
                        "commands.context.updated",
                        value = effective_context_label(snap, runtime_profile),
                        locale = locale
                    )
                    .into_owned()
                },
            )
            .await
        }
        None => Ok(CommandOutcome::reply(no_match_reply(locale, input))),
    }
}

async fn consume_verbose(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let locale = snapshot.settings.language.clone();
    let locale = locale.as_str();
    let candidates = as_string_vec(verbose_options());
    let input = text.trim();
    match fuzzy_match_unique(input, &candidates) {
        FuzzyOutcome::Exact(choice) => {
            let enabled = choice == "on";
            session
                .update_settings_for_user(openid, |state| state.verbose = enabled)
                .await?;
            session.set_pending_setting(openid, None).await?;
            let key = if enabled {
                "commands.verbose.updated_on"
            } else {
                "commands.verbose.updated_off"
            };
            Ok(CommandOutcome::reply_t(key, locale))
        }
        other => Ok(fuzzy_fallback(other, locale, input)),
    }
}

async fn consume_lang(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let current_locale = snapshot.settings.language.clone();
    let candidates = as_string_vec(lang_options());
    let input = text.trim();
    match fuzzy_match_unique(input, &candidates) {
        FuzzyOutcome::Exact(choice) => {
            let normalized = normalize_lang(&choice);
            session
                .update_settings_for_user(openid, |state| {
                    state.language = normalized.to_string();
                })
                .await?;
            session.set_pending_setting(openid, None).await?;
            Ok(CommandOutcome::reply(t!(
                "commands.lang.updated",
                lang = normalized,
                locale = normalized
            )))
        }
        other => Ok(fuzzy_fallback(other, current_locale.as_str(), input)),
    }
}

async fn consume_fg(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let snapshot = session.snapshot_for_user(openid).await?;
    let locale = snapshot.settings.language.clone();
    let locale = locale.as_str();
    let candidates: Vec<String> = snapshot.background.keys().cloned().collect();
    let input = text.trim();
    match fuzzy_match_unique(input, &candidates) {
        FuzzyOutcome::Exact(alias) => switch_foreground(&alias, ctx, locale).await,
        other => Ok(fuzzy_fallback(other, locale, input)),
    }
}

async fn consume_sessions_projects(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let locale = user_locale(session, openid).await;
    let projects_view = session.last_projects_view(openid).await?;
    let project_key = match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
        Ok(value) => value,
        Err(err) => {
            return Ok(CommandOutcome::reply(err.to_string()));
        }
    };
    let (scope, project_path) = decode_project_key(&project_key)?;
    let all_sessions = session.list_disk_sessions(scope).await?;
    let sessions = all_sessions
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text_out, ids) = format_project_sessions_page(
        &SESSIONS_LIST_KEYS,
        &project_path,
        &sessions,
        1,
        locale.as_str(),
    );
    session.set_last_sessions_view(openid, ids).await?;
    session
        .set_pending_setting(
            openid,
            Some(PendingSetting::SessionsSessions {
                project_key,
                page: 1,
            }),
        )
        .await?;
    Ok(CommandOutcome::reply(text_out))
}

async fn consume_sessions_sessions(
    _text: &str,
    ctx: CmdCtx<'_>,
    project_key: String,
    page: usize,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    // /sessions is view-only; at this depth, remind the user how to act.
    let locale = user_locale(session, openid).await;
    let text = t!("commands.sessions.page_footer", locale = locale.as_str()).into_owned();
    session
        .set_pending_setting(
            openid,
            Some(PendingSetting::SessionsSessions { project_key, page }),
        )
        .await?;
    Ok(CommandOutcome::reply(text))
}

async fn consume_import_projects(text: &str, ctx: CmdCtx<'_>) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let locale = user_locale(session, openid).await;
    let projects_view = session.last_import_projects_view(openid).await?;
    let project_key = match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
        Ok(value) => value,
        Err(err) => {
            return Ok(CommandOutcome::reply(err.to_string()));
        }
    };
    let (_, project_path) = decode_project_key(&project_key)?;
    let all = session.list_importable_sessions()?;
    let project_sessions = all
        .into_iter()
        .filter(|item| item.cwd.display().to_string() == project_path)
        .collect::<Vec<_>>();
    let (text_out, ids) = format_project_sessions_page(
        &IMPORT_LIST_KEYS,
        &project_path,
        &project_sessions,
        1,
        locale.as_str(),
    );
    session.set_last_import_sessions_view(openid, ids).await?;
    session
        .set_pending_setting(
            openid,
            Some(PendingSetting::ImportSessions {
                project_key,
                page: 1,
            }),
        )
        .await?;
    Ok(CommandOutcome::reply(text_out))
}

async fn consume_import_sessions(
    text: &str,
    ctx: CmdCtx<'_>,
    project_key: String,
    page: usize,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let locale = user_locale(session, openid).await;
    let all = session.list_importable_sessions()?;
    let last_view = session.last_import_sessions_view(openid).await?;
    let target = match resolve_selector(text.trim(), &all, &last_view, locale.as_str()) {
        Ok(value) => value,
        Err(err) => {
            // Keep the pending state alive so the user can retry.
            session
                .set_pending_setting(
                    openid,
                    Some(PendingSetting::ImportSessions { project_key, page }),
                )
                .await?;
            return Ok(CommandOutcome::reply(err.to_string()));
        }
    };
    super::import_and_reply(session, openid, &target, locale.as_str()).await
}

async fn consume_restore_projects(
    mode: RestoreMode,
    text: &str,
    ctx: CmdCtx<'_>,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let locale = user_locale(session, openid).await;
    let projects_view = session.last_projects_view(openid).await?;
    let project_key = match resolve_project_selector(text.trim(), &projects_view, locale.as_str()) {
        Ok(value) => value,
        Err(err) => {
            return Ok(CommandOutcome::reply(err.to_string()));
        }
    };
    enter_restore_sessions_prompt(mode, ctx, project_key, 1, None, locale.as_str()).await
}

async fn consume_restore_sessions(
    mode: RestoreMode,
    text: &str,
    ctx: CmdCtx<'_>,
    project_key: String,
    page: usize,
    alias: Option<String>,
) -> Result<CommandOutcome> {
    let CmdCtx {
        openid, session, ..
    } = ctx;
    let locale = user_locale(session, openid).await;
    let sessions = session.list_disk_sessions(SessionListScope::All).await?;
    let last_view = session.last_sessions_view(openid).await?;
    let target = match resolve_selector(text.trim(), &sessions, &last_view, locale.as_str()) {
        Ok(value) => value,
        Err(err) => {
            // Keep the pending state alive so the user can retry.
            session
                .set_pending_setting(
                    openid,
                    Some(mode.sessions_pending(project_key, page, alias)),
                )
                .await?;
            return Ok(CommandOutcome::reply(err.to_string()));
        }
    };
    match mode {
        RestoreMode::Resume => execute_resume(ctx, &target).await,
        RestoreMode::Loadbg => execute_loadbg(ctx, &target, alias.as_deref()).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_exact_wins_over_substring() {
        let candidates = vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5.3-codex".to_string(),
        ];
        assert_eq!(
            fuzzy_match_unique("gpt-5.4", &candidates),
            FuzzyOutcome::Exact("gpt-5.4".to_string())
        );
    }

    #[test]
    fn fuzzy_substring_requires_uniqueness() {
        let candidates = vec!["gpt-5.4".to_string(), "gpt-5.4-mini".to_string()];
        match fuzzy_match_unique("5.4", &candidates) {
            FuzzyOutcome::Ambiguous(matches) => {
                assert_eq!(matches.len(), 2);
            }
            other => panic!("expected ambiguous, got {other:?}"),
        }
        assert_eq!(
            fuzzy_match_unique("mini", &candidates),
            FuzzyOutcome::Exact("gpt-5.4-mini".to_string())
        );
        assert_eq!(fuzzy_match_unique("nope", &candidates), FuzzyOutcome::None);
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        let candidates = vec!["Medium".to_string(), "Low".to_string()];
        assert_eq!(
            fuzzy_match_unique("MED", &candidates),
            FuzzyOutcome::Exact("Medium".to_string())
        );
    }

    #[test]
    fn fuzzy_empty_input_returns_none() {
        let candidates = vec!["on".to_string(), "off".to_string()];
        assert_eq!(fuzzy_match_unique("   ", &candidates), FuzzyOutcome::None);
    }

    #[test]
    fn model_alias_exact_match_prefers_canonical_name() {
        let candidates = vec![
            CodexModelEntry {
                name: "gpt-5.4".to_string(),
                aliases: vec!["54".to_string(), "5.4".to_string()],
                description: None,
                description_zh: None,
                description_en: None,
            },
            CodexModelEntry {
                name: "gpt-5.4-mini".to_string(),
                aliases: vec!["mini".to_string(), "54m".to_string()],
                description: None,
                description_zh: None,
                description_en: None,
            },
        ];
        assert_eq!(
            match_model_input("mini", &candidates),
            FuzzyOutcome::Exact("gpt-5.4-mini".to_string())
        );
        assert_eq!(
            match_model_input("54", &candidates),
            FuzzyOutcome::Exact("gpt-5.4".to_string())
        );
    }

    #[test]
    fn model_match_uses_unique_prefix_not_substring() {
        let candidates = vec![
            CodexModelEntry {
                name: "gpt-5.4".to_string(),
                aliases: vec!["54".to_string()],
                description: None,
                description_zh: None,
                description_en: None,
            },
            CodexModelEntry {
                name: "gpt-5.4-mini".to_string(),
                aliases: vec!["mini".to_string(), "54m".to_string()],
                description: None,
                description_zh: None,
                description_en: None,
            },
        ];
        assert_eq!(match_model_input("4-m", &candidates), FuzzyOutcome::None);
        assert_eq!(
            match_model_input("gpt-5.4-m", &candidates),
            FuzzyOutcome::Exact("gpt-5.4-mini".to_string())
        );
    }

    #[test]
    fn reasoning_alias_exact_match_prefers_supported_values() {
        assert_eq!(
            match_reasoning_input("高"),
            FuzzyOutcome::Exact("high".to_string())
        );
        assert_eq!(
            match_reasoning_input("超高"),
            FuzzyOutcome::Exact("xhigh".to_string())
        );
        assert_eq!(
            match_reasoning_input("默认"),
            FuzzyOutcome::Exact("inherit".to_string())
        );
        assert_eq!(
            match_reasoning_input("继承"),
            FuzzyOutcome::Exact("inherit".to_string())
        );
    }
}
