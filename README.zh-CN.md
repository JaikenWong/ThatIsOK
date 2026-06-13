# ThatIsOK

ThatIsOK 是一个面向 AI 编码工具的本地桌面审批与用量驾驶舱。它会在桌面顶部常驻一个小型悬浮岛，用来处理权限确认，同时汇总多个 provider 的使用情况。

## 解决的问题

- 权限确认不必频繁切回终端
- 多个 AI 编码工具和 provider 的用量、余额统一查看
- 数据全部本地存储，无遥测

## 核心能力

- 顶部悬浮审批岛，支持收起和展开两种形态
- 支持全局快捷键批准、永久批准、拒绝
- 支持 Claude Code、Codex 的 hook 桥接
- 支持 Codex、Claude Code、Cursor、MiniMax、Gemini、DeepSeek 的用量或余额同步
- 支持托盘、会话跟踪、provider 显隐切换

## 平台现状

- 主要目标平台：Windows
- 同时支持：macOS
- 当前主桌面运行时：Tauri 2

## 支持情况

### Hook / 审批支持

| Agent | 状态 |
| --- | --- |
| Claude Code | 支持 |
| Codex | 支持 |

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

## 构建

```bash
npm run tauri:build
```

## 全局快捷键

- `Ctrl/Cmd+Shift+Space`：切换悬浮岛
- `Ctrl/Cmd+Opt+A`：批准
- `Ctrl/Cmd+Opt+L`：永久批准
- `Ctrl/Cmd+Opt+D`：拒绝

## 项目结构

```text
ThatIsOK/
|- src-tauri/    当前 Tauri 2 应用运行时与后端
|- renderer/     Tauri webview 加载的悬浮岛 UI
|- config/       providers、defaults
`- assets/       应用图标与静态资源
```

## Hook 注入

应用启动后会向以下配置写入受管 hook：

- `~/.claude/settings.json`
- `~/.codex/hooks.json`

现在这些受管 hook 会直接指向打包后的 ThatIsOK 可执行文件，并附带 `--hook-source` 和 `--hook-event`。

## 说明

- 运行时代码已拆分到 `src-tauri/src/` 下的 hooks、shortcuts、providers、UI state 等模块
- hook 本地 IPC 使用 `127.0.0.1:45873`
- `renderer/dashboard/` 已删除，因为它已不再接入当前运行时

## 文档

- Windows 发布检查清单：[docs/windows-validation-checklist.md](./docs/windows-validation-checklist.md)

## License

ISC
