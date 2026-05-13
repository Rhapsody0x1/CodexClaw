# Repository Guidelines

## Project Structure & Module Organization
`src/` contains the application code. Use [`src/main.rs`](src/main.rs) for startup, logging setup, CLI dispatch, and bot bootstrapping, and [`src/lib.rs`](src/lib.rs) as the module map. Core areas are `src/codex/` for Codex execution, app-server integration, config snapshots, and event parsing; `src/qq/` for QQ API and gateway handling; `src/session/` for persisted session state; `src/scheduler/` for cron storage, CLI, execution, and interactive job handling; `src/memory/`, `src/shadow/`, and `src/skills/` for memory distillation and skill indexing; and `src/self_update.rs` for `/self-update`. Smoke coverage for the app-server path lives in [`tests/app_server_smoke.rs`](tests/app_server_smoke.rs). User and operator docs live in `docs/`, especially [`docs/scheduler.md`](docs/scheduler.md), [`docs/configuration.md`](docs/configuration.md), and [`docs/commands.md`](docs/commands.md). Example configuration is in [`config/codexclaw.example.toml`](config/codexclaw.example.toml), with model presets in [`config/codex_models.toml`](config/codex_models.toml). `assets/` holds README images, not runtime code.

## Build, Test, and Development Commands
Use standard Cargo workflows from the repo root:

- `cargo check` verifies the crate quickly without producing a release binary.
- `cargo test` runs unit, integration, and doc tests.
- `cargo test --test app_server_smoke -- --ignored --nocapture` runs the ignored app-server smoke test when a real Codex app-server path is needed.
- `cargo fmt` applies Rust formatting.
- `cargo clippy --all-targets --all-features` catches common lint issues before review.
- `cargo run` starts the bot with `codexclaw.toml` in the current directory.
- `CODEX_CLAW_CONFIG=./config/codexclaw.example.toml cargo run` runs with an explicit config path.
- `codex-claw cron add|once|list|rm|pause|resume|run-now|tail` manages scheduled tasks from the CLI when the binary is on `PATH`.

## Coding Style & Naming Conventions
Follow `rustfmt` defaults: 4-space indentation, trailing commas where formatter inserts them, and one module per file. Prefer `snake_case` for functions, modules, and test names, `PascalCase` for types, and concise enums/structs that mirror QQ or Codex payloads. Keep async boundaries explicit and return `anyhow::Result` at application edges where the project already does so.

## Testing Guidelines
Write async tests with `#[tokio::test]` when exercising runtime behavior. Prefer focused unit tests beside the owning module; keep real app-server or end-to-end smoke checks in `tests/app_server_smoke.rs` and mark them ignored unless they are safe for default CI. Use descriptive names such as `qq_text_send_falls_back_to_plain_text_when_markdown_is_rejected`. Mock network calls with `wiremock` and temporary filesystem state with `tempfile`. Scheduler changes need more than green tests: explicitly review timeout cancellation, foreground restoration, interactive cleanup, delivery fallback, file-locking behavior, and one-shot lifecycle semantics.

## Scheduler & Cron Operations
Use [`docs/scheduler.md`](docs/scheduler.md) as the source of truth for cron job behavior. Jobs are stored under `data/scheduler/jobs.json` with a lock file and atomic writes, so manage them through `codex-claw cron ...` or QQ `/cron list|pause|resume|rm|run-now|tail` instead of hand-editing runtime JSON. For workflow-like jobs such as news digests, repository health checks, or recurring research tasks, confirm sources, filtering, output format, delivery, and failure behavior before creating the job. Job-specific reusable logic belongs under `data/cron-jobs/<job_id>/workspace/.agents/skills/<skill-name>/SKILL.md`; do not use the legacy `<job_id>/skills` layout. One-shot jobs are recycled into `data/cron-jobs-trash/<timestamp>-<job_id>` after completion, while `cron rm` removes job files unless `--keep-files` is used.

## Commit & Pull Request Guidelines
Follow the Conventional Commits style established in the repository: use prefixes like `feat`, `fix`, `refactor`, `doc` with an optional scope in parentheses (e.g., `feat(scheduler): add cron support`). Keep each commit scoped to one logical change. Pull requests should describe the behavior change, list the commands you ran (`cargo test`, `cargo clippy`), link related issues, and include screenshots only when README or user-facing message formatting changes.

## Configuration & Security Tips
Do not commit real QQ credentials or Codex auth material. Keep secrets in a local TOML file and load it with `CODEX_CLAW_CONFIG`; config loading checks that variable first, then `./codexclaw.toml`, then `~/.codex-claw/codexclaw.toml`. Treat `data/` and `~/.codex-claw/.codex/` as sensitive runtime state: they can contain session settings, downloaded attachments, scheduler jobs, cron workspaces, run logs, pending deliveries, memory notes, skill files, and copied Codex config/auth files. Inspect and back up these directories before deleting or sharing them. Shell cron jobs can execute arbitrary programs, and Codex cron jobs may store prompts and outputs in logs, so avoid placing credentials in prompts, messages, job metadata, or run output.
