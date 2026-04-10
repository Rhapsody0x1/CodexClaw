# CodexClaw

CodexClaw 是一个基于 QQ 官方机器人平台的、由 Codex 驱动的私人 AI 助理。

## 声明

本项目**完全以 Vibe Coding 方式完成**，现以“按现状提供”为原则开源或分发。作者、贡献者及相关第三方不对本项目的**可用性、正确性、安全性、适用性、持续维护状态**或与任何特定用途的兼容性作出任何明示或默示保证。

使用者应自行完成代码审查、环境隔离、权限控制、依赖审计、数据备份与上线验证，并自行判断本项目是否符合其所在地区、平台规则、组织制度及业务场景中的法律、合规、安全和运维要求。

本项目可能**调用外部服务、读写本地文件、处理聊天消息**、上传或下载附件，并可能因模型输出、配置错误、依赖缺陷、平台接口变化或操作失误导致服务中断、数据泄露、误操作、额外费用、账号处罚或其他直接、间接、附带、特殊、惩罚性损失。除法律强制规定外，作者与贡献者对此不承担责任。

在部署或使用本项目时，您**应妥善保管各类账号凭据、访问令牌、聊天数据与服务器权限，并自行承担由此产生的全部风险与后果**。若您不同意上述条件，请不要部署、复制、修改或使用本项目。

## 为什么你又写了个 Claw？

1. 我真的很讨厌 OpenClaw，它像个玩具，而且需要我电脑的完全访问权限；
2. 我真的很喜欢 Codex，它的任务交付能力相当可靠，而且沙箱安全机制也更完善；
3. Codex 订阅给的额度真的很多，赞 👍。

## 给人类看的部署说明

1. 在你想要部署 CodexClaw 的设备上配置好 Codex 的登陆凭据，确认 `codex exec --json` 可以正常运行；

2. 前往 [QQ 开放平台](https://q.qq.com/)，按要求完成登陆和个人认证等操作；

3. 在“首页”中，点击“机器人”，然后点击“创建 QQ 机器人”，获得的 AppID 和 AppSecret 记录下来备用；
    ![image-20260410150111815](./assets/image-20260410150111815.png)
4. 返回之前的首页，进入你新创建机器人的高级设置页面；
    ![image-20260410150217404](./assets/image-20260410150217404.png)

5. 下拉左侧栏，找到“回调配置”，在单聊事件中点击“全选”，保存；
    ![image-20260410150539552](./assets/image-20260410150539552.png)

6. 将第 3 步中获得的 AppID 和 AppSecret 填入下面的提示词中，发送给 Codex，它会帮你完成接下来的工作。当然你要是不希望将密钥泄露给 Codex 的话你也可以手工完成部署。

```plain
帮我在本机上运行 CodexClaw 项目：https://github.com/Rhapsody0x1/CodexClaw ，我已经获取了你所需的 AppId: {} 和 AppSecret: {}，然后将其注册为系统服务，并为我提供管理其启用状态的命令。
```

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

3. 确认 `~/.codex/config.toml` 与 `~/.codex/auth.json` 已存在。如不存在，则提示用户先完成 Codex 登录；如需要部署到 root 或 systemd 运行用户下，确保对应用户也能访问这些文件。

4. 选择部署目录，例如 `/opt/codex-claw`，然后拉取仓库：

```bash
mkdir -p /opt/codex-claw
cd /opt/codex-claw
git clone https://github.com/Rhapsdody0x1/CodexClaw.git repo
cd repo
```

若目录已存在仓库，则改为：

```bash
cd /opt/codex-claw/repo
git pull --ff-only
```

5. 创建运行配置文件，例如 `/opt/codex-claw/codexclaw.toml`。将用户提供的 `AppID` 与 `AppSecret` 写入其中对应位置：

```toml
[general]
data_dir = "/opt/codex-claw/data"
codex_binary = "codex"
default_model = "gpt-5.4"
default_reasoning_effort = "medium"

[qq]
app_id = "YOUR_APP_ID"
app_secret = "YOUR_APP_SECRET"
api_base_url = "https://api.sgroup.qq.com"
token_url = "https://bots.qq.com/app/getAppAccessToken"
```

6. 先做一次编译检查，再构建 release：

```bash
cd /opt/codex-claw/repo
cargo check
cargo build --release
```

7. 用前台方式先启动一次，确认程序能正常连上 QQ Gateway，且没有明显配置错误：

```bash
cd /opt/codex-claw/repo
CODEX_CLAW_CONFIG=/opt/codex-claw/codexclaw.toml ./target/release/codex-claw
```

如果日志中出现 access token 获取失败、Gateway 连接失败或 Codex 启动失败，先停止并修复问题，再继续后续步骤。

8. 如果需要注册为 systemd 服务，可写入 `/etc/systemd/system/codex-claw.service`；如果系统无 systemd，请按照实际环境进行系统服务注册。

```ini
[Unit]
Description=CodexClaw QQ Bot
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
WorkingDirectory=/opt/codex-claw/repo
Environment=HOME=/root
Environment=CODEX_HOME=/root/.codex
Environment=CODEX_CLAW_CONFIG=/opt/codex-claw/codexclaw.toml
ExecStart=/opt/codex-claw/repo/target/release/codex-claw
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

9. 启用并启动服务：

```bash
systemctl daemon-reload
systemctl enable --now codex-claw
systemctl status codex-claw --no-pager
journalctl -u codex-claw -f
```

10. 最后提醒从 QQ 客户端发送一条普通私聊消息进行联调。若需要验证运行状态，可继续检查：

```bash
systemctl status codex-claw --no-pager
journalctl -u codex-claw --since "10 minutes ago" --no-pager
```

## 命令列表

如果想要享受 QQ 官方机器人提供的快捷命令，可手动将下面的命令添加到命令列表。当然不添加也不影响它们的正常使用。

- `/help`：查看命令列表；
- `/model [name|inherit|status]`：设置或查看当前模型；
- `/fast [on|off|inherit|status]`：设置 Fast 模式；
- `/context [1m|standard|inherit|status]`：设置上下文模式；
- `/reasoning [low|medium|high|xhigh|inherit|status]`：设置思考深度；
- `/verbose [on|off|status]`：切换工具输出的简略或详细模式；
- `/plan [on|off|status]`：切换计划模式；
- `/status`：查看当前会话状态；
- `/new`：保留当前设置并重置 Codex 会话；
- `/stop` 或 `/interrupt`：停止当前运行。

## 运行时文件

- 收到的图片和文件会下载到 `data/session/main/workspace/inbox/`。
- 会话设置将持久化到 `data/session/main/settings.json`。

## 贡献指南

本项目主要围绕作者的个人需求与兴趣演进。如果你有新的想法或使用场景，欢迎在 Discussion 中交流；如果你发现了明确的 Bug 或兼容性问题，也欢迎提交 Issue。对于功能建议，作者不保证一定会采纳、排期或长期维护。

若你的需求较为个性化，或与当前路线不完全一致，你可以直接 fork 本仓库，并在其基础上继续定制。
