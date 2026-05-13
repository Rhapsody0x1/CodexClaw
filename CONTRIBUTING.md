# 贡献指南

> CodexClaw 是 **Rhapsody0x1** 的个人项目——一个基于 OpenAI Codex 的 Rust QQ 机器人。
> 欢迎提交 Bug 报告和参与讨论；功能建议将由作者酌情采纳。
> 你也可以随时 Fork 本项目来满足个性化需求。

*Read this in: [English](CONTRIBUTING_en.md) | [中文](CONTRIBUTING.md)*

---

## 1. 项目背景

CodexClaw 是一个使用 Rust 编写、通过 QQ 平台提供 Codex 能力的聊天机器人。主要目录结构如下：

| 路径 | 说明 |
|------|------|
| `src/main.rs` | 启动入口与日志初始化 |
| `src/lib.rs` | 模块映射 |
| `src/codex/` | Codex 执行与事件解析 |
| `src/qq/` | QQ API 与网关处理 |
| `src/session/` | 持久化会话状态 |
| `locales/` | 国际化资源文件 (`en.yml`, `zh.yml`) |
| `config/` | 配置文件示例 |
| `data/` | 运行时状态（不纳入版本控制） |

---

## 2. 开发环境

1. 安装 [Rust 工具链](https://rustup.rs/)（stable channel 即可）。
2. 克隆仓库并进入项目目录：
   ```bash
   git clone https://github.com/Rhapsody0x1/codex-claw.git
   cd codex-claw
   ```
3. 复制配置文件并填写自己的凭据：
   ```bash
   cp config/codexclaw.example.toml codexclaw.toml
   # 编辑 codexclaw.toml，填入 QQ 和 OpenAI 相关密钥
   ```
4. 通过环境变量指定配置路径（可选）：
   ```bash
   export CODEX_CLAW_CONFIG=./codexclaw.toml
   ```

---

## 3. 构建与测试

| 命令 | 用途 |
|------|------|
| `cargo check` | 快速类型检查，不产生可执行文件 |
| `cargo build` | 构建调试版本 |
| `cargo run` | 启动机器人（读取当前目录下的 `codexclaw.toml`） |
| `cargo test` | 运行单元测试、集成测试和文档测试 |
| `cargo fmt` | 格式化代码 |
| `cargo clippy --all-targets --all-features` | 静态分析与 lint 检查 |

提交前请确保 `cargo fmt`、`cargo clippy` 和 `cargo test` 全部通过。

---

## 4. 代码风格

- 遵循 `rustfmt` 默认规则：4 空格缩进，格式化器自动插入尾逗号，每个模块对应一个文件。
- 函数、模块和测试名使用 `snake_case`，类型名使用 `PascalCase`。
- 保持 async 边界清晰，在应用边界处返回 `anyhow::Result`。
- 避免不必要的 `unwrap()`，优先使用 `?` 操作符进行错误传播。

---

## 5. 测试指南

- 异步测试使用 `#[tokio::test]` 宏。
- 单元测试应放在对应模块旁边（同文件或同目录的 `tests` 子模块）。
- 跨模块或 HTTP 流程的集成测试放在 `tests/app_server_smoke.rs`。
- 使用 `wiremock` 模拟网络调用，使用 `tempfile` 管理临时文件系统状态。

示例：

```rust
#[tokio::test]
async fn test_session_persist() {
    let dir = tempfile::tempdir().unwrap();
    // ... 测试逻辑
}
```

---

## 6. 提交规范

使用 **Conventional Commits** 风格，主题行为简短的祈使句。支持的前缀：

| 前缀 | 用途 | 示例 |
|------|------|------|
| `feat` | 新功能 | `feat(qq): add group message handler` |
| `fix` | 修复缺陷 | `fix(session): prevent duplicate writes` |
| `refactor` | 重构代码 | `refactor(codex): simplify event parser` |
| `doc` | 文档更新 | `doc(README): update setup instructions` |

每个 commit 应当只包含一个逻辑变更。如果变更涉及特定模块，请在前缀后用括号标注作用域，例如 `feat(scheduler): add cron support`。

---

## 7. PR 流程

1. 从 `master` 分支创建功能分支。
2. 确保所有检查通过：`cargo fmt`、`cargo clippy --all-targets --all-features`、`cargo test`。
3. 提交 PR 时请在描述中说明：
   - 行为变更的内容和原因
   - 运行过的测试命令
   - 关联的 Issue（如有）
4. PR 将由作者 Review 后决定是否合并。功能性 PR 是否被接受取决于项目方向，请提前在 Issue 或 Discussion 中讨论。

---

## 8. 国际化

CodexClaw 使用 `rust_i18n` 提供双语支持。翻译资源文件位于：

- `locales/en.yml` — 英文
- `locales/zh.yml` — 中文

在代码中使用 `rust_i18n::t!` 宏获取翻译文本：

```rust
use rust_i18n::t;

let msg = t!("commands.help.description");
```

添加新命令时，必须同时提供中文和英文的命令别名（alias），确保两种语言的用户都能正常调用。修改或新增文本时请同步更新两个 locale 文件。

---

## 9. 安全须知

- **绝对不要** 提交真实的 QQ 凭据、OpenAI API Key 或任何其他密钥。
- 所有敏感信息应存放在本地 TOML 配置文件中，通过 `CODEX_CLAW_CONFIG` 环境变量加载。
- `data/` 目录为运行时状态目录，不应纳入版本控制。
- 如果发现安全漏洞，请通过 Issue 私信或邮件联系作者，不要公开披露。

---

## 10. 许可证

本项目基于 [MIT License](LICENSE) 发布。提交代码即表示你同意以相同许可证授权你的贡献。

