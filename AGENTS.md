# Repository Guidelines

## Project Structure & Module Organization
`src/` contains the application code. Use [`src/main.rs`](src/main.rs) for startup and logging setup, and [`src/lib.rs`](src/lib.rs) as the module map. Core areas are `src/codex/` for Codex execution and event parsing, `src/qq/` for QQ API and gateway handling, and `src/session/` for persisted session state. Integration coverage lives in [`tests/integration_flow.rs`](tests/integration_flow.rs). Example configuration is in [`config/codexclaw.example.toml`](config/codexclaw.example.toml). `assets/` holds README images, not runtime code.

## Build, Test, and Development Commands
Use standard Cargo workflows from the repo root:

- `cargo check` verifies the crate quickly without producing a release binary.
- `cargo test` runs unit, integration, and doc tests.
- `cargo fmt` applies Rust formatting.
- `cargo clippy --all-targets --all-features` catches common lint issues before review.
- `cargo run` starts the bot with `codexclaw.toml` in the current directory.
- `CODEX_CLAW_CONFIG=./config/codexclaw.example.toml cargo run` runs with an explicit config path.

## Coding Style & Naming Conventions
Follow `rustfmt` defaults: 4-space indentation, trailing commas where formatter inserts them, and one module per file. Prefer `snake_case` for functions, modules, and test names, `PascalCase` for types, and concise enums/structs that mirror QQ or Codex payloads. Keep async boundaries explicit and return `anyhow::Result` at application edges where the project already does so.

## Testing Guidelines
Write async tests with `#[tokio::test]` when exercising runtime behavior. Prefer focused unit tests beside the owning module and keep cross-module or HTTP flow checks in `tests/integration_flow.rs`. Use descriptive names such as `qq_text_send_falls_back_to_plain_text_when_markdown_is_rejected`. Mock network calls with `wiremock` and temporary filesystem state with `tempfile`.

## Commit & Pull Request Guidelines
This repository currently has no commit history, so there is no established convention to copy yet. Use short imperative commit subjects such as `Add QQ token refresh retry`, and keep each commit scoped to one logical change. Pull requests should describe the behavior change, list the commands you ran (`cargo test`, `cargo clippy`), link related issues, and include screenshots only when README or user-facing message formatting changes.

## Configuration & Security Tips
Do not commit real QQ credentials. Keep secrets in a local TOML file and load it with `CODEX_CLAW_CONFIG`. Treat `data/` as runtime state; inspect before deleting because it stores session settings and downloaded attachments.
