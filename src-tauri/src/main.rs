#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if thatisok_lib::run_hook_bridge_from_args() {
        return;
    }
    thatisok_lib::run()
}
