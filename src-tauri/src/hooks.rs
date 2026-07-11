use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;

use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::{
    app_config_dir, build_intervention, is_managed_command, is_persistently_allowed, now_millis,
    pending_intervention_json, refresh_opencode_local_usage, AppState, PendingIntervention,
    SessionEvent, SessionInfo, ALL_HOOK_EVENTS, CLAUDE_HOOK_EVENTS, CODEX_HOOK_EVENTS, MANAGED_KEY,
};

const ANTIGRAVITY_HOOK_NAME: &str = "agentisok";

pub fn run_hook_bridge_from_args() -> bool {
    let args = env::args().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--uninstall-hooks") {
        if let Err(error) = remove_agent_hooks() {
            eprintln!("failed to remove hooks: {error}");
        }
        return true;
    }
    if !args.iter().any(|arg| arg == "--hook-source") {
        return false;
    }

    let debug_log = |msg: &str| {
        if let Some(home) = dirs::home_dir() {
            let _ = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(home.join(".agentisok-hook-debug.log"))
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
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
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
        let _ = tx.send(buffer);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(buffer) => String::from_utf8_lossy(&buffer).to_string(),
        Err(_) => String::new(),
    }
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
    let bridge_path = match ensure_bridge_script() {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!("failed to install hook bridge script: {error}");
            emit_runtime_warning(
                app,
                "Hook bridge script setup failed. On Windows hooks will fall back to spawning the app executable.",
            );
            None
        }
    };
    if let Err(error) = inject_codex_hooks(&exe_path, bridge_path.as_deref()) {
        eprintln!("failed to install Codex hooks: {error}");
        emit_runtime_warning(
            app,
            "Codex hook setup failed. Check local permissions and Codex config files.",
        );
    }
    if let Err(error) = inject_claude_hooks(&exe_path, bridge_path.as_deref()) {
        eprintln!("failed to install Claude hooks: {error}");
        emit_runtime_warning(
            app,
            "Claude hook setup failed. Check local permissions and Claude settings.",
        );
    }
    if let Err(error) = inject_antigravity_hooks(&exe_path, bridge_path.as_deref()) {
        eprintln!("failed to install Antigravity hooks: {error}");
        emit_runtime_warning(
            app,
            "Antigravity hook setup failed. Check ~/.gemini/config/hooks.json.",
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

/// Drop the JS hook-bridge script next to the app config so that on platforms
/// where spawning the signed app exe on every hook event is undesirable (e.g.
/// Windows whitelisting/EDR) we can instead invoke `node <bridge.js>`.
fn ensure_bridge_script() -> Result<std::path::PathBuf, String> {
    let dir = app_config_dir();
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let bridge_path = dir.join("hook-bridge.js");
    let content = include_str!("../plugins/hook-bridge.js");
    fs::write(&bridge_path, content).map_err(|err| err.to_string())?;
    Ok(bridge_path)
}

pub(crate) fn hooks_intact() -> bool {
    let Some(home) = dirs::home_dir() else {
        return true;
    };
    claude_hooks_present(&home)
        && codex_hooks_present(&home)
        && antigravity_hooks_present(&home)
        && opencode_plugin_present(&home)
}

fn claude_hooks_present(home: &Path) -> bool {
    let path = home.join(".claude").join("settings.json");
    let Ok(content) = fs::read_to_string(&path) else {
        return true; // file doesn't exist, skip (not our job to create it)
    };
    let Ok(config) = serde_json::from_str::<Value>(&content) else {
        return true; // malformed, don't touch
    };
    config
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| hooks.values().any(|v| {
            v.as_array()
                .is_some_and(|arr| arr.iter().any(is_managed_hook_value))
        }))
}

fn codex_hooks_present(home: &Path) -> bool {
    let path = home.join(".codex").join("hooks.json");
    let Ok(content) = fs::read_to_string(&path) else {
        return true; // no file, skip
    };
    let Ok(config) = serde_json::from_str::<Value>(&content) else {
        return true;
    };
    config
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| hooks.values().any(|v| {
            v.as_array()
                .is_some_and(|arr| arr.iter().any(is_managed_hook_value))
        }))
}

