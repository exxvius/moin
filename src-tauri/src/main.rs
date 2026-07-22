// Prevent an extra console window on Windows in release. See
// https://tauri.app/v1/guides/building/building-windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    moin_lib::run();
}
