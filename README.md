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
- Active desktop runtime: Tauri 2
- Legacy Electron implementation is still present in `main/` and `bin/`, but is no longer the primary path

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
npm run tauri:dev
```

Legacy Electron launcher:

```bash
node bin/cli.js
```

## Build

```bash
npm run tauri:build
```

Legacy Electron packaging is still available through `npm run build`, `npm run build:win`, and `npm run build:mac`.

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
|- src-tauri/    active Tauri 2 app runtime and backend
|- renderer/     floating island UI loaded by Tauri webview
|- main/         legacy Electron main process
|- bridge/       legacy standalone hook bridge CLI
|- shared/       hook normalization and IPC config
|- config/       providers, defaults, models
`- bin/          helper scripts and legacy CLI entry
```

## Hook installation

On startup, the app injects managed hook entries into:

- `~/.claude/settings.json`
- `~/.codex/hooks.json`

Managed entries now point to the packaged ThatIsOk executable with `--hook-source` and `--hook-event`.
`bridge/hook-bridge.js` remains only for the legacy Electron path and old smoke tests.

## Notes

- Current runtime state and sync logic live in `src-tauri/src/lib.rs`
- Local hook IPC still uses `127.0.0.1:45873` or a Windows named pipe
- `renderer/dashboard/` was removed because it was no longer wired to either runtime
- `npm run test:hook` still exercises the legacy bridge path

## Docs

- Chinese README: [README.zh-CN.md](./README.zh-CN.md)

## License

ISC
