use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{main_window, persist_approval_rule, read_json_file, set_mode, AppState};

pub(crate) fn register_shortcuts(app: &AppHandle) {
    let config = read_json_file("defaults.json");
    let shortcuts = config.get("shortcuts").cloned().unwrap_or(default_shortcuts_json());
    register_from_config(app, &shortcuts);
}

pub(crate) fn reload_shortcuts(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
    register_shortcuts(app);
}

fn register_from_config(app: &AppHandle, shortcuts: &serde_json::Value) {
    let (decision_mod, toggle_mod) = platform_modifiers();

    let toggle_str = shortcuts.get("toggle").and_then(|v| v.as_str()).unwrap_or("Space");
    let approve_str = shortcuts.get("approve").and_then(|v| v.as_str()).unwrap_or("A");
    let always_str = shortcuts.get("approveAlways").and_then(|v| v.as_str()).unwrap_or("L");
    let deny_str = shortcuts.get("deny").and_then(|v| v.as_str()).unwrap_or("D");

    let resolve = |value: &str, default_modifier: &str| {
        if value.contains('+') { value.to_string() } else { format!("{default_modifier}+{value}") }
    };
    let toggle_full = resolve(toggle_str, &toggle_mod);
    let approve_full = resolve(approve_str, &decision_mod);
    let always_full = resolve(always_str, &decision_mod);
    let deny_full = resolve(deny_str, &decision_mod);

    register_one(app, &toggle_full, "toggle", |app_handle| {
        let _ = set_mode(&app_handle, "expanded");
        if let Ok(window) = main_window(&app_handle) {
            let _ = window.show();
            let _ = window.set_focus();
        }
    });
    register_one(app, &approve_full, "approve", |app_handle| {
        respond_to_intervention(&app_handle, "approve");
    });
    register_one(app, &always_full, "approve always", |app_handle| {
        respond_to_intervention(&app_handle, "approve_always");
    });
    register_one(app, &deny_full, "deny", |app_handle| {
        respond_to_intervention(&app_handle, "deny");
    });
}

fn platform_modifiers() -> (String, String) {
    if cfg!(target_os = "macos") {
        ("Cmd+Alt".into(), "Cmd+Shift".into())
    } else {
        ("Ctrl+Alt".into(), "Ctrl+Shift".into())
    }
}

fn register_one<F: Fn(AppHandle) + Send + Sync + 'static>(app: &AppHandle, shortcut_str: &str, label: &str, handler: F) {
    let Some((mods, code)) = parse_shortcut(shortcut_str) else {
        eprintln!("invalid shortcut for {label}: {shortcut_str}");
        return;
    };
    let app_handle = app.clone();
    if let Err(error) = app.global_shortcut().on_shortcut(
        Shortcut::new(Some(mods), code),
        move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                handler(app_handle.clone());
            }
        },
    ) {
        eprintln!("failed to register {label} shortcut: {error}");
        let _ = app.emit("runtime-warning", serde_json::json!({
            "message": format!("Shortcut unavailable: {label} key is in use.")
        }));
    }
}

// --- Config helpers ---

pub(crate) fn default_shortcuts_json() -> serde_json::Value {
    serde_json::json!({
        "toggle": "Space",
        "approve": "A",
        "approveAlways": "L",
        "deny": "D"
    })
}

pub(crate) fn display_shortcut(input: &str) -> String {
    if input.contains('+') {
        return input.replace('+', " ");
    }
    let (decision_mod, toggle_mod) = platform_modifiers();
    let (prefix, key) = if ["Space", "Enter", "Esc", "Up", "Down", "Left", "Right"].contains(&input) {
        (&toggle_mod, input)
    } else {
        (&decision_mod, input)
    };
    format!("{prefix} {key}")
}

