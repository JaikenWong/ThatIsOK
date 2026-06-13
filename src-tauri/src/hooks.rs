use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;
use std::process;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::{
    build_intervention, is_managed_command, is_persistently_allowed, now_millis,
    pending_intervention_json, AppState, SessionInfo, HOOK_EVENTS, MANAGED_KEY,
};

pub fn run_hook_bridge_from_args() -> bool {
    let args = env::args().collect::<Vec<_>>();
    if !args.iter().any(|arg| arg == "--hook-source") {
        return false;
    }

    let debug_log = |msg: &str| {
        if let Some(home) = dirs::home_dir() {
            let _ = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(home.join(".thatisok-hook-debug.log"))
                .and_then(|mut f| write!(f, "[{}] {}\n", now_millis(), msg));
        }
    };
    debug_log(&format!("hook-bridge start args={:?}", args));

    let source = get_cli_arg(&args, "--hook-source").unwrap_or_else(|| "unknown".to_string());
    let event_name = get_cli_arg(&args, "--hook-event").unwrap_or_else(|| "unknown".to_string());
    debug_log(&format!("parsed source={source} event={event_name}"));

    let input = read_stdin_json();
    debug_log(&format!("read stdin len={}", input.len()));

    if let Err(error) = run_hook_bridge(&source, &event_name, &input) {
        debug_log(&format!("bridge FAILED: {error}"));
        return true;
    }
    debug_log("bridge OK");
    true
}

fn read_stdin_json() -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let stdin = io::stdin();
    loop {
        match stdin.lock().read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if serde_json::from_slice::<Value>(&buffer).is_ok() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

pub(crate) fn inject_agent_hooks(app: &AppHandle) {
    let exe_path = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to resolve current exe for hooks: {error}");
            emit_runtime_warning(
                app,
                "Hook setup unavailable: app executable path could not be resolved.",
            );
            return;
        }
    };
    if let Err(error) = inject_codex_hooks(&exe_path) {
        eprintln!("failed to install Codex hooks: {error}");
        emit_runtime_warning(
            app,
            "Codex hook setup failed. Check local permissions and Codex config files.",
        );
    }
    if let Err(error) = inject_claude_hooks(&exe_path) {
        eprintln!("failed to install Claude hooks: {error}");
        emit_runtime_warning(
            app,
            "Claude hook setup failed. Check local permissions and Claude settings.",
        );
    }
    if let Err(error) = inject_opencode_hooks() {
        eprintln!("failed to install OpenCode hooks: {error}");
        emit_runtime_warning(
            app,
            "OpenCode hook setup failed. Check local permissions and OpenCode config.",
        );
    }
}

pub(crate) async fn start_hook_server(app: AppHandle) {
    let listener = match TcpListener::bind(("127.0.0.1", 45873)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind hook server: {error}");
            emit_runtime_warning(
                &app,
                "Hook server failed to start on 127.0.0.1:45873. Another process may already be using it.",
            );
            return;
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = handle_hook_socket(app_handle, stream).await;
                });
            }
            Err(error) => eprintln!("failed to accept hook client: {error}"),
        }
    }
}

fn get_cli_arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|items| items.first().is_some_and(|item| item == name))
        .and_then(|items| items.get(1))
        .cloned()
}

fn run_hook_bridge(source: &str, event_name: &str, input: &str) -> Result<(), String> {
    let payload = json!({
        "event": "hook-event",
        "data": {
            "source": source,
            "event": event_name,
            "raw": input,
            "payload": serde_json::from_str::<Value>(input).ok()
        }
    });
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", 45873)).map_err(|err| err.to_string())?;
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .map_err(|err| err.to_string())?;
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|err| err.to_string())?;
    let response = serde_json::from_str::<Value>(&line).map_err(|err| err.to_string())?;
    if event_name.eq_ignore_ascii_case("PermissionRequest") {
        write_permission_output(&response)?;
    }
    Ok(())
}

fn write_permission_output(response: &Value) -> Result<(), String> {
    if response.get("requiresDecision").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    let approved = response
        .get("approved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut decision = serde_json::Map::new();
    decision.insert(
        "behavior".to_string(),
        Value::String(if approved { "allow" } else { "deny" }.to_string()),
    );
    if !approved {
        decision.insert(
            "message".to_string(),
            Value::String("Denied from ThatIsOK".to_string()),
        );
    }
    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": Value::Object(decision)
        }
    });
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|err| err.to_string())?
    );
    Ok(())
}

fn inject_codex_hooks(exe_path: &Path) -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not available".to_string());
    };
    let hooks_path = home.join(".codex").join("hooks.json");
    let mut config = fs::read_to_string(&hooks_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| json!({ "hooks": {} }));
    if !config.get("hooks").is_some_and(Value::is_object) {
        config["hooks"] = json!({});
    }
    for event_name in HOOK_EVENTS {
        let existing = config["hooks"]
            .get(*event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let filtered = existing
            .into_iter()
            .filter(|entry| !is_managed_hook_value(entry))
            .collect::<Vec<_>>();
        let mut next_entries = filtered;
        let timeout = if *event_name == "PreToolUse" || *event_name == "PermissionRequest" { 86400 } else { 10000 };
        let command = build_tauri_hook_command(exe_path, "codex", event_name);
        let matcher_needed = !matches!(event_name, &"UserPromptSubmit" | &"Stop");
        let mut entry = json!({
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": timeout
            }],
            "_managedBy": MANAGED_KEY
        });
        if matcher_needed {
            entry["matcher"] = Value::String("*".to_string());
        }
        next_entries.push(entry);
        config["hooks"][*event_name] = Value::Array(next_entries);
    }
    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(hooks_path, format!("{content}\n")).map_err(|err| err.to_string())?;
    Ok(())
}