fn antigravity_hooks_present(home: &Path) -> bool {
    let path = home.join(".gemini").join("config").join("hooks.json");
    let Ok(content) = fs::read_to_string(&path) else {
        return true; // no file, skip
    };
    let Ok(config) = serde_json::from_str::<Value>(&content) else {
        return true;
    };
    config
        .get(ANTIGRAVITY_HOOK_NAME)
        .is_some_and(is_managed_hook_value)
}

fn opencode_plugin_present(home: &Path) -> bool {
    let plugin_path = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("agentisok.js");
    plugin_path.exists()
}

pub(crate) fn remove_agent_hooks() -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not available".to_string());
    };

    let mut errors = Vec::new();
    if let Err(error) = remove_managed_hooks_from_file(&home.join(".codex").join("hooks.json")) {
        errors.push(format!("Codex: {error}"));
    }
    if let Err(error) = remove_managed_hooks_from_file(&home.join(".claude").join("settings.json"))
    {
        errors.push(format!("Claude: {error}"));
    }
    if let Err(error) = remove_opencode_plugin(&home) {
        errors.push(format!("OpenCode: {error}"));
    }
    if let Err(error) = remove_antigravity_hooks(&home) {
        errors.push(format!("Antigravity: {error}"));
    }
    if let Err(error) = remove_bridge_script() {
        errors.push(format!("bridge: {error}"));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
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

fn remove_managed_hooks_from_file(path: &Path) -> Result<(), String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let mut config = serde_json::from_str::<Value>(&content).map_err(|err| err.to_string())?;
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };

    let events = hooks.keys().cloned().collect::<Vec<_>>();
    for event_name in events {
        let next = hooks
            .get(&event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| !is_managed_hook_value(entry))
            .collect::<Vec<_>>();
        if next.is_empty() {
            hooks.remove(&event_name);
        } else {
            hooks.insert(event_name, Value::Array(next));
        }
    }

    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(path, format!("{content}\n")).map_err(|err| err.to_string())
}

fn remove_antigravity_hooks(home: &Path) -> Result<(), String> {
    let path = home.join(".gemini").join("config").join("hooks.json");
    let Ok(content) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut config = serde_json::from_str::<Value>(&content).map_err(|err| err.to_string())?;
    let Some(hooks) = config.as_object_mut() else {
        return Ok(());
    };
    hooks.retain(|name, value| {
        name != ANTIGRAVITY_HOOK_NAME && name != "thatisok" && !is_managed_hook_value(value)
    });
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(path, format!("{content}\n")).map_err(|err| err.to_string())
}

fn remove_opencode_plugin(home: &Path) -> Result<(), String> {
    let plugins_dir = home.join(".config").join("opencode").join("plugins");
    for plugin_name in ["agentisok.js", "thatisok.js"] {
        let plugin_path = plugins_dir.join(plugin_name);
        if plugin_path.exists() {
            fs::remove_file(&plugin_path).map_err(|err| err.to_string())?;
        }
    }

    let config_path = home.join(".config").join("opencode").join("config.json");
    let Ok(content) = fs::read_to_string(&config_path) else {
        return Ok(());
    };
    let mut config = serde_json::from_str::<Value>(&content).map_err(|err| err.to_string())?;
    let Some(plugins) = config.get_mut("plugin").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    plugins.retain(|item| {
        item.as_str()
            .map(|value| {
                !value.contains("agentisok.js")
                    && !value.contains("AgentIsOK")
                    && !value.contains("thatisok.js")
                    && !value.contains("ThatIsOK")
            })
            .unwrap_or(true)
    });
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(config_path, format!("{content}\n")).map_err(|err| err.to_string())
}

