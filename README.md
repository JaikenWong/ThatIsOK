# ThatIsOK

ThatIsOK is a local desktop approval and usage cockpit for AI coding tools. It keeps a small floating island on screen for permission decisions and tracks provider usage across local agents.

## Why it exists

- Keep approval prompts visible without alt-tabbing back to the terminal
- Track usage and balance across multiple coding agents and providers in one place
- Store everything locally with no telemetry

## Highlights

- Floating always-on-top approval island with compact and expanded modes
- Approve, approve always, or deny with global shortcuts
- Hook bridge support for Claude Code and Codex
- Usage and balance sync for Codex, Claude Code, Cursor, MiniMax, Gemini, and DeepSeek
- Tray integration, session tracking, provider visibility toggles, and local-only persistence

## Platform status

- Primary target: Windows
- Also supported: macOS
- Active desktop runtime: Tauri 2

## Supported agents and providers

### Approval / hook support

| Agent | Status |
| --- | --- |
| Claude Code | Supported |
| Codex | Supported |

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

## Build

```bash
npm run tauri:build
```

## Global shortcuts

- `Ctrl/Cmd+Shift+Space`: toggle island
- `Ctrl/Cmd+Opt+A`: approve
- `Ctrl/Cmd+Opt+L`: approve always
- `Ctrl/Cmd+Opt+D`: deny

## Project layout

```text
ThatIsOK/
|- src-tauri/    active Tauri 2 app runtime and backend
|- renderer/     floating island UI loaded by Tauri webview
|- config/       providers and defaults
`- assets/       app icons and static assets
```

## Hook installation

On startup, the app injects managed hook entries into:

- `~/.claude/settings.json`
- `~/.codex/hooks.json`

Managed entries now point to the packaged ThatIsOK executable with `--hook-source` and `--hook-event`.

## Notes

- Runtime code is split across `src-tauri/src/` modules for hooks, shortcuts, providers, and UI state
- Local hook IPC uses `127.0.0.1:45873`
- `renderer/dashboard/` was removed because it was no longer wired to the active runtime

## Docs

- Chinese README: [README.zh-CN.md](./README.zh-CN.md)
- Windows release gate: [docs/windows-validation-checklist.md](./docs/windows-validation-checklist.md)

## License

ISC