pub(crate) fn parse_shortcut(input: &str) -> Option<(Modifiers, Code)> {
    let parts: Vec<&str> = input.split('+').map(str::trim).collect();
    if parts.len() < 2 {
        return None;
    }
    let mut mods = Modifiers::empty();
    for part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "cmd" | "super" | "win" => mods |= Modifiers::SUPER,
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" | "option" | "opt" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            _ => return None,
        }
    }
    let code = match parts.last()?.to_lowercase().as_str() {
        "space" => Code::Space,
        "enter" | "return" => Code::Enter,
        "escape" | "esc" => Code::Escape,
        "backspace" => Code::Backspace,
        "tab" => Code::Tab,
        "delete" | "del" => Code::Delete,
        "up" => Code::ArrowUp,
        "down" => Code::ArrowDown,
        "left" => Code::ArrowLeft,
        "right" => Code::ArrowRight,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" => Code::PageUp,
        "pagedown" => Code::PageDown,
        "insert" => Code::Insert,
        "f1" => Code::F1, "f2" => Code::F2, "f3" => Code::F3, "f4" => Code::F4,
        "f5" => Code::F5, "f6" => Code::F6, "f7" => Code::F7, "f8" => Code::F8,
        "f9" => Code::F9, "f10" => Code::F10, "f11" => Code::F11, "f12" => Code::F12,
        other => parse_alphanumeric(other)?,
    };
    Some((mods, code))
}

pub(crate) fn parse_code(s: &str) -> Option<Code> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "space" => Some(Code::Space),
        "enter" | "return" => Some(Code::Enter),
        "escape" | "esc" => Some(Code::Escape),
        "backspace" => Some(Code::Backspace),
        "tab" => Some(Code::Tab),
        "delete" | "del" => Some(Code::Delete),
        "up" => Some(Code::ArrowUp),
        "down" => Some(Code::ArrowDown),
        "left" => Some(Code::ArrowLeft),
        "right" => Some(Code::ArrowRight),
        "home" => Some(Code::Home),
        "end" => Some(Code::End),
        "pageup" => Some(Code::PageUp),
        "pagedown" => Some(Code::PageDown),
        "insert" => Some(Code::Insert),
        "f1" => Some(Code::F1), "f2" => Some(Code::F2), "f3" => Some(Code::F3), "f4" => Some(Code::F4),
        "f5" => Some(Code::F5), "f6" => Some(Code::F6), "f7" => Some(Code::F7), "f8" => Some(Code::F8),
        "f9" => Some(Code::F9), "f10" => Some(Code::F10), "f11" => Some(Code::F11), "f12" => Some(Code::F12),
        _ => parse_alphanumeric(s),
    }
}

fn parse_alphanumeric(s: &str) -> Option<Code> {
    let upper = s.to_uppercase();
    match upper.as_str() {
        "A" => Some(Code::KeyA), "B" => Some(Code::KeyB), "C" => Some(Code::KeyC), "D" => Some(Code::KeyD),
        "E" => Some(Code::KeyE), "F" => Some(Code::KeyF), "G" => Some(Code::KeyG), "H" => Some(Code::KeyH),
        "I" => Some(Code::KeyI), "J" => Some(Code::KeyJ), "K" => Some(Code::KeyK), "L" => Some(Code::KeyL),
        "M" => Some(Code::KeyM), "N" => Some(Code::KeyN), "O" => Some(Code::KeyO), "P" => Some(Code::KeyP),
        "Q" => Some(Code::KeyQ), "R" => Some(Code::KeyR), "S" => Some(Code::KeyS), "T" => Some(Code::KeyT),
        "U" => Some(Code::KeyU), "V" => Some(Code::KeyV), "W" => Some(Code::KeyW), "X" => Some(Code::KeyX),
        "Y" => Some(Code::KeyY), "Z" => Some(Code::KeyZ),
        "0" => Some(Code::Digit0), "1" => Some(Code::Digit1), "2" => Some(Code::Digit2), "3" => Some(Code::Digit3),
        "4" => Some(Code::Digit4), "5" => Some(Code::Digit5), "6" => Some(Code::Digit6), "7" => Some(Code::Digit7),
        "8" => Some(Code::Digit8), "9" => Some(Code::Digit9),
        _ => None,
    }
}

fn respond_to_intervention(app: &AppHandle, decision: &str) {
    let app_state = app.state::<AppState>();
    let mut pending = match app_state.intervention.lock() {
        Ok(pending) => pending,
        Err(_) => return,
    };
    let Some(mut current) = pending.take() else {
        return;
    };
    let approved = decision == "approve" || decision == "approve_always";
    let allow_persistent = decision == "approve_always";
    if approved && allow_persistent {
        persist_approval_rule(&current);
    }
    if let Some(responder) = current.responder.take() {
        let _ = responder.send(crate::InterventionDecision {
            approved,
            allow_persistent,
            answer: None,
        });
    }
    let _ = app.emit("intervention-state", Option::<serde_json::Value>::None);
    let _ = set_mode(app, "pill");
}
