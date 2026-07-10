# AgentIsOK

> AI 编码工具桌面审批与用量驾驶舱 — 悬浮、本地、无追踪。

AgentIsOK 是一个常驻桌面的**浮动小岛**。它能拦截编码 Agent 的权限请求，追踪额度、余额，并在 Home 中展示今日本地 token 用量。

<p align="center">
  <img src="assets/images/small.png" alt="AgentIsOK 收起状态" width="360" />
</p>

<p align="center">
  <img src="assets/images/home.png" alt="AgentIsOK Home：Provider 状态和 token 用量" width="31%" />
  <img src="assets/images/usage.jpeg" alt="AgentIsOK Usage：Provider 开关和额度卡片" width="31%" />
  <img src="assets/images/rules.png" alt="AgentIsOK Rules：规则筛选和删除" width="31%" />
</p>

## 功能

- **权限审批** — 批准、创建允许规则、回答 ask prompt、拒绝，都不用切回终端
- **Home 驾驶舱** — Provider 健康状态、运行中的 Agent、精确/估算 token、今日本地活动集中展示
- **用量追踪** — 实时进度条显示 5h / 周 / 月额度、credits、余额和重置时间
- **Token 统计** — Claude、Codex、OpenCode 在本地日志暴露 token 时显示精确今日用量，并展示 Antigravity 本地活动
- **规则管理** — 搜索、按来源筛选、长命令预览、删除规则、3 秒撤销
- **始终置顶** — 可拖动的半透明悬浮岛，不会被其他窗口遮挡

## 支持的 Provider

### 审批（Hook 桥接）

