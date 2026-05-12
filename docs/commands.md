# CodexClaw 命令参考 / Command Reference

> CodexClaw 是一个由 OpenAI Codex 驱动的 Rust QQ 机器人。所有命令通过 QQ 私聊以 `/命令` 格式触发。
> 中文别名与英文命令完全等价，在消息调度层自动规范化。

---

## 目录 / Table of Contents

- [命令调度机制](#命令调度机制--command-dispatch)
- [基础命令](#1-基础命令--basic-commands)
  - [/help](#help--帮助)
  - [/status](#status--状态)
  - [/lang](#lang--语言)
  - [/verbose](#verbose--详细)
- [会话管理](#2-会话管理--session-management)
  - [/new](#new--新建)
  - [/stop](#stop--停止)
  - [/interrupt](#interrupt--中断)
  - [/save](#save--保存)
  - [/sessions](#sessions--会话)
  - [/import](#import--导入)
  - [/resume](#resume--恢复)
  - [/loadbg](#loadbg--载入后台)
  - [/bg](#bg--后台)
  - [/fg](#fg--前台)
  - [/rename](#rename--重命名)
  - [/compact](#compact--压缩)
- [模型设置](#3-模型设置--model-settings)
  - [/model](#model--模型)
  - [/fast](#fast--快速)
  - [/context](#context--上下文)
  - [/reasoning](#reasoning--思考)
- [审批流程](#4-审批流程--approval-flow)
  - [/approvals](#approvals--审批)
  - [/approve](#approve--同意)
  - [/approve-session](#approve-session--同意本会话)
  - [/deny](#deny--拒绝)
  - [/cancel (审批)](#cancel--取消)
- [计划模式](#5-计划模式--plan-mode)
  - [/plan](#plan--计划)
  - [/execute-plan](#execute-plan--实施)
  - [/keep-planning](#keep-planning--继续规划)
  - [/cancel-plan](#cancel-plan--取消计划)
- [命令别名](#6-命令别名--command-aliases)
  - [/alias add](#alias-add)
  - [/alias list](#alias-list)
  - [/alias remove](#alias-remove)
- [定时任务](#7-定时任务--scheduler)
  - [/cron list](#cron-list)
  - [/cron pause](#cron-pause)
  - [/cron resume](#cron-resume)
  - [/cron rm](#cron-rm)
  - [/cron run-now](#cron-run-now)
  - [/cron tail](#cron-tail)
- [系统命令](#8-系统命令--system)
  - [/self-update](#self-update--自更新)
  - [/back](#back--返回)
  - [/retry](#retry--重试)
- [交互模式行为规则](#交互模式行为规则--interactive-mode-rules)
- [English Summary](#english-summary)

---

## 命令调度机制 / Command Dispatch

1. 用户在 QQ 中发送 `/command` 或 `/中文命令`。
2. 中文别名由 `canonicalize_core_command()` 规范化为对应的英文命令。
3. 若当前处于交互模式（如模型选择器），非 `/back` 的斜杠命令会自动退出交互模式后再执行；非斜杠文本则由交互模式处理器消费。
4. 若匹配用户自定义别名，展开别名并逐步执行（最大展开深度 3 层）。
5. 若均不匹配，文本作为普通消息发送给 Codex 执行。

### 完整中英文别名映射表

| 中文命令 | 英文命令 |
|---|---|
| `/帮助` | `/help` |
| `/语言` | `/lang` |
| `/模型` | `/model` |
| `/快速` | `/fast` |
| `/上下文` | `/context` |
| `/思考` | `/reasoning` |
| `/详细` | `/verbose` |
| `/审批` | `/approvals` |
| `/计划` | `/plan` |
| `/定时` | `/cron` |
| `/实施` | `/execute-plan` |
| `/继续规划` | `/keep-planning` |
| `/取消计划` | `/cancel-plan` |
| `/同意` | `/approve` |
| `/同意本会话` | `/approve-session` |
| `/拒绝` | `/deny` |
| `/取消` | `/cancel` |
| `/重试` | `/retry` |
| `/状态` | `/status` |
| `/会话` | `/sessions` |
| `/导入` | `/import` |
| `/新建` | `/new` |
| `/后台` | `/bg` |
| `/前台` | `/fg` |
| `/恢复` | `/resume` |
| `/载入后台` | `/loadbg` |
| `/保存` | `/save` |
| `/重命名` | `/rename` |
| `/停止` | `/stop` |
| `/中断` | `/interrupt` |
| `/压缩` | `/compact` |
| `/自更新` | `/self-update` |
| `/别名` | `/alias` |
| `/返回` | `/back` |

---

## 1. 基础命令 / Basic Commands

### /help / 帮助

显示命令帮助列表。

```
/help
/帮助
```

**中文别名：** `/帮助`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 返回按分类组织的完整命令列表，包含基础命令、模型设置、审批设置、会话管理和高级命令等分区。输出内容跟随当前语言设置（中文或英文）。

**示例：**
```
/help
→ 返回完整的命令指南
```

---

### /status / 状态

显示当前会话状态的综合摘要。

```
/status
/状态
```

**中文别名：** `/状态`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 返回以下信息：
- 当前工作目录
- 当前使用的模型（含 override 信息）
- 详细输出开关状态
- 后台会话数量和列表
- 前台状态（运行中/空闲）
- 上下文窗口使用率（百分比、已用 tokens / 总容量）
- 当前界面语言

**示例：**
```
/状态
→ 工作目录: `/home/user/project`
  模型: gpt-5.4
  详细输出: 关闭
  后台会话：无
  前台状态：空闲
  上下文窗口: —（尚无用量数据）
  语言: zh
```

---

### /lang / 语言

切换界面语言。

```
/lang [en|zh|status]
/语言 [en|zh|status]
```

**中文别名：** `/语言`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `en`（英文）、`zh`（中文）、`status`（查看当前设置） |

**行为：**
- 无参数：进入交互式语言选择器（`PendingSetting::Lang`），提示用户输入 `en` 或 `zh`。
- `en` / `zh`：直接切换到指定语言。
- `status`：显示当前语言设置。
- 不支持的语言值：返回错误提示，列出可选值。

**示例：**
```
/lang zh
→ 语言已切换为：zh

/语言 status
→ 当前语言：zh

/lang
→ 当前语言：zh
  可选值：en / zh
```

---

### /verbose / 详细

切换工具调用输出的详细/简略模式。

```
/verbose [on|off|status]
/详细 [on|off|status]
```

**中文别名：** `/详细`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `on`（开启）、`off`（关闭）、`status`（查看当前状态） |

**行为：**
- 无参数：进入交互式选择器。
- `on` / `true`：开启详细输出，Codex 执行过程中的工具调用细节会完整发送。
- `off` / `false`：关闭详细输出。
- `status`：显示当前详细输出状态。

**示例：**
```
/verbose on
→ 详细输出已开启

/详细 status
→ 详细输出：关闭
```

---

## 2. 会话管理 / Session Management

### /new / 新建

创建新的前台会话。

```
/new [工作目录]
/新建 [工作目录]
```

**中文别名：** `/新建`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `工作目录` | 路径字符串 | 否 | 新会话的工作目录。支持绝对路径；相对路径按当前前台工作目录解析 |

**行为：**
1. 若当前前台有内容（已绑定 session_id 或已保存），自动将其转入后台并分配别名。
2. 创建新的临时前台会话。
3. 若指定了工作目录，新会话使用该目录；否则使用默认工作目录。
4. 返回创建确认和当前运行时配置摘要（模型、推理深度等）。

**示例：**
```
/new
→ 已创建新的临时前台会话。

/新建 /home/user/another-project
→ 已将原前台会话转入后台：`bg-1`
  已创建新的临时前台会话。
  工作目录: `/home/user/another-project`
```

---

### /stop / 停止

结束当前前台会话。

```
/stop
/停止
```

**中文别名：** `/停止`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：**
1. 结束当前前台会话：已保存的会话保留，未保存的丢弃。
2. 若有后台会话，自动切回最近的后台会话。
3. 若没有后台会话，创建新的临时前台会话。
4. 若前台本来就没有活跃会话，也会尝试切回后台或重置。

**备注：** 当存在处于进行中的定时任务交互会话时，`/stop` 会优先结束该交互任务并恢复原对话，而非执行常规停止逻辑。

**示例：**
```
/停止
→ 前台会话已结束并保留。已自动切回最近的后台会话 `work`。

/stop
→ 前台会话已结束并丢弃（未保存）。已创建新的临时前台会话。
```

---

### /interrupt / 中断

停止当前正在运行的任务，但不结束会话。

```
/interrupt
/中断
```

**中文别名：** `/中断`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 向当前正在执行的 Codex 回合发送取消信号。会话本身保持不变，可以继续发送新消息。

**示例：**
```
/中断
→ 已请求停止当前运行。
```

---

### /save / 保存

将当前前台会话标记为持久保存。

```
/save
/保存
```

**中文别名：** `/保存`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 标记当前前台会话为 `saved` 状态。已保存的会话在 `/stop` 时会保留而非丢弃。若已经处于保存状态，提示"已处于保存状态"。

**示例：**
```
/保存
→ 前台会话已标记为持久保存。

/save
→ 前台会话已处于保存状态。
```

---

### /sessions / 会话

浏览历史会话，按工作目录（项目）分组。

```
/sessions [all]
/sessions <项目编号> [page]
/会话 [all]
/会话 <项目编号> [page]
```

**中文别名：** `/会话`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `scope` | 字符串 | 否 | 仅支持 `all`（默认也是 all） |
| `项目编号` | 数字 | 否 | 项目列表中的编号，进入对应项目查看会话 |
| `page` | 数字 | 否 | 会话分页页码，默认为 1 |

**行为：**
1. 无参数或 `all`：展示按工作目录分组的项目列表，进入交互模式（`PendingSetting::SessionsProjects`）。
2. 输入项目编号：进入该项目下的会话列表（分页展示），进入交互模式（`PendingSetting::SessionsSessions`）。
3. 在会话列表中可使用 `/恢复` 或 `/载入后台` 操作指定会话。

**示例：**
```
/会话
→ 项目列表 total=3：
  1. /home/user/project-a | sessions=5 | latest=2026-05-10
  2. /home/user/project-b | sessions=2 | latest=2026-05-08
  3. /home/user/project-c | sessions=1 | latest=2026-05-01
  输入项目编号（例如 `1`）进入下一级，或 `/返回` 退出。
```

---

### /import / 导入

从系统 `~/.codex/sessions` 导入宿主 Codex 的会话。

```
/import
/import <编号|会话ID>
/导入
/导入 <编号|会话ID>
```

**中文别名：** `/导入`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `selector` | 编号或会话ID | 否 | 要导入的会话标识 |

**行为：**
1. 无参数：展示 `~/.codex/sessions` 中可导入的项目列表（按工作目录分组），进入交互模式（`PendingSetting::ImportProjects`）。
2. 输入项目编号：钻入查看该项目下可导入的会话列表。
3. 输入会话编号或 ID：执行导入，将会话复制到 `~/.codex-claw/.codex/sessions/`。若会话已存在则刷新导入配置。

**示例：**
```
/导入
→ 可导入项目 total=2
  1. /home/user/project-a | sessions=3 | latest=2026-05-10
  2. /home/user/project-b | sessions=1 | latest=2026-05-05
  输入项目编号进入下一级，或 `/返回` 退出。
```

---

### /resume / 恢复

从磁盘恢复会话到前台。

```
/resume <编号|会话ID>
/恢复 <编号|会话ID>
```

**中文别名：** `/恢复`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `selector` | 编号或会话ID | 是 | 要恢复的会话的列表编号或完整/前缀会话 ID |

**行为：**
1. 将指定的磁盘会话恢复到前台。
2. 原前台会话自动转入后台。
3. 恢复后显示会话摘要和运行时配置。
4. 若恢复失败（如 Codex session resume 出错），进入交互恢复模式（`PendingSetting::ResumeRecovery`），可选 `/重试`、`/新建`、`/取消`。

**备注：** 无参数时进入项目浏览交互模式，与 `/sessions` 类似，可逐级钻入选择会话。

**示例：**
```
/恢复 1
→ 原前台已转入后台：`bg-1`。
  已恢复会话：修复登录bug（workspace: `/home/user/project`）。
```

**恢复失败时：**
```
→ 恢复当前 Codex 会话失败，尚未自动新建线程。
  可选择：`/重试` 再试一次、`/新建` 开始新会话，或 `/取消` 放弃本次恢复。
```

---

### /loadbg / 载入后台

加载磁盘会话到后台。

```
/loadbg <编号|会话ID> [alias]
/载入后台 <编号|会话ID> [alias]
```

**中文别名：** `/载入后台`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `selector` | 编号或会话ID | 是 | 要加载的会话标识 |
| `alias` | 字符串 | 否 | 后台会话的别名标签 |

**行为：** 将磁盘上的会话加载到后台，可选指定别名。不影响当前前台会话。无参数时进入项目浏览交互模式。

**示例：**
```
/载入后台 3 work
→ 已加载会话到后台标签 `work`：重构数据库模块。
```

---

### /bg / 后台

将当前前台会话转入后台。

```
/bg <alias>
/后台 <alias>
```

**中文别名：** `/后台`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `alias` | 字符串 | 是 | 后台会话的别名标签 |

**行为：**
- 将当前前台会话以指定别名存入后台。
- 若前台是空白临时会话（无 session_id、未保存），不转入后台，仅重置为新的临时会话。
- 无参数时提示用法。

**示例：**
```
/后台 work
→ 前台会话已转为后台：`work`。

/bg temp
→ 当前前台是空白临时会话，已重置为新的临时会话。
```

---

### /fg / 前台

将后台会话切到前台。

```
/fg <alias>
/前台 <alias>
```

**中文别名：** `/前台`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `alias` | 字符串 | 否 | 后台会话的别名标签 |

**行为：**
- 指定别名：将对应后台会话切到前台，原前台转入后台。
- 无参数：进入交互式后台会话选择器，列出所有后台会话供选择。若无后台会话，提示"暂无后台会话"。

**示例：**
```
/前台 work
→ 原前台已转入后台：`bg-2`。
  已切换到后台会话 `work`。

/fg
→ 后台会话：
    • `work`
    • `debug`
```

---

### /rename / 重命名

重命名后台会话标签。

```
/rename <old_alias> <new_alias>
/重命名 <旧名> <新名>
```

**中文别名：** `/重命名`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `old_alias` | 字符串 | 是 | 当前后台会话别名 |
| `new_alias` | 字符串 | 是 | 新的别名 |

**行为：** 将指定后台会话的别名从旧名修改为新名。需要恰好两个参数，否则提示用法。

**示例：**
```
/重命名 bg-1 refactor
→ 后台标签已重命名：`bg-1` -> `refactor`
```

---

### /compact / 压缩

手动压缩当前会话上下文。

```
/compact
/压缩
```

**中文别名：** `/压缩`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：**
1. 前提条件：当前无任务运行，且前台有活跃的 Codex 会话（session_id 不为空）。
2. 触发 Codex 的会话压缩功能，将当前对话历史压缩为摘要。
3. 压缩后，后续对话基于压缩摘要继续。
4. 压缩完成后返回成功确认和警告提示。

**备注：** 反复压缩可能降低模型准确性。上下文接近容量上限时（>=80%），系统会自动发出警告建议使用此命令。若条件允许，建议及时 `/新建` 一个更聚焦的新会话。

**示例：**
```
/压缩
→ 开始压缩当前会话上下文，完成后我会再通知你。
  ...
→ 已手动压缩当前会话上下文。后续对话会基于压缩摘要继续。
  提示：对话线程过长、反复 compact 后，模型准确性可能下降。若条件允许，建议及时 `/新建` 一个更聚焦的新会话。
```

**错误场景：**
```
/compact  （任务运行中时）
→ 当前有任务在运行，请先等待当前任务完成后再执行 `/压缩`。

/compact  （无活跃会话时）
→ 当前前台还没有可压缩的 Codex 会话。先发起一轮对话后再执行 `/压缩`。
```

---

## 3. 模型设置 / Model Settings

> **全局 vs 会话级设置：** 当前台会话为未保存的临时会话时，模型、快速模式、上下文、思考深度的修改写入全局运行时配置文件（`config.toml`）。当前台会话已保存（`saved=true`）时，修改仅作用于当前会话。

### /model / 模型

设置或查看当前使用的模型。

```
/model [name|inherit|status]
/模型 [name|inherit|status]
```

**中文别名：** `/模型`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | 模型名称、`inherit`（恢复配置默认值）、`status`（查看当前状态） |

**行为：**
- 无参数：进入交互式模型选择器，显示当前模型和所有可选模型（含别名和描述）。
- 指定模型名：设置为该模型。支持模型别名匹配。
- `inherit` / `default`：清除 override，恢复为配置文件默认模型。
- `status`：显示当前生效模型和 override 值。

**示例：**
```
/模型
→ **当前模型：** gpt-5.4
  **可选模型：**
  - `gpt-5.4`
    - 默认模型
  - `o3`
    - 别名：o3-mini
  ...

/model o3
→ 模型已更新为：o3

/模型 status
→ model: gpt-5.4    override: inherit
```

---

### /fast / 快速

设置快速推理模式（Fast service tier）。

```
/fast [on|off|inherit|status]
/快速 [on|off|inherit|status]
```

**中文别名：** `/快速`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `on`（开启）、`off`（关闭）、`inherit`（恢复默认）、`status`（查看状态） |

**行为：**
- 无参数：进入交互式选择器。
- `on`：启用 Fast service tier。
- `off`：使用 Flex tier。
- `inherit`：清除 override，恢复默认。
- `status`：显示当前 fast 设置值。
- 设置始终写入全局运行时配置（`SetGlobalFast`）。

**示例：**
```
/快速 on
→ fast 已更新为：on

/fast status
→ fast: off
```

---

### /context / 上下文

设置上下文窗口模式。

```
/context [standard|1m|inherit|status]
/上下文 [standard|1m|inherit|status]
```

**中文别名：** `/上下文`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `standard`（标准 272K tokens）、`1m` / `1M`（长上下文 1M tokens）、`inherit`（恢复默认）、`status`（查看状态） |

**行为：**
- 无参数：进入交互式选择器。
- `standard`：设置为标准上下文窗口（272K tokens）。
- `1m` / `1M`：设置为长上下文窗口（1M tokens）。
- `inherit`：清除 override，恢复默认。
- `status`：显示当前上下文模式。
- 已保存会话为会话级设置，未保存会话为全局设置（`SetGlobalContext`）。

**示例：**
```
/上下文 1m
→ 上下文模式已更新为：1M

/context status
→ 上下文: 272K
```

---

### /reasoning / 思考

设置推理（思考）深度。

```
/reasoning [low|medium|high|xhigh|inherit|status]
/思考 [low|medium|high|xhigh|inherit|status]
```

**中文别名：** `/思考`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `low`（低）、`medium`（中）、`high`（高）、`xhigh`（超高）、`inherit`（恢复默认）、`status`（查看状态） |

**行为：**
- 无参数：进入交互式选择器。
- 设置推理深度，影响模型的思考 token 分配。
- 已保存会话为会话级设置，未保存会话为全局设置（`SetGlobalReasoning`）。

**示例：**
```
/思考 high
→ 思考深度已更新为：high

/reasoning status
→ 思考深度: medium
```

---

## 4. 审批流程 / Approval Flow

当 Codex 需要执行 shell 命令、写入/修改文件或请求权限升级时，会通过 QQ 消息发送审批请求，等待用户决策。

### /approvals / 审批

查看和切换执行审批策略。

```
/approvals [on|strict|off|status]
/审批 [on|strict|off|status]
```

**中文别名：** `/审批`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `on`（按需审批，默认）、`strict`（unless-trusted 严格模式）、`off`（关闭审批）、`status`（查看当前策略） |

**行为：**
- 无参数：显示当前策略并进入交互选择器（`PendingSetting::Approvals`）。
- `on` / `on-request`：按需审批（默认）。
- `strict` / `unless-trusted`：严格模式，所有操作都需审批。
- `off` / `never`：关闭审批，自动放行所有操作。
- `status`：仅显示当前策略。

**示例：**
```
/审批
→ 当前审批策略：按需（on-request，默认）
  回复以下选项切换：
  /approvals on           按需审批（默认）
  /approvals strict       严格（unless-trusted）
  /approvals off          关闭审批

/approvals strict
→ 已切换审批策略：严格（unless-trusted）
```

---

### /approve / 同意

放行当前挂起的审批请求（仅本次）。

```
/approve
/同意
```

**中文别名：** `/同意`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 批准当前最早的挂起审批请求。仅本次有效，下次同类操作仍需审批。若无挂起的审批请求，提示"当前没有待处理的审批请求"。

**示例：**
```
（Codex 请求执行 shell 命令）
/同意
→ 已放行本次请求。
```

---

### /approve-session / 同意本会话

在当前会话中自动放行同类命令。

```
/approve-session
/同意本会话
```

**中文别名：** `/同意本会话`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 批准当前审批请求，且在当前会话中自动放行后续同类命令。

**示例：**
```
/同意本会话
→ 已放行本次请求，后续类似命令将自动放行。
```

---

### /deny / 拒绝

拒绝当前审批请求。

```
/deny
/拒绝
```

**中文别名：** `/拒绝`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 拒绝当前审批请求。Codex 会尝试用其他方式完成任务。

**示例：**
```
/拒绝
→ 已拒绝本次请求。
```

---

### /cancel / 取消

拒绝审批请求并终止当前回合。

```
/cancel
/取消
```

**中文别名：** `/取消`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 拒绝当前审批请求，并要求 Codex 终止当前执行回合。比 `/deny` 更强——不仅拒绝，还中止整个回合。

**备注：** 在恢复失败交互模式中，`/cancel` 的行为是取消恢复流程（清除 `PendingSetting::ResumeRecovery`），而非处理审批。

**示例：**
```
/取消
→ 已拒绝并要求终止当前回合。
```

---

## 5. 计划模式 / Plan Mode

计划模式下，Codex 以只读方式运行，先制定计划再执行。

### /plan / 计划

进入或退出计划模式。

```
/plan [on|off|status]
/计划 [on|off|status]
```

**中文别名：** `/计划`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `value` | 字符串 | 否 | `on`（开启）、`off`（关闭）、`status`（查看状态）。也支持中文：`开`/`开启`/`关`/`关闭` |

**行为：**
- 无参数：显示当前 Plan 模式状态并进入交互选择器（`PendingSetting::Plan`）。
- `on`：进入 Plan 模式。Codex 将在只读沙箱中先制定计划，然后发出 `<proposed_plan>` 块。
- `off`：退出 Plan 模式，恢复默认执行模式。
- 当 Codex 产出 `<proposed_plan>` 后，系统自动提示用户使用 `/实施`、`/继续规划` 或 `/取消计划`。

**示例：**
```
/计划 on
→ 已进入 Plan 模式。Codex 将在只读沙箱中先制定计划，随后发 <proposed_plan>，你可以用 /实施 批准执行。
```

---

### /execute-plan / 实施

审批通过待执行计划，开始执行。

```
/execute-plan
/实施
```

**中文别名：** `/实施`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：**
1. 退出 Plan 模式（`plan_mode = false`）。
2. 清除 `pending_plan`。
3. 提示用户发送后续消息（如 "开始"）触发按计划执行。
4. 若无待执行计划，提示"当前没有待执行的计划"。

**示例：**
```
/实施
→ 已退出 Plan 模式并批准计划。你可以直接回复 "开始" 或描述下一步，我会按计划执行。
```

---

### /keep-planning / 继续规划

留在计划模式继续优化计划。

```
/keep-planning
/继续规划
```

**中文别名：** `/继续规划`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 保持 Plan 模式开启，清除当前 `pending_plan`。下一条消息继续规划迭代。

**示例：**
```
/继续规划
→ 已保留 Plan 模式，继续打磨计划。下一条消息将继续规划。
```

---

### /cancel-plan / 取消计划

丢弃当前待执行计划。

```
/cancel-plan
/取消计划
```

**中文别名：** `/取消计划`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 清除 `pending_plan`，但不改变 Plan 模式开关状态。

**示例：**
```
/取消计划
→ 已丢弃当前计划。
```

---

## 6. 命令别名 / Command Aliases

用户可以创建自定义命令别名，支持多步管道式执行。

### /alias add

创建命令别名。

```
/alias add <名称> <命令1> | <命令2> | ...
/别名 add <名称> <命令1> | <命令2> | ...
```

**中文别名：** `/别名`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `名称` | 字符串 | 是 | 别名名称。规则：1-20 字符，不含 `\|`，不能以 `/` 开头 |
| `命令...` | 字符串 | 是 | 一个或多个子命令，用 `\|` 分隔 |

**行为：**
- 创建一个命令别名，调用时依次执行所有子命令。
- 别名不能与内置命令名（包括中英文）冲突。
- 展开深度上限 3 层（防止递归别名导致无限循环）。
- 展开时非命令文本会被跳过（标记为"已跳过：不是命令"）。

**示例：**
```
/alias add setup /model o3 | /reasoning high | /context 1m
→ 别名 `/setup` 已注册，包含 3 步

/setup
→ 别名 `/setup` 执行结果：
  模型已更新为：o3
  思考深度已更新为：high
  上下文模式已更新为：1M
```

---

### /alias list

列出所有已注册别名。

```
/alias list
/别名 list
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 列出当前用户注册的所有别名及其子命令。无别名时提示创建方式。也支持 `ls` 作为 `list` 的缩写。

**示例：**
```
/alias list
→ 已注册别名：
    /setup → /model o3 | /reasoning high | /context 1m
    /quick → /fast on | /reasoning low
```

---

### /alias remove

删除命令别名。

```
/alias remove <名称>
/别名 remove <名称>
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `名称` | 字符串 | 是 | 要删除的别名名称 |

**行为：** 删除指定别名。别名不存在时提示"不存在"。也支持 `rm`、`delete`、`del` 作为 `remove` 的缩写。

**示例：**
```
/alias remove setup
→ 别名 `/setup` 已删除
```

---

## 7. 定时任务 / Scheduler

通过 QQ 管理自己的定时任务。任务由 CodexClaw 的调度器引擎在后台执行。

### /cron list

列出自己的定时任务。

```
/cron list
/定时 list
```

**中文别名：** `/定时 list`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：** 列出当前用户拥有的所有定时任务，显示 job_id、启用状态、下次运行时间、运行次数、失败次数和标题。也支持 `ls` 作为 `list` 的缩写。

**示例：**
```
/cron list
→ 你的定时任务：
  abc123  enabled  next=2026-05-12T08:00:00Z  runs=5  failures=0  每日检查
  def456  disabled next=-                      runs=12 failures=2  周报
```

---

### /cron pause

暂停指定定时任务。

```
/cron pause <job_id>
/定时 pause <job_id>
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `job_id` | 字符串 | 是 | 要暂停的任务 ID |

**行为：** 将指定任务标记为 `disabled`。只能管理自己的任务。

**示例：**
```
/cron pause abc123
→ 已暂停 `每日检查`。
```

---

### /cron resume

恢复已暂停的定时任务。

```
/cron resume <job_id>
/定时 resume <job_id>
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `job_id` | 字符串 | 是 | 要恢复的任务 ID |

**行为：** 将指定任务恢复为 `enabled`，并重新计算下次运行时间。对于一次性任务（OneShot）若已过期，会安排立即运行。

**示例：**
```
/cron resume abc123
→ 已恢复 `每日检查`。
```

---

### /cron rm

删除定时任务。

```
/cron rm <job_id>
/定时 rm <job_id>
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `job_id` | 字符串 | 是 | 要删除的任务 ID |

**行为：** 删除指定任务及其关联文件。只能删除自己的任务。也支持 `remove` 作为 `rm` 的同义词。

**示例：**
```
/cron rm def456
→ 已删除 `周报`。
```

---

### /cron run-now

立即触发一次定时任务。

```
/cron run-now <job_id>
/定时 run-now <job_id>
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `job_id` | 字符串 | 是 | 要立即触发的任务 ID |

**行为：** 在下一个调度 tick 立即执行一次指定任务，不影响正常的 cron 计划。

**示例：**
```
/cron run-now abc123
→ 已安排 `每日检查` 立即运行一次。
```

---

### /cron tail

查看最近一次运行日志。

```
/cron tail <job_id>
/定时 tail <job_id>
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `job_id` | 字符串 | 是 | 要查看日志的任务 ID |

**行为：** 读取指定任务最近一次运行的日志文件，返回最后 3500 字符的内容。若无运行记录，提示"还没有运行日志"。

**示例：**
```
/cron tail abc123
→ 最近运行日志 `/path/to/runs/2026-05-11T080000Z.log`：
  [日志内容...]
```

---

## 8. 系统命令 / System

### /self-update / 自更新

构建并部署最新版本。

```
/self-update
/自更新
```

**中文别名：** `/自更新`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：**
1. 检查当前是否有任务在运行，若有则拒绝执行。
2. 在仓库目录执行构建命令（默认 `cargo build --release`）。
3. 构建成功后，用新编译的二进制替换当前运行中的可执行文件。
4. 通知 codex app-server 关闭。
5. 退出当前进程（`exit(0)`）。由外部服务管理器（如 systemd、launchd）负责重新拉起。

**备注：** 这是一个破坏性操作，进程会立即退出。确保配置了外部守护服务以实现自动重启。

**示例：**
```
/自更新
→ 已覆盖运行中的二进制：`/home/user/.codex-claw/bin/codex-claw`
  即将退出当前进程（已通知 codex app-server 关闭）。若已配置外部守护服务，将自动重启；否则请手动重新启动。
```

---

### /back / 返回

退出当前交互式设置。

```
/back
/返回
```

**中文别名：** `/返回`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：**
- 在交互模式中（如模型选择器、语言选择器、会话浏览等）：退出交互模式，清除 `pending_setting`。
- 不在交互模式时：提示"当前没有进行交互式设置"。

**示例：**
```
（在模型选择器中）
/返回
→ 已退出 `模型` 设置。

（无交互模式时）
/back
→ 当前没有进行交互式设置。
```

---

### /retry / 重试

在恢复失败交互模式中重试恢复。

```
/retry
/重试
```

**中文别名：** `/重试`

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| （无） | — | — | — |

**行为：**
- 在恢复失败交互模式（`PendingSetting::ResumeRecovery`）中：清除 pending 状态，重新尝试恢复上次失败的会话。
- 不在该模式时：提示"当前没有待恢复的失败会话"。

**示例：**
```
（恢复失败后）
/重试
→ （重新尝试恢复）

（无失败恢复时）
/retry
→ 当前没有待恢复的失败会话。
```

---

## 交互模式行为规则 / Interactive Mode Rules

以下命令在无参数时会进入交互式设置模式：`/model`、`/fast`、`/context`、`/reasoning`、`/verbose`、`/lang`、`/approvals`、`/plan`、`/sessions`、`/import`、`/fg`、`/resume`、`/loadbg`。

在交互模式下的行为规则：

1. **非斜杠文本**被交互处理器消费，用于匹配选项、输入值等。不会转发给 Codex。
2. **`/back`**（或 `/返回`）退出交互模式，清除 pending 状态。
3. **其他斜杠命令**会隐式退出当前交互模式（清除 pending 状态并发送"已退出 `XXX` 设置"通知），然后执行该命令。
4. 交互模式下的模糊匹配：输入的值会模糊匹配选项列表。匹配到唯一项则应用，匹配多个则提示"请更精确"，匹配不到则提示"没有匹配项"。

---

## English Summary

CodexClaw is a Rust-based QQ bot powered by OpenAI Codex. Users interact with it via slash commands in QQ direct messages. Every command has a Chinese alias that is canonicalized to the English form at dispatch time.

### Command Categories

**Basic:** `/help`, `/status`, `/lang`, `/verbose` -- show help, session status, switch UI language, toggle verbose output.

**Session Management:** `/new`, `/stop`, `/interrupt`, `/save`, `/sessions`, `/import`, `/resume`, `/loadbg`, `/bg`, `/fg`, `/rename`, `/compact` -- create, end, browse, restore, background/foreground, and compress sessions.

**Model Settings:** `/model`, `/fast`, `/context`, `/reasoning` -- configure the model name, fast service tier, context window size (272K/1M), and reasoning depth (low/medium/high/xhigh). Changes apply globally when the foreground session is unsaved, or per-session when saved.

**Approval Flow:** `/approvals`, `/approve`, `/approve-session`, `/deny`, `/cancel` -- configure the approval policy and respond to pending approval requests for shell commands, file writes, and permission escalations.

**Plan Mode:** `/plan`, `/execute-plan`, `/keep-planning`, `/cancel-plan` -- enter a read-only planning mode where Codex proposes a plan before execution, then approve/refine/discard the plan.

**Command Aliases:** `/alias add|list|remove` -- create multi-step command aliases with pipe-separated sub-commands. Max expansion depth is 3. Built-in command names cannot be overridden.

**Scheduler:** `/cron list|pause|resume|rm|run-now|tail` -- manage personal cron jobs from QQ. Only the job owner can manage their own jobs.

**System:** `/self-update` (build and hot-swap the binary), `/back` (exit interactive mode), `/retry` (retry a failed session resume).

### Interactive Mode

When invoked without arguments, many commands enter an interactive picker. In this mode, plain text is consumed by the picker; `/back` exits; other slash commands implicitly exit the picker first.

### Key Behaviors

- **Busy guard:** Only one Codex turn runs at a time. Extra messages are rejected with a "still processing" notice.
- **Context warnings:** When context usage exceeds 80%, a warning is appended suggesting `/compact`.
- **Auto-build on self-modification:** If a Codex turn modifies CodexClaw's own source code, an automatic build is triggered.
- **Resume recovery:** If session resumption fails, the user enters a recovery flow with `/retry`, `/new`, or `/cancel` options.
