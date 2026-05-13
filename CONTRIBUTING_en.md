# Contributing Guidelines

> CodexClaw is a personal project by **Rhapsody0x1** -- a Rust QQ chat bot powered by OpenAI Codex.
> Bug reports and discussions are welcome; feature requests are considered at the author's discretion.
> You are also free to fork the project at any time to suit your own needs.

*Read this in: [English](CONTRIBUTING_en.md) | [中文](CONTRIBUTING.md)*

---

## 1. About

CodexClaw is a chat bot written in Rust that provides Codex capabilities through the QQ platform. The main directory structure is as follows:

| Path | Description |
|------|-------------|
| `src/main.rs` | Entry point and log initialization |
| `src/lib.rs` | Module mapping |
| `src/codex/` | Codex execution and event parsing |
| `src/qq/` | QQ API and gateway handling |
| `src/session/` | Persistent session state |
| `locales/` | Internationalization resource files (`en.yml`, `zh.yml`) |
| `config/` | Example configuration files |
| `data/` | Runtime state (not version-controlled) |

---

## 2. Development Setup

1. Install the [Rust toolchain](https://rustup.rs/) (the stable channel is sufficient).
2. Clone the repository and enter the project directory:
   ```bash
   git clone https://github.com/Rhapsody0x1/codex-claw.git
   cd codex-claw
   ```
3. Copy the example configuration file and fill in your own credentials:
   ```bash
   cp config/codexclaw.example.toml codexclaw.toml
   # Edit codexclaw.toml and fill in the QQ and OpenAI related keys
   ```
4. Specify the configuration path via environment variable (optional):
   ```bash
   export CODEX_CLAW_CONFIG=./codexclaw.toml
   ```

---

## 3. Build & Test

| Command | Purpose |
|---------|---------|
| `cargo check` | Fast type checking without producing an executable |
| `cargo build` | Build the debug version |
| `cargo run` | Start the bot (reads `codexclaw.toml` from the current directory) |
| `cargo test` | Run unit tests, integration tests, and doc tests |
| `cargo fmt` | Format code |
| `cargo clippy --all-targets --all-features` | Static analysis and lint checks |

Before submitting, make sure `cargo fmt`, `cargo clippy`, and `cargo test` all pass.

---

## 4. Code Style

- Follow `rustfmt` default rules: 4-space indentation, trailing commas inserted by the formatter, one module per file.
- Use `snake_case` for functions, modules, and test names; use `PascalCase` for type names.
- Keep async boundaries clear; return `anyhow::Result` at application boundaries.
- Avoid unnecessary `unwrap()`; prefer the `?` operator for error propagation.

---

## 5. Testing

- Async tests use the `#[tokio::test]` macro.
- Unit tests should be placed alongside the corresponding module (in the same file or in a `tests` submodule in the same directory).
- Integration tests for cross-module or HTTP flows go in `tests/app_server_smoke.rs`.
- Use `wiremock` to mock network calls and `tempfile` to manage temporary filesystem state.

Example:

```rust
#[tokio::test]
async fn test_session_persist() {
    let dir = tempfile::tempdir().unwrap();
    // ... test logic
}
```

---

## 6. Commit Conventions

Use the **Conventional Commits** style with short imperative subject lines. Supported prefixes:

| Prefix | Purpose | Example |
|--------|---------|---------|
| `feat` | New feature | `feat(qq): add group message handler` |
| `fix` | Bug fix | `fix(session): prevent duplicate writes` |
| `refactor` | Code refactoring | `refactor(codex): simplify event parser` |
| `doc` | Documentation update | `doc(README): update setup instructions` |

Each commit should contain only one logical change. If the change involves a specific module, indicate the scope in parentheses after the prefix, e.g., `feat(scheduler): add cron support`.

---

## 7. Pull Request Process

1. Create a feature branch from the `master` branch.
2. Ensure all checks pass: `cargo fmt`, `cargo clippy --all-targets --all-features`, `cargo test`.
3. When submitting a PR, please describe in the description:
   - What behavioral changes were made and why
   - The test commands that were run
   - Related issues (if any)
4. PRs will be reviewed by the author before deciding whether to merge. Whether functional PRs are accepted depends on the project direction; please discuss in an Issue or Discussion beforehand.

---

## 8. Internationalization

CodexClaw uses `rust_i18n` to provide bilingual support. Translation resource files are located at:

- `locales/en.yml` -- English
- `locales/zh.yml` -- Chinese

Use the `rust_i18n::t!` macro in code to retrieve translated text:

```rust
use rust_i18n::t;

let msg = t!("commands.help.description");
```

When adding new commands, you must provide both Chinese and English command aliases so that users of both languages can invoke them normally. When modifying or adding text, please update both locale files accordingly.

---

## 9. Security

- **Never** commit real QQ credentials, OpenAI API keys, or any other secrets.
- All sensitive information should be stored in a local TOML configuration file and loaded via the `CODEX_CLAW_CONFIG` environment variable.
- The `data/` directory is for runtime state and should not be version-controlled.
- If you discover a security vulnerability, please contact the author privately via an Issue or email; do not disclose it publicly.

---

## 10. License

This project is released under the [MIT License](LICENSE). By submitting code, you agree to license your contributions under the same terms.
