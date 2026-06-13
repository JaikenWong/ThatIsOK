# ThatIsOK

> AI 编码工具桌面审批与用量驾驶舱 — 悬浮、本地、无追踪。

ThatIsOK 是一个常驻桌面的**浮动小岛**。它能拦截 Claude Code 和 Codex 的权限请求，同时一站式追踪所有 AI 编码工具的用量和余额。

<p align="center">
  <em>截图即将补充 — 收起时显示 provider 进度圈，展开后展示完整面板：开关、进度条、会话日志。</em>
</p>

## 功能

- **权限审批** — 批准 / 永久批准 / 拒绝，不用切回终端
- **用量追踪** — 实时进度条显示各 provider 的 session / 周 / 月配额
- **余额同步** — 通过 provider API 拉取最新数据，快用完时颜色变红警示
- **始终置顶** — 可拖动的半透明悬浮岛，不会被其他窗口遮挡

## 支持的 Provider

### 审批（Hook 桥接）

| Agent | 状态 | 配置方式 |
|-------|------|---------|
| Claude Code | ✅ | 启动时自动注入 |
| Codex | ✅ | 启动时自动注入 |
| OpenCode | ✅ | 需手动安装插件（[见下方](#opencode-插件)） |

### 用量 & 余额

| Provider | 追踪内容 | 数据来源 |
|----------|---------|---------|
| Codex | 5h / 7d 频率限制 | 本地认证 + session 文件 |
| Claude Code | 会话费用 | 本地 JSONL transcript |
| Cursor | 用量摘要 | 本地应用存储 |
| Gemini | 用量数据 | 本地登录 + session 数据 |
| DeepSeek | API 余额 | `DEEPSEEK_API_KEY` |
| MiniMax | Token 套餐余额 | `MINIMAX_API_KEY` |
| OpenCode Go | $12/$30/$60 配额 | 本地 SQLite（`opencode.db`）|
| OpenCode Zen | 模型可用性 | OpenCode API key |

## 安装

### 发行版（推荐）

从 [Releases](https://github.com/anomalyco/ThatIsOK/releases) 下载最新 `.dmg`（macOS）或 `.exe`（Windows）。

### 从源码编译

```bash
git clone https://github.com/anomalyco/ThatIsOK.git
cd ThatIsOK
npm install
npm run tauri:dev     # 开发
npm run tauri:build   # 生产构建 → src-tauri/target/release/bundle/
```

**前置依赖：** Node.js 18+、Rust 工具链、平台构建工具（macOS 需 Xcode，Windows 需 MSVC）。

## 使用

### 悬浮岛

| 模式 | 显示内容 |
|------|---------|
| **收起** | Logo + provider 进度圈。每个圈代表一个 provider，填充比例 = 已用额度。悬停圆圈看具体数字，悬停 `?` 看配置提示。 |
| **展开** | 点击小岛展开。显示开关面板（显示/隐藏 provider）、各 provider 详细进度条（含美元金额和重置时间）、会话日志、同步间隔设置。点击外部区域或开关收起。 |

### 全局快捷键

| 快捷键 | 功能 |
|--------|------|
| `Ctrl/Cmd+Shift+Space` | 显示/隐藏悬浮岛 |
| `Ctrl/Cmd+Opt+A` | 批准 |
| `Ctrl/Cmd+Opt+L` | 永久批准 |
| `Ctrl/Cmd+Opt+D` | 拒绝 |

### 托盘菜单

右键托盘图标（macOS 菜单栏 / Windows 系统托盘）可 **打开**、**立即同步**、**退出**。

## Hook 工作原理

启动时，ThatIsOK 会向以下文件写入受管 hook 条目：

- `~/.claude/settings.json`
- `~/.codex/hooks.json`

当工具使用权限被请求时，agent 以 `--hook-source` 和 `--hook-event` 参数调用 ThatIsOK 可执行文件。本地 TCP 服务（`127.0.0.1:45873`）接收事件，显示审批面板，并返回决定。

### OpenCode 插件

将 `src-tauri/plugins/thatisok-opencode.js` 复制到 `~/.config/opencode/plugins/`，然后在 `~/.config/opencode/config.json` 中添加：

```json
{ "plugin": ["file:///Users/你的用户名/.config/opencode/plugins/thatisok-opencode.js"] }
```

## 配置

- **同步间隔** — 展开悬浮岛，点击设置行的 `+/-`（5 / 10 / 15 / 30 / 60 分钟）
- **Provider 显隐** — 展开面板中的开关；关闭的 provider 不出现在圈和列表
- **审批规则** — "永久批准" 会创建持久规则，保存在 `~/.config/ThatIsOK/approval-rules.json`
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
| 进度圈显示不全 | 减少可见 provider 数量，收起模式最多显示 5 个圈 |
| Hook 不生效 | 先启动 ThatIsOK，再启动编码工具 |
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
