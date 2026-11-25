mod account;
#[cfg(target_os = "android")]
mod android_input;
mod audio;
mod credentials;
mod diagnostics;
mod library;
mod phone;
mod player;
mod recording;
mod ringer;
mod ui;

use std::{sync::Arc, time::Duration};

#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result {
    run(
        dirs::config_dir()
            .expect("The account settings directory is unavailable")
            .join("com.softphone.desktop"),
        Default::default(),
    )
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    let directory = app
        .internal_data_path()
        .expect("Android app storage is unavailable");
    android_input::init(app.clone());
    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };
    if let Err(error) = run(directory, options) {
        eprintln!("Could not open Wise Softphone: {error}");
    }
}

/// Windows installs have no project folder, so logs go to the per-user local
/// application data directory. Other desktops keep the project `logs` folder.
#[cfg(not(target_os = "android"))]
fn desktop_log_directory() -> std::path::PathBuf {
    if cfg!(windows) {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("com.softphone.desktop")
            .join("logs")
    } else {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logs")
    }
}

fn run(directory: std::path::PathBuf, native_options: eframe::NativeOptions) -> eframe::Result {
    #[cfg(target_os = "android")]
    diagnostics::init(directory.join("logs"));
    #[cfg(not(target_os = "android"))]
    diagnostics::init(desktop_log_directory());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Could not start the SIP runtime");
    let path = directory.join("account.json");
    let phone = {
        let _entered = runtime.enter();
        Arc::new(phone::PhoneHandle::start(path.clone()))
    };
    let app_phone = phone.clone();
    let handle = runtime.handle().clone();
    let result = eframe::run_native(
        "Softphone",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size(ui::PHONE_SIZE)
                .with_max_inner_size(ui::PHONE_SIZE)
                .with_min_inner_size([240.0, 360.0])
                .with_maximize_button(false)
                .with_resizable(false),
            renderer: eframe::Renderer::Glow,
            ..native_options
        },
        Box::new(move |cc| Ok(Box::new(ui::SoftphoneApp::new(cc, app_phone, handle, path)))),
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
