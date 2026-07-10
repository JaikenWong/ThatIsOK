#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if agentisok_lib::run_hook_bridge_from_args() {
        return;
    }
    agentisok_lib::run()
}