| Agent | 状态 | 配置方式 |
|-------|------|---------|
| Claude Code | ✅ | 启动时自动注入 |
| Codex | ✅ | 启动时自动注入 |
| Antigravity | ✅ | 自动注入到 `~/.gemini/config/hooks.json` |
| OpenCode | ✅ | 需手动安装插件（[见下方](#opencode-插件)） |

### 用量 & 余额

| Provider | 追踪内容 | Token 数据 | 数据来源 |
|----------|---------|------------|---------|
| Codex | 5h / 7d 频率限制 | 从 session `token_count` 精确统计今日 token | 本地认证 + session 文件 |
| Claude Code | 今日消息 / 会话 / 工具调用 | 从 JSONL transcript 精确统计今日 token | 本地 Claude 数据 |
| Antigravity | 本地 agent calls / 会话 / 当前模型 | 未暴露 | 本地 Antigravity 日志（`~/.gemini/antigravity*`） |
| OpenCode Go | $12 / $30 / $60 配额 | 从 SQLite session/message 精确统计今日 token | 本地 SQLite（`opencode.db`）|
| OpenCode Zen | 模型可用性 | 未暴露 | OpenCode API key |
| Kiro | Credits | 未暴露 | 本地 Kiro DB |
| DeepSeek | API 余额 | 未暴露 | `DEEPSEEK_API_KEY` |
| MiniMax | Token 套餐 prompt 余额 | 未暴露 | `MINIMAX_API_KEY` |

## 安装

### 发行版（推荐）

从 [Releases](https://github.com/JaikenWong/AgentIsOK/releases) 下载最新 `.dmg`（macOS）或 `.exe`（Windows）。

**macOS 用户注意：** 应用未经过 Apple 公证，安装后需运行一次：

```bash
xattr -cr /Applications/AgentIsOK.app
```

或在 Finder 中右键点击 app → **打开**。

### 从源码编译

```bash
git clone https://github.com/JaikenWong/AgentIsOK.git
cd AgentIsOK
npm install
npm run tauri:dev     # 开发
npm run tauri:build   # 生产构建 → src-tauri/target/release/bundle/
```

**前置依赖：** Node.js 18+、Rust 工具链、平台构建工具（macOS 需 Xcode，Windows 需 MSVC）。

## 使用

### 悬浮岛

| 视图 | 显示内容 |
|------|---------|
| **收起** | Logo + provider 紧凑状态。点击打开完整面板。 |
| **Home** | 活动 Agent 会话、Provider 健康状态、token 摘要、时间线详情、终端跳转目标。 |
| **Usage** | Provider 显隐开关、同步间隔、版本/更新检查、额度卡片、余额和重置时间。 |
| **Rules** | 可搜索允许规则列表，支持来源筛选、命令预览、删除图标和撤销。 |

### 全局快捷键

| 快捷键 | 功能 |
|--------|------|
| `Ctrl/Cmd+Shift+Space` | 显示/隐藏悬浮岛 |
| `Ctrl/Cmd+Opt+A` | 批准 |
| `Ctrl/Cmd+Opt+L` | 永久批准 |
| `Ctrl/Cmd+Opt+D` | 拒绝 |

### 托盘菜单

右键托盘图标（macOS 菜单栏 / Windows 系统托盘）可 **打开**、**立即同步**、**安装 Hooks**、**移除 Hooks**、查看更新状态、**退出**。

## Hook 工作原理

启动时，AgentIsOK 会向以下文件写入受管 hook 条目：

- `~/.claude/settings.json`
- `~/.codex/hooks.json`
- `~/.gemini/config/hooks.json`（Antigravity）

当工具使用权限被请求时，agent 以 `--hook-source` 和 `--hook-event` 参数调用 AgentIsOK 可执行文件。本地 TCP 服务（`127.0.0.1:45873`）接收事件，显示审批面板，并返回决定。

### OpenCode 插件

将 `src-tauri/plugins/agentisok-opencode.js` 复制到 `~/.config/opencode/plugins/`，然后在 `~/.config/opencode/config.json` 中添加：

```json
{ "plugin": ["file:///Users/你的用户名/.config/opencode/plugins/agentisok-opencode.js"] }
```

## 配置

- **同步间隔** — 展开悬浮岛，点击设置行的 `+/-`（5 / 10 / 15 / 30 / 60 分钟）
- **Provider 显隐** — Usage 视图中的开关；关闭的 provider 不出现在 Home 和收起状态
- **审批规则** — "Allow Rule / 允许规则" 会创建持久规则，保存在 `~/.config/AgentIsOK/approval-rules.json`
- **Hooks** — 可在托盘菜单安装/移除受管 hooks，用于临时关闭 Agent 拦截
- **隐藏程序坞** — macOS：应用以辅助模式运行，仅显示托盘图标。Windows：默认不显示任务栏图标。

## 隐私

- 所有数据**本地存储** — 无遥测、无云同步、无分析
- Provider 凭据仅从标准位置读取（`.codex/auth.json`、`~/.local/share/opencode/auth.json`、`DEEPSEEK_API_KEY` 等），**仅向相应 provider 官方 API 发送余额查询请求**
- Hook 事件在本地 TCP 处理，立即丢弃

## 常见问题

| 问题 | 检查 |
|------|------|
| Provider 显示 "Stale" | 重新登录对应 provider，然后点 **Sync** |
| 没有额度进度条 | Provider 可能需要本地登录才有数据 — 悬停 `?` 查看配置说明 |
| Token 显示 `--` | 该 provider 没有暴露本地 token 记录，或今天还没同步到数据 |
| Hook 不生效 | 先启动 AgentIsOK，再启动编码工具 |
| 小岛不显示 | `Ctrl/Cmd+Shift+Space` 切换可见性；检查托盘图标 |

## 技术栈

- **桌面框架:** [Tauri 2](https://tauri.app)（Rust + webview）
- **前端:** 原生 JS + CSS，透明 webview
- **存储:** 本地 JSON 文件 + SQLite（OpenCode Go 历史）
- **IPC:** Tauri commands + 本地 TCP（hook 桥接）

## 平台

| 平台 | 状态 |
|------|------|
| Windows | 主要目标 |
| macOS | 完全支持 |
| Linux | 未测试（欢迎贡献）|

## License

ISC

---

[English README](./README.md) · [Windows 检查清单](./docs/windows-validation-checklist.md)
