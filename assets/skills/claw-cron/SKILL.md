---
name: "claw-cron"
description: "Register and manage codex-claw scheduled tasks from an agent turn."
---

# codex-claw Scheduled Tasks

Use the `codex-claw cron` CLI when the user asks to create, list, pause, resume, remove, or immediately trigger scheduled tasks.

Do not create workflow-like scheduled jobs from a vague one-line prompt. A workflow-like job is anything that must repeatedly gather data, call websites/APIs, use search/social/news sources, run scripts, maintain source lists, transform outputs, or make nontrivial decisions at trigger time. Examples: daily AI news breakfast, market scan, paper digest, repo health report, scraping, multi-source summaries.

For workflow-like jobs, first design the execution workflow and confirm it with the user before creating the cron job:
1. Clarify schedule, timezone, delivery format, sources, allowed network/search behavior, language, length, failure behavior, and whether the job may create/update scripts.
2. Propose a concrete workflow: source list, collection steps, filtering/dedup rules, summarization format, and fallback behavior.
3. Ask for confirmation. Do not run `codex-claw cron add` until the user confirms.
4. After confirmation, create the job, then place a job-specific Skill under the job workspace at `<job_workspace>/.agents/skills/<skill-name>/SKILL.md`. Use `codex-claw cron list` or the create output to get the job id, and inspect `<job_workspace>/.claw-job.json` for metadata.
5. The scheduled prompt should tell Codex to use that job-specific Skill and execute the confirmed workflow. Avoid vague prompts like “collect news and summarize”; encode the workflow in the Skill.

Job-specific Skill location:
- Use `<cron_job_id>/workspace/.agents/skills/...` inside `data/cron-jobs/`.
- Do not put new job-specific Skills in `<cron_job_id>/skills`; that path is legacy and not the canonical workspace-local skill location.
- Keep scripts, source lists, caches, and notes in the job `workspace/` so future triggers reuse them.

Examples:

```shell
codex-claw cron add --cron "0 16 * * *" --tz Asia/Shanghai --title "homework reminder" --action reminder --message "记得检查还有没有没交的作业"
codex-claw cron once --at "2026-05-20T08:00:00+08:00" --title "interview reminder" --action reminder --message "准备面试"
codex-claw cron add --cron "0 9 * * 1-5" --tz Asia/Shanghai --title "repo health check" --action codex-exec --workspace "/path/to/repo" --prompt "检查这个仓库的测试状态，简短总结发现，不要修改文件。"
codex-claw cron add --cron "0 18 * * 5" --tz Asia/Shanghai --title "weekly project review" --action codex-turn --workspace "/path/to/repo" --session-strategy persistent --prompt "继续跟踪这个项目，本周回顾待办、最近变更和下周风险，给出简短中文总结。"
codex-claw cron once --at "2026-05-20T20:00:00+08:00" --title "quiz" --action codex-turn --session-strategy per-invocation --interactive --reply-ttl 86400 --prompt "出一道题，等待用户回答。判分后在末尾单独输出 <<<CLAW_END>>>"
codex-claw cron list
codex-claw cron run-now <job_id>
codex-claw cron pause <job_id>
codex-claw cron resume <job_id>
codex-claw cron rm <job_id>
codex-claw cron tail <job_id>
```

If you are running inside a codex-claw QQ turn, omit `--owner`; the CLI reads `.claw-turn.json` from the current workspace. Otherwise pass `--owner <openid>`.

The CLI auto-discovers `~/.codex-claw/codexclaw.toml` when `CODEX_CLAW_CONFIG` is not set.

Supported actions:
- `reminder`: requires `--message`. Use this for normal user reminders. The message is the text to send when the schedule fires.
- `shell`: requires `--program`, accepts repeated `--arg`.
- `codex-exec`: runs `codex exec` with `CODEX_HOME` set to codex-claw's global Codex home.
- `codex-turn`: runs one app-server turn; pass `--session-strategy per-invocation` or `--session-strategy persistent`. It accepts `--approval never|guardian|on-request|unless-trusted`; unattended non-interactive jobs should normally use the default `never`.

Action selection:
- Use `reminder` when the scheduled task only needs to send a fixed reminder text.
- Use `shell` when a deterministic local command is enough and no Codex reasoning is needed, for example running `/bin/echo`, a backup script, or a project script.
- Use `codex-exec` when the scheduled task should run an isolated Codex CLI job. It is best for stateless checks or one-off analysis in a workspace. It does not preserve an app-server conversation across runs.
- Use `codex-turn` when the scheduled task should behave like a codex-claw conversation turn. It can keep a Codex session when `--session-strategy persistent` is used, making it suitable for recurring project monitoring or follow-up work that benefits from continuity.
- Add `--interactive` only when the scheduled task needs to take over the foreground conversation, ask the user for replies, and later restore the previous conversation. The prompt must tell Codex to output the end signal `<<<CLAW_END>>>` when finished.

`codex-exec` examples:
```shell
codex-claw cron once --at "2026-05-20T09:00:00+08:00" --title "test summary" --action codex-exec --workspace "/path/to/repo" --prompt "运行或检查测试状态，只返回失败点和下一步建议，不要修改文件。"
codex-claw cron add --cron "0 9 * * 1" --tz Asia/Shanghai --title "weekly dependency scan" --action codex-exec --workspace "/path/to/repo" --prompt "查看依赖相关文件，提醒我是否有明显需要升级或审查的依赖。"
```

`codex-turn` examples:
```shell
codex-claw cron add --cron "0 18 * * 5" --tz Asia/Shanghai --title "weekly project review" --action codex-turn --workspace "/path/to/repo" --session-strategy persistent --prompt "延续上次项目跟踪，汇总本周进展、未完成事项和下周风险。"
codex-claw cron once --at "2026-05-20T20:00:00+08:00" --title "one-off agent check" --action codex-turn --workspace "/path/to/repo" --session-strategy per-invocation --prompt "检查 README 中是否有过期命令，只报告发现，不要修改文件。"
codex-claw cron once --at "2026-05-20T20:00:00+08:00" --title "interactive quiz" --action codex-turn --session-strategy per-invocation --interactive --prompt "出一道考研选择题，等待用户回答并判分。判分结束后在末尾单独输出 <<<CLAW_END>>>"
```

Important:
- For requests like "提醒我 X", "每天 16:00 提醒我 X", or "明天提醒我 X", create a `reminder` job. Do not use `codex-exec` or `codex-turn`.
- The reminder message must be only the content the user wants to receive at trigger time, for example `X`. It must not be the full scheduling request like "每天 16:00 提醒我 X".
- For recurring workflow jobs such as “每天早上 9 点获取当天和前一天最新 AI 新闻并发早餐摘要”, do not immediately create a generic `codex-exec` job. First confirm the workflow with the user, then create a job-specific Skill in `workspace/.agents/skills` and make the job prompt invoke that Skill.
- Use `codex-exec` or `codex-turn` only when the scheduled task should perform autonomous Codex work at trigger time, such as checking files, running commands, summarizing results, or editing code.
- If you use `codex-exec` or `codex-turn`, write the prompt as an execution instruction for the future trigger. Do not ask Codex to create another reminder.
- `run-now` triggers one extra execution via `run_now_at`; it does not rewrite the normal `next_run_at`.
- Human-side management is also available through QQ: `/cron list`, `/cron pause <job_id>`, `/cron resume <job_id>`, `/cron rm <job_id>`, `/cron run-now <job_id>`, `/cron tail <job_id>`.
