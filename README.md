# ThatIsOk

ThatIsOk is a local desktop approval and usage cockpit for AI coding tools. It keeps a small floating island on screen for permission decisions, tracks provider usage across local agents, and exposes a local API for dashboards or integrations.

## Why it exists

- Keep approval prompts visible without alt-tabbing back to the terminal
- Track usage and balance across multiple coding agents and providers in one place
- Expose local state through a simple HTTP API
- Store everything locally with no telemetry

## Highlights

- Floating always-on-top approval island with compact and expanded modes
- Approve, approve always, or deny with global shortcuts
- Hook bridge support for Claude Code, Codex, and Gemini
- Usage and balance sync for Codex, Claude Code, Cursor, MiniMax, Gemini, and DeepSeek
- Tray integration, session tracking, provider visibility toggles, and local-only persistence

## Platform status

- Primary target: Windows
- Also supported: macOS
- Default runtime: Electron
- `src-tauri/` is present, but Electron is the active desktop runtime today

## Supported agents and providers

### Approval / hook support

| Agent | Status |
| --- | --- |
| Claude Code | Supported |
| Codex | Supported |
| Gemini | Supported |

### Usage / balance sync

| Provider | Source |
| --- | --- |
| Codex | local auth and session data |
| Claude Code | local JSONL transcripts |
| Cursor | local app storage |
| MiniMax | local and API-backed usage fetch |
| Gemini | local login and session data |
| DeepSeek | API balance |

## Quick start

```bash
npm install
npm start
```

Alternative launcher:

```bash
node bin/cli.js
```

## Build

```bash
npm run build
npm run build:win
npm run build:mac
```

Tauri commands are also available:

```bash
npm run tauri:dev
npm run tauri:build
```

## Global shortcuts

- `Ctrl/Cmd+Shift+Space`: toggle island
- `Ctrl/Cmd+Shift+A`: approve
- `Ctrl/Cmd+Shift+L`: approve always
- `Ctrl/Cmd+Shift+D`: deny

## Local API

Base URL: `http://127.0.0.1:45874`

| Endpoint | Description |
| --- | --- |
| `GET /api/health` | health check |
| `GET /api/usage` | full dashboard payload |
| `GET /api/overview` | total balance, today, month, runway |
| `GET /api/accounts` | provider/account list |
| `GET /api/intervention` | current pending approval request |

## Project layout

```text
ThatIsOk/
|- main/         Electron main process
|- renderer/     floating island UI and dashboard
|- bridge/       standalone hook bridge CLI
|- shared/       hook normalization and IPC config
|- config/       providers, defaults, models
|- bin/          helper scripts and CLI entry
`- src-tauri/    Tauri 2 workspace
```

## Hook installation

On startup, the app injects managed hook entries into:

- `~/.claude/settings.json`
- `~/.codex/hooks.json`
- `~/.gemini/settings.json`

These managed entries point to `bridge/hook-bridge.js`.

## Notes

- Data is stored locally with `electron-store`
- The bridge uses local IPC over `127.0.0.1:45873` or a Windows named pipe
- `npm run test:hook` is the main hook smoke test today

## Docs

- Chinese README: [README.zh-CN.md](./README.zh-CN.md)

## License

ISC