fn remove_bridge_script() -> Result<(), String> {
    let bridge_path = app_config_dir().join("hook-bridge.js");
    if bridge_path.exists() {
        fs::remove_file(&bridge_path).map_err(|err| err.to_string())?;
    }
    Ok(())
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
    let addr: std::net::SocketAddr = "127.0.0.1:45873"
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let mut stream =
        match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3)) {
            Ok(stream) => stream,
            Err(error) => {
                if write_antigravity_default_output(source, event_name)? {
                    return Ok(());
                }
                return Err(error.to_string());
            }
        };
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .map_err(|err| err.to_string())?;
    let read_timeout = if is_decision_event(source, event_name) {
        1_800
    } else {
        5
    };
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(read_timeout)))
        .map_err(|err| err.to_string())?;
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|err| err.to_string())?;
    let response = serde_json::from_str::<Value>(&line).map_err(|err| err.to_string())?;
    if is_decision_event(source, event_name) {
        write_permission_output(source, &response)?;
    } else if source == "antigravity" {
        write_antigravity_non_decision_output(event_name)?;
    }
    Ok(())
}

fn is_decision_event(source: &str, event_name: &str) -> bool {
    let event_lower = event_name.to_lowercase();
    matches!(
        event_lower.as_str(),
        "permissionrequest" | "permission_request"
    ) || (source == "antigravity" && matches!(event_lower.as_str(), "pretooluse" | "pre_tool_use"))
}

fn write_antigravity_default_output(source: &str, event_name: &str) -> Result<bool, String> {
    if source != "antigravity" {
        return Ok(false);
    }
    let output = if is_decision_event(source, event_name) {
        json!({
            "decision": "ask",
            "reason": "AgentIsOK is not reachable."
        })
    } else {
        antigravity_non_decision_output(event_name)
    };
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|err| err.to_string())?
    );
    Ok(true)
}

fn write_antigravity_non_decision_output(event_name: &str) -> Result<(), String> {
    let output = antigravity_non_decision_output(event_name);
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|err| err.to_string())?
    );
    Ok(())
}

fn antigravity_non_decision_output(event_name: &str) -> Value {
    if event_name.eq_ignore_ascii_case("Stop") {
        json!({ "decision": "allow" })
    } else {
        json!({})
    }
}

fn write_permission_output(source: &str, response: &Value) -> Result<(), String> {
    if response.get("requiresDecision").and_then(Value::as_bool) != Some(true) {
        if source == "antigravity" {
            println!("{}", json!({ "decision": "allow" }));
        }
        return Ok(());
    }
    if response.get("isQuestion").and_then(Value::as_bool) == Some(true) {
        write_question_output(source, response)?;
        return Ok(());
    }
    let approved = response
        .get("approved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if source == "antigravity" {
        let output = json!({
            "decision": if approved { "allow" } else { "deny" },
            "reason": if approved { "" } else { "Denied from AgentIsOK" }
        });
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|err| err.to_string())?
        );
        return Ok(());
    }
    let mut decision = serde_json::Map::new();
    decision.insert(
        "behavior".to_string(),
        Value::String(if approved { "allow" } else { "deny" }.to_string()),
    );
    if !approved {
        decision.insert(
            "message".to_string(),
            Value::String("Denied from AgentIsOK".to_string()),
        );
        decision.insert("interrupt".to_string(), Value::Bool(false));
    }
    let mut output = json!({
        "continue": true,
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": Value::Object(decision)
        }
    });
    if source == "claude" {
        output["suppressOutput"] = Value::Bool(true);
    }
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|err| err.to_string())?
    );
    Ok(())
}

fn write_question_output(source: &str, response: &Value) -> Result<(), String> {
    let answer = response
        .get("answer")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    let output = if source == "opencode" {
        json!({
            "type": "answer",
            "text": answer
        })
    } else if source == "antigravity" {
        json!({
            "decision": "allow",
            "reason": answer
        })
    } else {
        let question = response
            .get("question")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("answer");
        let mut decision = serde_json::Map::new();
        decision.insert("behavior".to_string(), Value::String("allow".to_string()));
        let mut hook_specific_output = json!({
            "hookEventName": "PermissionRequest",
            "decision": Value::Object(decision),
            "answer": answer
        });
        if !answer.is_empty() {
            hook_specific_output["updatedInput"] = json!({
                "answers": {
                    question: answer
                }
            });
        }
        let mut output = json!({
            "continue": true,
            "hookSpecificOutput": hook_specific_output
        });
        if source == "claude" {
            output["suppressOutput"] = Value::Bool(true);
        }
        output
    };

    println!(
        "{}",
        serde_json::to_string(&output).map_err(|err| err.to_string())?
    );
    Ok(())
}