fn inject_claude_hooks(exe_path: &Path) -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not available".to_string());
    };
    let settings_path = home.join(".claude").join("settings.json");
    let Ok(content) = fs::read_to_string(&settings_path) else {
        return Ok(());
    };
    let Ok(mut config) = serde_json::from_str::<Value>(&content) else {
        return Err("invalid Claude settings.json".to_string());
    };
    if !config.get("hooks").is_some_and(Value::is_object) {
        config["hooks"] = json!({});
    }
    for event_name in HOOK_EVENTS {
        let existing = config["hooks"]
            .get(*event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let filtered = existing
            .into_iter()
            .filter(|entry| !is_managed_hook_value(entry))
            .collect::<Vec<_>>();
        let mut next_entries = filtered;
        next_entries.push(json!({
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": build_tauri_hook_command(exe_path, "claude", event_name),
                "timeout": if *event_name == "PermissionRequest" { 86400 } else { 10000 }
            }],
            "_managedBy": MANAGED_KEY
        }));
        config["hooks"][*event_name] = Value::Array(next_entries);
    }
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(settings_path, format!("{content}\n")).map_err(|err| err.to_string())?;
    Ok(())
}

fn build_tauri_hook_command(exe_path: &Path, source: &str, event_name: &str) -> String {
    let escaped = exe_path.display().to_string().replace('"', "\\\"");
    format!("\"{escaped}\" --hook-source {source} --hook-event {event_name}")
}

fn is_managed_hook_value(value: &Value) -> bool {
    if value.get("_managedBy").and_then(Value::as_str) == Some(MANAGED_KEY) {
        return true;
    }
    if value
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_managed_command)
    {
        return true;
    }
    value
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(is_managed_hook_value))
}

fn map_session_status(event_name: &str) -> String {
    match event_name.to_lowercase().as_str() {
        "sessionstart" => "active".to_string(),
        "stop" | "sessionend" => "done".to_string(),
        "permissionrequest" => "waiting".to_string(),
        "posttooluse" | "userpromptsubmit" => "active".to_string(),
        _ => "idle".to_string(),
    }
}

async fn handle_hook_socket(app: AppHandle, stream: TcpStream) -> Result<(), String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .await
        .map_err(|err| err.to_string())?;
    if read == 0 || line.trim().is_empty() {
        return Ok(());
    }

    let response = handle_hook_message(
        app,
        serde_json::from_str(&line).map_err(|err| err.to_string())?,
    )
    .await;
    let payload = serde_json::to_string(&response).map_err(|err| err.to_string())?;
    let mut stream = reader.into_inner();
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .await
        .map_err(|err| err.to_string())
}

async fn handle_hook_message(app: AppHandle, message: Value) -> Value {
    if message.get("event").and_then(Value::as_str) != Some("hook-event") {
        return json!({ "ok": true });
    }

    let data = message.get("data").cloned().unwrap_or_else(|| json!({}));
    let source = data
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let event_name = data
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let raw = data.get("raw").and_then(Value::as_str).unwrap_or("");
    let payload = data.get("payload").cloned().unwrap_or(Value::Null);

    {
        let app_state = app.state::<AppState>();
        let mut sessions = match app_state.sessions.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "ok": true, "recorded": false }),
        };
        let session_id = payload
            .get("session_id")
            .or_else(|| payload.get("sessionId"))
            .or_else(|| data.get("session_id"))
            .or_else(|| data.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or(source);
        let entry = sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionInfo::default);
        entry.id = session_id.to_string();
        entry.source = source.to_string();
        entry.status = map_session_status(event_name);
        entry.updated_at = now_millis();
        entry.last_event = event_name.to_string();
    }

    if !event_name.eq_ignore_ascii_case("PermissionRequest") {
        return json!({ "ok": true, "recorded": true });
    }

    let mut intervention = build_intervention(source, event_name, raw, payload);
    if is_persistently_allowed(&intervention) {
        return json!({
            "ok": true,
            "requiresDecision": true,
            "approved": true,
            "allowPersistent": true
        });
    }

    let (tx, rx) = oneshot::channel();
    intervention.responder = Some(tx);
    let pending_json = pending_intervention_json(&intervention);
    {
        let app_state = app.state::<AppState>();
        let mut pending = match app_state.intervention.lock() {
            Ok(pending) => pending,
            Err(_) => return json!({ "ok": true, "requiresDecision": true, "approved": false }),
        };
        if pending.is_some() {
            return json!({
                "ok": true,
                "requiresDecision": true,
                "approved": false,
                "allowPersistent": false,
                "message": "ThatIsOK already has a pending approval."
            });
        }
        *pending = Some(intervention);
    }

    let _ = app.emit("intervention-state", pending_json);
    let _ = app.emit("island-force-expand", ());
    match rx.await {
        Ok(decision) => json!({
            "ok": true,
            "requiresDecision": true,
            "approved": decision.approved,
            "allowPersistent": decision.allow_persistent
        }),
        Err(_) => json!({
            "ok": true,
            "requiresDecision": true,
            "approved": false,
            "allowPersistent": false
        }),
    }
}

fn inject_opencode_hooks() -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not available".to_string());
    };
    let plugins_dir = home.join(".config").join("opencode").join("plugins");
    fs::create_dir_all(&plugins_dir).map_err(|err| err.to_string())?;
    let plugin_path = plugins_dir.join("thatisok.js");
    let plugin_content =
        include_str!("../plugins/thatisok-opencode.js");
    fs::write(&plugin_path, plugin_content).map_err(|err| err.to_string())?;
    Ok(())
}

fn emit_runtime_warning(app: &AppHandle, message: &str) {
    let _ = app.emit(
        "runtime-warning",
        json!({
            "message": message
        }),
    );
}
