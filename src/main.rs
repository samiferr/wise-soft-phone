#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result {
    softphone::run_desktop()
}

#[cfg(target_os = "android")]
fn main() {}