fn inject_codex_hooks(exe_path: &Path, bridge_path: Option<&Path>) -> Result<(), String> {
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
    for event_name in ALL_HOOK_EVENTS {
        let existing = config["hooks"]
            .get(*event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let filtered = existing
            .into_iter()
            .filter(|entry| !is_managed_hook_value(entry))
            .collect::<Vec<_>>();
        config["hooks"][*event_name] = Value::Array(filtered);
    }
    for event_name in CODEX_HOOK_EVENTS {
        let mut next_entries = config["hooks"]
            .get(*event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let timeout = if *event_name == "PermissionRequest" {
            3_600
        } else {
            45
        };
        let command = build_tauri_hook_command(exe_path, bridge_path, "codex", event_name);
        let matcher = match *event_name {
            "SessionStart" => Some("startup|resume"),
            "UserPromptSubmit" | "PermissionRequest" | "Stop" => None,
            _ => Some("*"),
        };
        let mut entry = json!({
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": timeout
            }],
            "_managedBy": MANAGED_KEY
        });
        if let Some(matcher) = matcher {
            entry["matcher"] = Value::String(matcher.to_string());
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

fn inject_claude_hooks(exe_path: &Path, bridge_path: Option<&Path>) -> Result<(), String> {
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
    for event_name in CLAUDE_HOOK_EVENTS {
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
        let entry = json!({
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": build_tauri_hook_command(exe_path, bridge_path, "claude", event_name),
                "timeout": if *event_name == "PermissionRequest" { 86_400 } else { 10_000 }
            }],
            "_managedBy": MANAGED_KEY
        });
        next_entries.push(entry);
        config["hooks"][*event_name] = Value::Array(next_entries);
    }
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(settings_path, format!("{content}\n")).map_err(|err| err.to_string())?;
    Ok(())
}

fn inject_antigravity_hooks(exe_path: &Path, bridge_path: Option<&Path>) -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not available".to_string());
    };
    let hooks_path = home.join(".gemini").join("config").join("hooks.json");
    let mut config = fs::read_to_string(&hooks_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| json!({}));
    if !config.is_object() {
        config = json!({});
    }
    let hooks = config
        .as_object_mut()
        .ok_or_else(|| "invalid Antigravity hooks.json".to_string())?;
    hooks.retain(|name, value| {
        name != ANTIGRAVITY_HOOK_NAME && name != "thatisok" && !is_managed_hook_value(value)
    });
    hooks.insert(
        ANTIGRAVITY_HOOK_NAME.to_string(),
        json!({
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": build_tauri_hook_command(exe_path, bridge_path, "antigravity", "PreToolUse"),
                    "timeout": 1800
                }]
            }],
            "PostToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": build_tauri_hook_command(exe_path, bridge_path, "antigravity", "PostToolUse"),
                    "timeout": 10
                }]
            }],
            "Stop": [{
                "type": "command",
                "command": build_tauri_hook_command(exe_path, bridge_path, "antigravity", "Stop"),
                "timeout": 10
            }]
        }),
    );
    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(hooks_path, format!("{content}\n")).map_err(|err| err.to_string())
}

