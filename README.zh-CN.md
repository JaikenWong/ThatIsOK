# ThatIsOk

ThatIsOk 是一个面向 AI 编码工具的本地桌面审批与用量驾驶舱。它会在桌面顶部常驻一个小型悬浮岛，用来处理权限确认，同时汇总多个 provider 的使用情况，并通过本地 HTTP API 暴露状态数据。

## 解决的问题

- 权限确认不必频繁切回终端
- 多个 AI 编码工具和 provider 的用量、余额统一查看
- 本地状态可被面板或集成工具读取
- 数据全部本地存储，无遥测

## 核心能力

- 顶部悬浮审批岛，支持收起和展开两种形态
- 支持全局快捷键批准、永久批准、拒绝
- 支持 Claude Code、Codex、Gemini 的 hook 桥接
- 支持 Codex、Claude Code、Cursor、MiniMax、Gemini、DeepSeek 的用量或余额同步
- 支持托盘、会话跟踪、provider 显隐切换、本地 API

## 平台现状

- 主要目标平台：Windows
- 同时支持：macOS
- 当前主桌面运行时：Tauri 2
- `main/` 和 `bin/` 中仍保留旧 Electron 实现，但不再是主路径

## 支持情况

### Hook / 审批支持

| Agent | 状态 |
| --- | --- |
| Claude Code | 支持 |
| Codex | 支持 |
| Gemini | 支持 |

### 用量 / 余额同步

| Provider | 数据来源 |
| --- | --- |
| Codex | 本地认证和 session 数据 |
| Claude Code | 本地 JSONL transcript |
| Cursor | 本地应用存储 |
| MiniMax | 本地 / API 用量抓取 |
| Gemini | 本地登录和 session 数据 |
| DeepSeek | API 余额 |

## 快速开始

```bash
npm install
npm run tauri:dev
```

旧 Electron 启动方式：

```bash
node bin/cli.js
```

## 构建

```bash
npm run tauri:build
```

旧 Electron 打包命令仍保留：`npm run build`、`npm run build:win`、`npm run build:mac`。

## 全局快捷键

- `Ctrl/Cmd+Shift+Space`：切换悬浮岛
- `Ctrl/Cmd+Shift+A`：批准
- `Ctrl/Cmd+Shift+L`：永久批准
- `Ctrl/Cmd+Shift+D`：拒绝

## 本地 API

基础地址：`http://127.0.0.1:45874`

| Endpoint | 说明 |
| --- | --- |
| `GET /api/health` | 健康检查 |
| `GET /api/usage` | 完整仪表盘数据 |
| `GET /api/overview` | 总余额、今日、本月、runway |
| `GET /api/accounts` | provider / account 列表 |
| `GET /api/intervention` | 当前待处理审批 |

## 项目结构

```text
ThatIsOk/
|- src-tauri/    当前 Tauri 2 应用运行时与后端
|- renderer/     Tauri webview 加载的悬浮岛 UI
|- main/         旧 Electron 主进程
|- bridge/       旧独立 hook bridge CLI
|- shared/       hook 归一化与 IPC 配置
|- config/       providers、defaults、models
`- bin/          辅助脚本与旧 CLI 入口
```

## Hook 注入

应用启动后会向以下配置写入受管 hook：

- `~/.claude/settings.json`
- `~/.codex/hooks.json`

现在这些受管 hook 会直接指向打包后的 ThatIsOk 可执行文件，并附带 `--hook-source` 和 `--hook-event`。
`bridge/hook-bridge.js` 只给旧 Electron 路径和旧冒烟测试保留。

## 说明

- 当前运行时状态和同步逻辑集中在 `src-tauri/src/lib.rs`
- hook 本地 IPC 仍使用 `127.0.0.1:45873` 或 Windows named pipe
- `renderer/dashboard/` 已删除，因为它已不再接入任何运行时
- `npm run test:hook` 目前仍走旧 bridge 冒烟路径

## License

ISC
