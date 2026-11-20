#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod audio;
mod diagnostics;
mod phone;
mod ringer;
mod ui;

use std::{sync::Arc, time::Duration};

fn main() -> eframe::Result {
    diagnostics::init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Could not start the SIP runtime");
    let path = dirs::config_dir()
        .expect("The account settings directory is unavailable")
        .join("com.softphone.desktop")
        .join("account.json");
    let phone = {
        let _entered = runtime.enter();
        Arc::new(phone::PhoneHandle::start(path))
    };
    let app_phone = phone.clone();
    let handle = runtime.handle().clone();
    let result = eframe::run_native(
        "Softphone",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([900.0, 820.0])
                .with_min_inner_size([780.0, 680.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(ui::SoftphoneApp::new(cc, app_phone, handle)))),
    );
    // Covers window-system exits and initialization failures as well as normal close.
    runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(10), phone.shutdown()).await;
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    tracing::info!(clean_exit = result.is_ok(), "Softphone session stopped");
    result
}

#[cfg(test)]
mod sip_routing_tests;

#[cfg(test)]
mod codec_tests;