fn build_tauri_hook_command(
    exe_path: &Path,
    bridge_path: Option<&Path>,
    source: &str,
    event_name: &str,
) -> String {
    let esc_source = source.replace('"', "\\\"");
    let esc_event = event_name.replace('"', "\\\"");
    #[cfg(windows)]
    {
        if let Some(bridge) = bridge_path {
            let bridge = bridge.display().to_string().replace('"', "\\\"");
            return format!(
                "node \"{bridge}\" --hook-source \"{esc_source}\" --hook-event \"{esc_event}\""
            );
        }
        let exe = exe_path.display().to_string().replace('"', "\\\"");
        format!("\"{exe}\" --hook-source \"{esc_source}\" --hook-event \"{esc_event}\"")
    }
    #[cfg(not(windows))]
    {
        let _ = bridge_path;
        let exe = exe_path.display().to_string().replace('"', "\\\"");
        format!("\"{exe}\" --hook-source \"{esc_source}\" --hook-event \"{esc_event}\"")
    }
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
    if value
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(is_managed_hook_value))
    {
        return true;
    }
    if let Some(items) = value.as_array() {
        return items.iter().any(is_managed_hook_value);
    }
    if let Some(items) = value.as_object() {
        return items.values().any(is_managed_hook_value);
    }
    false
}

struct EventInfo {
    status: &'static str,
    summary: &'static str,
}

fn lookup_event_info(event_name: &str) -> Option<EventInfo> {
    match event_name.to_lowercase().as_str() {
        "sessionstart" | "session_start" => Some(EventInfo {
            status: "Active",
            summary: "Session started",
        }),
        "stop" | "sessionend" | "session_end" | "stopfailure" => Some(EventInfo {
            status: "Done",
            summary: "completed",
        }),
        "permissionrequest" | "permission_request" => Some(EventInfo {
            status: "Awaiting approval",
            summary: "",
        }),
        "pretooluse" | "pre_tool_use" => Some(EventInfo {
            status: "Running tool",
            summary: "Running tool",
        }),
        "posttooluse" | "post_tool_use" | "posttoolusefailure" => Some(EventInfo {
            status: "Active",
            summary: "finished",
        }),
        "userpromptsubmit" | "user_prompt_submit" => Some(EventInfo {
            status: "Active",
            summary: "Prompt submitted",
        }),
        "notification" => Some(EventInfo {
            status: "Active",
            summary: "Notification",
        }),
        _ => None,
    }
}

