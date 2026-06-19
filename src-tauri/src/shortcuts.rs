use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{main_window, persist_approval_rule, set_mode, AppState, InterventionDecision};

pub(crate) fn register_shortcuts(app: &AppHandle) {
    let toggle_modifiers = platform_toggle_modifiers();
    let decision_modifiers = platform_decision_modifiers();

    let app_handle = app.clone();
    let toggle = Shortcut::new(Some(toggle_modifiers), Code::Space);
    if let Err(error) = app
        .global_shortcut()
        .on_shortcut(toggle, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = set_mode(&app_handle, "expanded");
                if let Ok(window) = main_window(&app_handle) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
    {
        eprintln!("failed to register toggle shortcut: {error}");
        emit_runtime_warning(
            app,
            "Shortcut unavailable: toggle key is in use. Check system or app shortcut conflicts.",
        );
    }

    let app_handle = app.clone();
    let approve = Shortcut::new(Some(decision_modifiers), Code::KeyA);
    if let Err(error) = app
        .global_shortcut()
        .on_shortcut(approve, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                respond_to_intervention(&app_handle, "approve");
            }
        })
    {
        eprintln!("failed to register approve shortcut: {error}");
        emit_runtime_warning(
            app,
            "Shortcut unavailable: approve key is in use. Check system or app shortcut conflicts.",
        );
    }

    let app_handle = app.clone();
    let always = Shortcut::new(Some(decision_modifiers), Code::KeyL);
    if let Err(error) = app
        .global_shortcut()
        .on_shortcut(always, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                respond_to_intervention(&app_handle, "approve_always");
            }
        })
    {
        eprintln!("failed to register approve-always shortcut: {error}");
        emit_runtime_warning(
            app,
            "Shortcut unavailable: always-approve key is in use. Check system or app shortcut conflicts.",
        );
    }

    let app_handle = app.clone();
    let deny = Shortcut::new(Some(decision_modifiers), Code::KeyD);
    if let Err(error) = app
        .global_shortcut()
        .on_shortcut(deny, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                respond_to_intervention(&app_handle, "deny");
            }
        })
    {
        eprintln!("failed to register deny shortcut: {error}");
        emit_runtime_warning(
            app,
            "Shortcut unavailable: deny key is in use. Check system or app shortcut conflicts.",
        );
    }
}

fn emit_runtime_warning(app: &AppHandle, message: &str) {
    let _ = app.emit(
        "runtime-warning",
        serde_json::json!({
            "message": message
        }),
    );
}

fn platform_toggle_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::SUPER | Modifiers::SHIFT
    } else {
        Modifiers::CONTROL | Modifiers::SHIFT
    }
}

fn platform_decision_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::SUPER | Modifiers::ALT
    } else {
        Modifiers::CONTROL | Modifiers::ALT
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
        let _ = responder.send(InterventionDecision {
            approved,
            allow_persistent,
            answer: None,
        });
    }
    let _ = app.emit("intervention-state", Option::<serde_json::Value>::None);
    let _ = set_mode(app, "pill");
}
