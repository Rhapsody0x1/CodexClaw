<div align="center">

<img src="./assets/banner.svg" width="600" alt="CodexClaw">

由 [OpenAI Codex App-Server](https://developers.openai.com/codex/app-server) 驱动的 QQ 私人 AI 助理

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](https://www.rust-lang.org/)

</div>

---

CodexClaw 是一个构建于 Codex App Server 上、接入 QQ 官方机器人平台的私人 AI 助理。它允许你通过 QQ 来操作你电脑上的 Codex，从而完成各种任务。

## 目录

- [宇宙级安全声明](#宇宙级安全声明)
- [为什么你又整了个 Claw？](#为什么你又整了个-claw)
- [功能亮点](#功能亮点)
- [快速开始](#快速开始)
- [命令速查](#命令速查)
- [定时任务](#定时任务)
- [运行时文件](#运行时文件)
- [文档](#文档)
- [贡献指南](#贡献指南)
- [给 Codex 看的部署说明](#给-codex-看的部署说明)

## 宇宙级安全声明

本项目*<u>**完全以 Vibe Coding 方式完成**</u>*，现以"按现状提供"为原则开源或分发。作者、贡献者及相关第三方不对本项目的**可用性、正确性、安全性、适用性、持续维护状态**或与任何特定用途的兼容性作出任何明示或默示保证。

使用者应自行完成代码审查、环境隔离、权限控制、依赖审计、数据备份与上线验证，并自行判断本项目是否符合其所在地区、平台规则、组织制度及业务场景中的法律、合规、安全和运维要求。

本项目可能**调用外部服务、读写本地文件、处理聊天消息**、上传或下载附件，并可能因模型输出、配置错误、依赖缺陷、平台接口变化或操作失误导致服务中断、数据泄露、误操作、额外费用、账号处罚或其他直接、间接、附带、特殊、惩罚性损失。除法律强制规定外，作者与贡献者对此不承担责任。

在部署或使用本项目时，您**应妥善保管各类账号凭据、访问令牌、聊天数据与服务器权限，并自行承担由此产生的全部风险与后果**。若您不同意上述条件，请不要部署、复制、修改或使用本项目。

## 为什么你又整了个 Claw？

1. 我很讨厌 OpenClaw，它像个玩具，太重，而且一上来就要我电脑的完全访问权限；
2. 比起 OpenClaw，Codex 的模型和 Harness 都很棒，而且 OpenAI 正在积极地维护它；
3. Codex 订阅给的额度真的很多 👍；
4. 这个项目在蹭 OpenClaw 的热度 : (

## 功能亮点

- **会话管理** &mdash; 前台/后台多会话并行，支持保存、恢复、导入系统 Codex 会话
- **定时任务** &mdash; 内置 cron 调度器，支持提醒、Codex 执行、Shell 脚本和交互式多轮对话
- **记忆蒸馏** &mdash; (实验性) 后台自动从对话中提取记忆和 Skill，注入后续 prompt
- **审批流程** &mdash; Codex 执行敏感操作前通过 QQ 请求用户审批，支持按会话自动放行
- **自更新** &mdash; 通过 QQ 发送 `/self-update` 即可拉取最新代码、编译并热替换

## 快速开始

> 完整部署指南见 [docs/getting-started.md](docs/getting-started.md)

1. 在你想要部署 CodexClaw 的设备上配置好 Codex 的登陆凭据，确认 `codex exec --json` 可以正常运行；

2. 前往 [QQ 开放平台](https://q.qq.com/)，按要求完成登陆和个人认证等操作；

3. 在"首页"中，点击"机器人"，然后点击"创建 QQ 机器人"，获得的 AppID 和 AppSecret 记录下来备用；
    ![image-20260410150111815](./assets/image-20260410150111815.png)
4. 返回之前的首页，进入你新创建机器人的高级设置页面；
    ![image-20260410150217404](./assets/image-20260410150217404.png)

5. 下拉左侧栏，找到"回调配置"，在单聊事件中点击"全选"，保存；
    ![image-20260410150539552](./assets/image-20260410150539552.png)

6. 将第 3 步中获得的 AppID 和 AppSecret 填入下面的提示词中，发送给 Codex，它会帮你完成接下来的工作。注意，由于涉及到系统服务安装等行为，Codex **可能会请求执行一些提权命令，请谨慎检查这些命令的安全性，作者不对因此造成的损失承担任何责任**。如果不希望将密钥泄露给 Codex 的话，你也可以要求 Codex 引导你手工完成部署过程。

```plain
帮我在本机上部署 CodexClaw：https://github.com/Rhapsody0x1/CodexClaw ，我已经获取了你所需的 AppId: {} 和 AppSecret: {}。然后将其注册为系统服务，并为我提供管理其启用状态的命令。
```

7. CodexClaw 默认以允许网络访问的 workspace-write 沙盒模式启动 Codex，这能一定程度上保护你的设备安全。但由于各个平台的安全机制，沙盒可能导致 Codex 的一些能力无法正常发挥。目前作者观察到的问题：

    - 在 macOS 上由于 Seatbelt 机制无法正常使用 Playwright 操作浏览器；
    - 在 Linux 上因系统安全机制不能调用 apt 等系统级命令；

    ~~如有条件，可以考虑在隔离的虚拟机/VPS 上以 danger-full-access 模式运行 Codex，这可以更好地发挥 Codex 的能力。至于如何修改配置，你可以询问万能的 Codex ; )~~ 目前项目已经迁移到了 Codex App Server，现在它可以向用户申请执行高于沙盒权限的命令。

## 命令速查

> 完整命令参考见 [docs/commands.md](docs/commands.md)

如果想要享受 QQ 官方机器人提供的快捷命令，可手动将下面的命令添加到命令列表。当然，不添加也不影响它们的正常使用。

| 命令 | 中文别名 | 说明 |
|------|---------|------|
| `/help` | `/帮助` | 查看命令列表 |
| `/status` | `/状态` | 查看当前会话状态 |
| `/new [dir]` | `/新建` | 新建前台会话 |
| `/stop` | `/停止` | 结束当前会话 |
| `/sessions` | `/会话` | 列出历史会话 |
| `/model [name]` | `/模型` | 设置或查看模型 |
| `/compact` | `/压缩` | 压缩会话上下文 |
| `/self-update` | `/自更新` | 编译并热替换二进制 |

<details>
<summary>更多命令</summary>

- `/interrupt` / `/中断`：仅停止当前运行，不结束会话
- `/lang [en\|zh]` / `/语言`：切换界面语言
- `/fast [on\|off]` / `/快速`：设置 Fast 模式
- `/context [1m\|standard]` / `/上下文`：设置上下文模式
- `/reasoning [low\|medium\|high\|xhigh]` / `/思考`：设置思考深度
- `/verbose [on\|off]` / `/详细`：切换详细输出
- `/save` / `/保存`：显式保存当前前台会话
- `/bg [alias]` / `/后台`：将当前会话转入后台
- `/fg <alias>` / `/前台`：切回后台会话
- `/resume <id>` / `/恢复`：恢复磁盘会话
- `/import` / `/导入`：导入系统 Codex 会话
- `/loadbg <id> [alias]` / `/载入后台`：加载会话到后台
- `/rename <old> <new>` / `/重命名`：重命名后台标签
- `/alias` / `/别名`：管理命令别名
- `/approvals` / `/审批`：切换审批策略
- `/approve` / `/同意`：放行审批请求
- `/deny` / `/拒绝`：拒绝审批请求
- `/plan` / `/计划`：进入计划模式
- `/cron` / `/定时`：管理定时任务

</details>

## 文档

| 文档 | 说明 |
|------|------|
| [快速入门](docs/getting-started.md) | 安装、配置、首次运行与系统服务注册 |
| [命令参考](docs/commands.md) | 全部 QQ 命令的详细用法 |
| [配置参考](docs/configuration.md) | 所有 TOML 配置项及默认值 |
| [定时任务](docs/scheduler.md) | 调度器系统、CLI 完整参考与最佳实践 |
| [系统架构](docs/architecture.md) | 模块职责、数据流与扩展指南 |
| [贡献指南](CONTRIBUTING.md) | 开发环境、代码风格与 PR 流程 |

## 贡献指南

本项目主要围绕作者的个人需求与兴趣演进。如果你有新的想法或使用场景，欢迎在 Discussion 中交流；如果你发现了明确的 Bug 或兼容性问题，也欢迎提交 Issue。对于功能建议，作者不保证一定会采纳、排期或长期维护。

若你的需求较为个性化，或与当前路线不完全一致，你可以直接 fork 本仓库，并在其基础上继续定制。

详细的开发指南见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 给 Codex 看的部署说明

1. 先检查基础环境，确认当前机器可联网，且已经安装 Rust 与 Codex CLI：

```bash
uname -a
whoami
pwd
rustc --version
cargo --version
codex --version
git --version
```

2. 如果缺少 Rust 工具链，则安装 Rust；如果缺少 `codex`，则先按当前官方方式安装，并要求用户完成登录。验收命令。若这一步失败，不要继续部署，先要求用户修复 Codex 运行环境与凭据问题。

```bash
codex exec --json "reply with exactly: ok"
```

3. 确认系统 Codex 目录 `~/.codex` 已存在，且至少包含 `config.toml` 与 `auth.json`。`codex-claw` 首次启动会将这些文件及 `skills/` 复制到 `~/.codex-claw/.codex/` 作为独立运行目录。

4. 使用用户目录部署（无需提权），例如：

```bash
mkdir -p ~/.codex-claw
cd ~/.codex-claw
git clone https://github.com/Rhapsody0x1/CodexClaw.git repo
cd repo
```

若目录已存在仓库，则改为：

```bash
cd ~/.codex-claw/repo
git pull --ff-only
```

5. 创建运行配置文件，例如 `~/.codex-claw/codexclaw.toml`。将用户提供的 `AppID` 与 `AppSecret` 写入其中对应位置：

```toml
[general]
data_dir = "~/.codex-claw/data"
system_codex_home = "~/.codex"
codex_home_global = "~/.codex-claw/.codex"
default_workspace_dir = "~/.codex-claw/data/session/workspace"
codex_binary = "codex"
default_model = "gpt-5.4"
default_reasoning_effort = "medium"
self_repo_dir = "~/.codex-claw/repo"
self_build_command = "cargo build --release"
self_binary_path = "~/.codex-claw/repo/target/release/codex-claw"

[qq]
app_id = "YOUR_APP_ID"
app_secret = "YOUR_APP_SECRET"
api_base_url = "https://sandbox.api.sgroup.qq.com"
token_url = "https://bots.qq.com/app/getAppAccessToken"

[scheduler]
enabled = true
tick_secs = 30
default_tz = "Asia/Shanghai"
max_concurrent_jobs = 4
max_turn_secs = 600
max_attempts = 3
retry_backoff_secs = 30
circuit_breaker_threshold = 5
runs_retention = 30
```

6. 先做一次编译检查，再构建 release：

```bash
cd ~/.codex-claw/repo
cargo check
cargo build --release
```

7. 用前台方式先启动一次，确认程序能正常连上 QQ Gateway，且没有明显配置错误：

```bash
cd ~/.codex-claw/repo
CODEX_CLAW_CONFIG=~/.codex-claw/codexclaw.toml ./target/release/codex-claw
```

如果日志中出现 access token 获取失败、Gateway 连接失败或 Codex 启动失败，先停止并修复问题，再继续后续步骤。

8. 注册为"用户级自启服务"时，需要按系统环境灵活处理（如 macOS `launchd`、Linux 用户级 `systemd`、其他 init 系统）。核心要求：
- 工作目录指向 `~/.codex-claw/repo`
- 设置 `CODEX_CLAW_CONFIG=~/.codex-claw/codexclaw.toml`
- 启动命令为 `~/.codex-claw/repo/target/release/codex-claw`
- 使用当前登录用户运行，不要求 root

9. 启用并启动服务（命令因系统而异）。由于 `/self-update` 会替换当前运行中的二进制并退出当前进程，因此建议交给外部服务管理器负责重新拉起。

10. 最后提醒从 QQ 客户端发送一条普通私聊消息进行联调，并检查服务日志。