fn map_session_status(event_name: &str) -> String {
    lookup_event_info(event_name)
        .map(|info| info.status.to_string())
        .unwrap_or_default()
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
    let activity = build_event_summary(source, event_name, &payload);
    let activity_detail = build_event_detail(source, event_name, &payload);
    let tool_name = event_tool_name(&payload);
    let command = event_command(&payload);
    let file_path = event_file_path(&payload);

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
        let new_status = map_session_status(event_name);
        if !new_status.is_empty() {
            entry.status = new_status;
        }
        if !activity.is_empty() {
            entry.activity = activity.clone();
        }
        if !activity_detail.is_empty() {
            entry.activity_detail = activity_detail.clone();
        }
        entry.tool_name = tool_name.clone();
        entry.command = command.clone();
        entry.file_path = file_path.clone();
        let now = now_millis();
        entry.events.insert(
            0,
            SessionEvent {
                event: event_name.to_string(),
                summary: activity.clone(),
                detail: activity_detail.clone(),
                created_at: now,
            },
        );
        entry.events.truncate(8);
        entry.updated_at = now;
        entry.last_event = event_name.to_string();
        if let Some(jt) = payload
            .get("jump_target")
            .or_else(|| payload.get("jumpTarget"))
        {
            entry.jump_target = Some(jt.clone());
        }
    }

    let event_lower = event_name.to_lowercase();
    if should_refresh_opencode_usage(source, &event_lower) {
        let app_for_refresh = app.clone();
        tauri::async_runtime::spawn(async move {
            // Cancel previous pending refresh
            {
                let state = app_for_refresh.state::<AppState>();
                let abort_guard = state.opencode_refresh_abort.lock();
                if let Ok(mut abort) = abort_guard {
                    if let Some(sender) = abort.take() {
                        let _ = sender.send(());
                    }
                };
            }
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            {
                let state = app_for_refresh.state::<AppState>();
                let abort_guard = state.opencode_refresh_abort.lock();
                if let Ok(mut abort) = abort_guard {
                    *abort = Some(tx);
                };
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(700)) => {
                    refresh_opencode_local_usage(&app_for_refresh);
                }
                _ = rx => {
                    // Cancelled by a newer refresh request
                }
            }
        });
    }

    let session_id = payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .or_else(|| data.get("session_id"))
        .or_else(|| data.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or(source);
    let _ = app.emit(
        "hook-event",
        json!({
            "source": source,
            "event": event_name,
            "sessionID": session_id,
            "status": map_session_status(event_name),
            "summary": activity,
            "activityDetail": activity_detail,
            "toolName": tool_name,
            "command": command,
            "filePath": file_path,
            "timelineEvent": {
                "event": event_name,
                "summary": activity,
                "detail": activity_detail,
                "createdAt": now_millis(),
            },
            "jumpTarget": payload.get("jump_target")
                .or_else(|| payload.get("jumpTarget"))
        }),
    );

    if !is_decision_event(source, event_name) {
        return json!({ "ok": true, "recorded": true });
    }

    let is_question = payload
        .get("tool_name")
        .or_else(|| payload.get("toolName"))
        .and_then(Value::as_str)
        .is_some_and(|t| t == "AskUserQuestion" || t.eq_ignore_ascii_case("askuserquestion"))
        || payload.get("questionPrompt").is_some()
        || payload.get("question_prompt").is_some();

    if is_question {
        let mut intervention = build_intervention(source, event_name, raw, payload);
        intervention.event = "QuestionAsked".to_string();
        return await_intervention_decision(app, intervention, true).await;
    }

    let intervention = build_intervention(source, event_name, raw, payload);
    if is_persistently_allowed(&intervention) {
        return json!({
            "ok": true,
            "requiresDecision": true,
            "approved": true,
            "allowPersistent": true
        });
    }

    await_intervention_decision(app, intervention, false).await
}

