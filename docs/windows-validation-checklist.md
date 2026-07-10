# Windows Validation Checklist

Run this on a real Windows machine before any paid release.

## Scope

This checklist proves:

- packaged app installs and launches
- floating island works
- tray works
- shortcuts work
- hook injection points at packaged `AgentIsOK.exe`
- hook runtime returns decisions correctly
- sync works or fails softly
- no stale Electron or `node` dependency remains

## Test Machine

Record these before testing:

- Windows version
- architecture: `x64` / `arm64`
- shell used for build: `PowerShell` / `cmd`
- whether Codex / Claude / Cursor / Gemini / MiniMax / DeepSeek credentials already exist

## Build

From clean checkout:

```bash
npm install
npm run tauri:build
```

Expected evidence:

- command exits `0`
- NSIS bundle exists under:
  - `src-tauri/target/release/bundle/nsis/`
- installer file exists and is recent

Fail release if:

- build needs manual patching
- bundle output is missing

## Install

1. Run NSIS installer with default per-user flow.
2. Confirm Start Menu entry exists: `AgentIsOK`.
3. Confirm uninstall entry exists in Windows Apps list.

Expected evidence:

- app installs without needing `node`
- no console errors during install

Fail release if:

- installer references missing runtime
- installer path still points at dev machine or repo path

## Launch

1. Launch from Start Menu.
2. Wait for floating island.

Expected evidence:

- island appears near top-center on primary display
- window is transparent and always-on-top
- app does not open a debug console by default

Fail release if:

- window does not appear
- window loses topmost behavior
- app crashes on first launch

## Tray

1. Confirm tray icon appears.
2. Left click tray icon.
3. Open tray menu.

Expected evidence:

- left click expands and focuses island
- tray menu contains:
  - `Open`
  - `Sync Now`
  - `Quit`
- `Quit` exits app fully

Fail release if:

- tray icon missing
- menu items do nothing
- quit leaves background process running

## Window Behavior

1. Drag collapsed pill.
2. Expand and collapse repeatedly.
3. Trigger warning state if possible.

Expected evidence:

- drag is smooth
- no visual overlap
- expand height fits content
- warning strip does not break layout

Fail release if:

- drag stutters badly
- content overlaps
- layout jumps or clips badly

## Shortcuts

Verify:

- `Ctrl + Shift + Space` toggles island
- with pending approval:
  - `Ctrl + Alt + A` approves
  - `Ctrl + Alt + L` approves permanently
  - `Ctrl + Alt + D` denies

Expected evidence:

- shortcut action happens immediately
- if shortcut registration fails, island shows warning instead of silent failure

Fail release if:

- shortcuts fail silently
- wrong action fires
- warning path is missing when shortcut bind fails

## Hook Injection

Launch packaged app once, then inspect:

- `%USERPROFILE%\.codex\hooks.json`
- `%USERPROFILE%\.claude\settings.json`

Expected evidence:

- managed entries exist
- commands point to packaged `AgentIsOK.exe`
- commands contain:
  - `--hook-source`
  - `--hook-event`
- no command uses:
  - `node`
  - repo-local `.js` hook path

PowerShell checks:

```powershell
Select-String -Path "$env:USERPROFILE\.codex\hooks.json" -Pattern "AgentIsOK.exe","--hook-source","--hook-event"
Select-String -Path "$env:USERPROFILE\.claude\settings.json" -Pattern "AgentIsOK.exe","--hook-source","--hook-event"
Select-String -Path "$env:USERPROFILE\.codex\hooks.json" -Pattern "node|hook-bridge\.js"
Select-String -Path "$env:USERPROFILE\.claude\settings.json" -Pattern "node|hook-bridge\.js"
```

Fail release if:

- hook command points at dev path
- hook command requires `node`
- managed hook entry missing

## Hook Runtime

1. Trigger Codex permission request.
2. Trigger Claude permission request.
3. Use Approve / Always / Deny once each.

Expected evidence:

- island expands automatically
- correct source badge shows
- decision returns to originating tool
- `Always` persists approval rule

Persistent approval evidence:

- `%APPDATA%\AgentIsOK\approval-rules.json` exists after approve-always

Fail release if:

- approval does not return to caller
- wrong source/tool shown
- persistent rule not saved

## Hook Server

While app is running:

```powershell
netstat -ano | findstr 45873
```

Expected evidence:

- local listener on `127.0.0.1:45873`

Fail release if:

- port never binds
- bind conflict happens with no in-app warning

## Sync

1. Click `Sync`.
2. Wait for data refresh.
3. Test with both valid and missing credentials.

Expected evidence:

- UI stays responsive
- valid providers update
- missing providers fail softly
- if one or more providers fail, island shows compact warning

Providers to verify when credentials exist:

- Codex
- Claude
- Cursor
- MiniMax
- Gemini
- DeepSeek
- OpenCode if configured

Fail release if:

- app freezes during sync
- single provider failure crashes whole refresh
- sync failure stays silent

## Settings

1. Expand island.
2. Change `Sync cadence` with `-` / `+`.
3. Close and reopen app.

Expected evidence:

- value changes immediately in UI
- next scheduled sync uses new interval immediately
- value persists across restart

Config evidence:

- `%APPDATA%\AgentIsOK\defaults.json` contains `syncIntervalMinutes`

Fail release if:

- control changes UI only but not runtime
- value resets after restart

## No Old Runtime Residue

Installed artifact and hook config must not reference:

- `electron`
- `electron-builder`
- `hook-bridge.js`
- repo-local source paths

PowerShell spot checks:

```powershell
Get-ChildItem "$env:LOCALAPPDATA","$env:APPDATA" -Recurse -ErrorAction SilentlyContinue |
  Select-String -Pattern "hook-bridge\.js|electron" -ErrorAction SilentlyContinue
```

Fail release if:

- installed runtime still depends on Electron residue

## Capture For Release Record

Save these with release notes or QA ticket:

- installer filename
- screenshot of idle island
- screenshot of pending approval
- screenshot of warning strip
- copy of `hooks.json` managed command
- copy of `%APPDATA%\AgentIsOK\defaults.json`
- build date and git commit

## Release Gate

Do not ship if any item below is true:

- app requires `node`
- hook command points at dev path
- tray action fails
- shortcut fails silently
- hook bind fails silently
- approval does not return to tool
- sync crashes app
- sync failure is invisible
- settings value does not persist
- packaged Windows artifact missing
