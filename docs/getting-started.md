# 快速入门

CodexClaw 的安装、配置与首次运行指南。

*Read this guide in: [English](getting-started_en.md) | [中文](getting-started.md)*

## 前置条件

- Rust 工具链 (edition 2024)
- OpenAI Codex CLI 已安装并完成登录认证，确认 `codex exec --json "reply with exactly: ok"` 可正常运行
- 系统 `~/.codex` 目录已存在，至少包含 `config.toml` 与 `auth.json`
- QQ 开放平台账号（需完成个人认证）

## 安装

```bash
mkdir -p ~/.codex-claw
cd ~/.codex-claw
git clone https://github.com/Rhapsody0x1/CodexClaw.git repo
cd repo
cargo build --release
```

若目录已存在仓库：
```bash
cd ~/.codex-claw/repo
git pull --ff-only
cargo build --release
```

## 配置

1. 复制示例配置文件：
```bash
cp config/codexclaw.example.toml ~/.codex-claw/codexclaw.toml
```

2. 编辑 `~/.codex-claw/codexclaw.toml`，填入 QQ 机器人凭据：
```toml
[qq]
app_id = "你的AppID"
app_secret = "你的AppSecret"
api_base_url = "https://sandbox.api.sgroup.qq.com"  # 正式环境改为 https://api.sgroup.qq.com
```

完整配置项说明见 [配置参考](configuration.md)。

## QQ 开放平台设置

1. 前往 [QQ 开放平台](https://q.qq.com/)，完成登录和个人认证；

2. 在"首页"中，点击"机器人"，然后点击"创建 QQ 机器人"，记录获得的 AppID 和 AppSecret；
   ![创建机器人](../assets/image-20260410150111815.png)

3. 返回首页，进入新创建机器人的高级设置页面；
   ![高级设置](../assets/image-20260410150217404.png)

4. 下拉左侧栏，找到"回调配置"，在单聊事件中点击"全选"，保存。
   ![回调配置](../assets/image-20260410150539552.png)

## 首次运行

先以前台方式启动，确认程序能正常连上 QQ Gateway：

```bash
cd ~/.codex-claw/repo
CODEX_CLAW_CONFIG=~/.codex-claw/codexclaw.toml ./target/release/codex-claw
```

**日志排查要点：**
- 如果出现 access token 获取失败 → 检查 `app_id` 和 `app_secret` 是否正确
- 如果 Gateway 连接失败 → 检查网络连接和 `api_base_url`
- 如果 Codex 启动失败 → 确认 `codex exec --json` 可正常运行

首次启动会自动从系统 `~/.codex` 复制 `config.toml`、`auth.json`、`skills/` 到 `~/.codex-claw/.codex/`（已存在文件不覆盖）。

确认无误后，从 QQ 客户端向机器人发送一条私聊消息进行联调。

## 注册为系统服务

### macOS (launchd)

创建 `~/Library/LaunchAgents/com.codexclaw.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.codexclaw</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/你的用户名/.codex-claw/repo/target/release/codex-claw</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/Users/你的用户名/.codex-claw/repo</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>CODEX_CLAW_CONFIG</key>
        <string>/Users/你的用户名/.codex-claw/codexclaw.toml</string>
    </dict>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/Users/你的用户名/.codex-claw/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/你的用户名/.codex-claw/stderr.log</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.codexclaw.plist
launchctl start com.codexclaw
```

### Linux (systemd --user)

创建 `~/.config/systemd/user/codexclaw.service`：

```ini
[Unit]
Description=CodexClaw QQ Bot
After=network-online.target

[Service]
Type=simple
WorkingDirectory=%h/.codex-claw/repo
Environment=CODEX_CLAW_CONFIG=%h/.codex-claw/codexclaw.toml
ExecStart=%h/.codex-claw/repo/target/release/codex-claw
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now codexclaw
```

> **提示：** 由于 `/self-update` 命令会替换运行中的二进制并退出当前进程，建议交给外部服务管理器负责自动重新拉起。

## 沙盒模式

CodexClaw 默认以允许网络访问的 workspace-write 沙盒模式启动 Codex，这能一定程度上保护你的设备安全。但沙盒可能导致 Codex 的一些能力无法正常发挥：

- **macOS**: 由于 Seatbelt 机制无法正常使用 Playwright 操作浏览器
- **Linux**: 因系统安全机制不能调用 `apt` 等系统级命令

~~如有条件，可以考虑在隔离的虚拟机/VPS 上以 `danger-full-access` 模式运行 Codex，这可以更好地发挥 Codex 的能力。~~ 目前项目已经迁移到了 Codex App Server，现在它可以向用户申请执行高于沙盒权限的命令。

## 更新

**手动更新：**
```bash
cd ~/.codex-claw/repo
git pull --ff-only
cargo build --release
# 重启服务
```

**QQ 内更新：** 发送 `/self-update` 或 `/自更新`，CodexClaw 会自动拉取最新代码、编译并替换运行中的二进制。