async fn await_intervention_decision(
    app: AppHandle,
    mut intervention: PendingIntervention,
    is_question: bool,
) -> Value {
    let (tx, rx) = oneshot::channel();
    intervention.responder = Some(tx);
    let question_key = if is_question {
        question_answer_key(&intervention)
    } else {
        None
    };
    let pending_json = pending_intervention_json(&intervention);
    {
        let app_state = app.state::<AppState>();
        let mut pending = match app_state.intervention.lock() {
            Ok(pending) => pending,
            Err(_) => {
                return json!({
                    "ok": true,
                    "requiresDecision": true,
                    "approved": false,
                    "allowPersistent": false,
                    "isQuestion": is_question
                })
            }
        };
        if pending.is_some() {
            return json!({
                "ok": true,
                "requiresDecision": true,
                "approved": false,
                "allowPersistent": false,
                "message": "AgentIsOK already has a pending approval.",
                "isQuestion": is_question
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
            "allowPersistent": decision.allow_persistent,
            "answer": decision.answer,
            "question": question_key,
            "isQuestion": is_question
        }),
        Err(_) => json!({
            "ok": true,
            "requiresDecision": true,
            "approved": false,
            "allowPersistent": false,
            "isQuestion": is_question
        }),
    }
}

fn question_answer_key(intervention: &PendingIntervention) -> Option<String> {
    let tool_input = intervention
        .meta
        .get("tool_input")
        .or_else(|| intervention.meta.get("toolInput"));
    let first_question = tool_input
        .and_then(|input| input.get("questions"))
        .and_then(Value::as_array)
        .and_then(|questions| questions.first())
        .and_then(|question| question.get("question"))
        .and_then(Value::as_str);
    first_question
        .or_else(|| {
            intervention
                .meta
                .get("questionPrompt")
                .or_else(|| intervention.meta.get("question_prompt"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn should_refresh_opencode_usage(source: &str, event_lower: &str) -> bool {
    source == "opencode"
        && matches!(
            event_lower,
            "posttooluse" | "post_tool_use" | "stop" | "sessionend" | "session_end"
        )
}

fn inject_opencode_hooks() -> Result<(), String> {
    let Some(home) = dirs::home_dir() else {
        return Err("home directory not available".to_string());
    };
    let plugins_dir = home.join(".config").join("opencode").join("plugins");
    fs::create_dir_all(&plugins_dir).map_err(|err| err.to_string())?;
    let legacy_plugin_path = plugins_dir.join("thatisok.js");
    if legacy_plugin_path.exists() {
        fs::remove_file(&legacy_plugin_path).map_err(|err| err.to_string())?;
    }
    let plugin_path = plugins_dir.join("agentisok.js");
    let plugin_content = include_str!("../plugins/agentisok-opencode.js");
    fs::write(&plugin_path, plugin_content).map_err(|err| err.to_string())?;
    register_opencode_plugin(&home, &plugin_path)?;
    Ok(())
}

fn register_opencode_plugin(home: &Path, plugin_path: &Path) -> Result<(), String> {
    let config_path = home.join(".config").join("opencode").join("config.json");
    let mut config = fs::read_to_string(&config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| json!({ "$schema": "https://opencode.ai/config.json" }));
    if !config.get("plugin").is_some_and(Value::is_array) {
        config["plugin"] = json!([]);
    }
    let plugin_uri = format!("file://{}", plugin_path.display());
    let plugins = config["plugin"]
        .as_array_mut()
        .ok_or_else(|| "invalid OpenCode plugin config".to_string())?;
    plugins.retain(|item| {
        item.as_str()
            .map(|value| !value.contains("thatisok.js") && !value.contains("ThatIsOK"))
            .unwrap_or(true)
    });
    let already_registered = plugins
        .iter()
        .filter_map(Value::as_str)
        .any(|item| item == plugin_uri || item.ends_with("/agentisok.js"));
    if !already_registered {
        plugins.push(Value::String(plugin_uri));
    }
    let content = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
    fs::write(config_path, format!("{content}\n")).map_err(|err| err.to_string())
}

fn build_event_summary(source: &str, event_name: &str, payload: &Value) -> String {
    let tool = event_tool_name(payload);
    let cmd = event_command(payload);
    let file_path = event_file_path(payload);
    let prompt = event_prompt(payload);

    let event_lower = event_name.to_lowercase();
    let info = lookup_event_info(event_name);

    // "stop"/"sessionend" uses dynamic source label
    if matches!(event_lower.as_str(), "stop" | "sessionend" | "session_end") {
        return format!("{} completed", format_source_label(source));
    }

    // "userpromptsubmit" with prompt text
    if matches!(
        event_lower.as_str(),
        "userpromptsubmit" | "user_prompt_submit"
    ) {
        if !prompt.is_empty() {
            return format!("Prompt: {}", truncate_chars(&prompt, 80));
        }
        return info.map(|i| i.summary.to_string()).unwrap_or_default();
    }

    if matches!(
        event_lower.as_str(),
        "permissionrequest" | "permission_request"
    ) {
        if !cmd.is_empty() {
            return format!("Waiting approval: {}", truncate_chars(&cmd, 72));
        }
        if !file_path.is_empty() && !tool.is_empty() {
            return format!(
                "Waiting approval: {} {}",
                tool,
                truncate_chars(&file_path, 56)
            );
        }
        if !tool.is_empty() {
            return format!("Waiting approval: {tool}");
        }
        return "Waiting approval".to_string();
    }

    // "pretooluse" with tool/cmd details
    if matches!(event_lower.as_str(), "pretooluse" | "pre_tool_use") {
        if !tool.is_empty() && !cmd.is_empty() {
            return format!("Running {}: {}", tool, truncate_chars(&cmd, 60));
        }
        if !tool.is_empty() && !file_path.is_empty() {
            return format!("Running {} on {}", tool, truncate_chars(&file_path, 56));
        }
        if !tool.is_empty() {
            return format!("Running {}", tool);
        }
        return "Running tool".to_string();
    }

    // "posttooluse" with tool name
    if matches!(event_lower.as_str(), "posttooluse" | "post_tool_use") {
        if !tool.is_empty() {
            return format!("{} finished", tool);
        }
        return "Tool finished".to_string();
    }

    // All other events use the static summary from lookup
    info.map(|i| i.summary.to_string()).unwrap_or_default()
}

fn build_event_detail(source: &str, event_name: &str, payload: &Value) -> String {
    let mut parts = Vec::new();
    parts.push(format!("Source: {}", format_source_label(source)));
    parts.push(format!("Event: {event_name}"));

    let tool = event_tool_name(payload);
    if !tool.is_empty() {
        parts.push(format!("Tool: {tool}"));
    }

    let prompt = event_prompt(payload);
    if !prompt.is_empty() {
        parts.push(format!("Prompt:\n{}", truncate_chars(&prompt, 900)));
    }

    let command = event_command(payload);
    if !command.is_empty() {
        parts.push(format!("Command:\n{}", truncate_chars(&command, 900)));
    }

    let file_path = event_file_path(payload);
    if !file_path.is_empty() {
        parts.push(format!("File:\n{}", truncate_chars(&file_path, 500)));
    }

    if let Some(input) = payload
        .get("tool_input")
        .or_else(|| payload.get("toolInput"))
        .or_else(|| payload.get("input"))
        .or_else(|| payload.get("parameters"))
        .or_else(|| payload.get("arguments"))
    {
        parts.push(format!(
            "Tool input:\n{}",
            truncate_chars(&json_preview(input), 1200)
        ));
    } else {
        parts.push(format!(
            "Payload:\n{}",
            truncate_chars(&json_preview(payload), 1200)
        ));
    }

    parts.join("\n\n")
}

fn event_tool_name(payload: &Value) -> String {
    payload
        .get("tool_name")
        .or_else(|| payload.get("toolName"))
        .or_else(|| payload.get("tool"))
        .or_else(|| nested_string(payload, &["toolCall", "name"]))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn event_command(payload: &Value) -> String {
    payload
        .get("command")
        .or_else(|| payload.get("cmd"))
        .or_else(|| nested_string(payload, &["tool_input", "command"]))
        .or_else(|| nested_string(payload, &["toolInput", "command"]))
        .or_else(|| nested_string(payload, &["input", "command"]))
        .or_else(|| nested_string(payload, &["parameters", "command"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "command"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "CommandLine"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "commandLine"]))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn event_file_path(payload: &Value) -> String {
    payload
        .get("file_path")
        .or_else(|| payload.get("filePath"))
        .or_else(|| payload.get("path"))
        .or_else(|| nested_string(payload, &["tool_input", "file_path"]))
        .or_else(|| nested_string(payload, &["toolInput", "filePath"]))
        .or_else(|| nested_string(payload, &["tool_input", "path"]))
        .or_else(|| nested_string(payload, &["toolInput", "path"]))
        .or_else(|| nested_string(payload, &["input", "path"]))
        .or_else(|| nested_string(payload, &["parameters", "path"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "path"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "Path"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "FilePath"]))
        .or_else(|| nested_string(payload, &["toolCall", "args", "DirectoryPath"]))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn event_prompt(payload: &Value) -> String {
    payload
        .get("prompt")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("reason"))
        .or_else(|| payload.get("questionPrompt"))
        .or_else(|| payload.get("question_prompt"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn json_preview(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn nested_string<'a>(payload: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = payload;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

/// Truncate a string to at most `max_chars` Unicode characters (byte-safe).
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Map source identifier to display label.
fn format_source_label(source: &str) -> &'static str {
    match source {
        "codex" => "Codex",
        "claude" => "Claude",
        "gemini" | "antigravity" => "Antigravity",
        "opencode" => "OpenCode",
        _ => "Agent",
    }
}

fn emit_runtime_warning(app: &AppHandle, message: &str) {
    let _ = app.emit(
        "runtime-warning",
        json!({
            "message": message
        }),
    );
}
