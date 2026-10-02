pub mod adapters;
pub mod catalog;
pub mod db;
pub mod gateway;
pub mod gateway_key;
pub mod oauth;
pub mod oauth_secret;
pub mod pricing;
pub mod quota;
pub mod secrets;
pub mod tray;
#[cfg(windows)]
pub mod wincred;

pub use gateway::router::{AppState, GatewayPort};

use tauri::Manager;

pub const SHARED_PORT_CANDIDATE: u16 = 11434;
pub const PROVIDER_SLUGS: [&str; 5] = ["commandcode", "opencode", "chatgpt", "antigravity", "claude"];

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "proxy_dock=info".into()),
        )
        .init();
    // Single-instance first: a second launch focuses the existing main
    // window instead of starting a second gateway.
    #[cfg(desktop)]
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init());
    #[cfg(not(desktop))]
    let builder = tauri::Builder::default();
    builder
        .manage(GatewayPort::default())
        .manage(tray::DesktopPrefs::default())
        .invoke_handler(tauri::generate_handler![
            gateway::router::gateway_info,
            gateway_key::get_gateway_key,
            gateway_key::set_gateway_key,
            gateway_key::rotate_gateway_key,
            oauth_secret::antigravity_secret_status,
            oauth_secret::set_antigravity_secret,
            tray::set_close_behavior,
            tray::hide_main_window,
            tray::quit_app,
            secrets::local_token_status,
            secrets::set_local_token,
            secrets::clear_local_token,
            secrets::local_account_info,
            secrets::start_codex_sign_in,
            secrets::start_antigravity_sign_in,
            secrets::start_claude_sign_in,
            secrets::start_commandcode_sign_in,
            secrets::verify_commandcode_key,
            secrets::verify_opencode_key,
            secrets::verify_claude_key,
            quota::refresh_quota,
        ])
        .setup(|app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::MacosLauncher;
                let _ = app
                    .handle()
                    .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None));
                if let Err(err) = tray::build_tray(app) {
                    tracing::error!(error = %err, "tray setup failed");
                }
                // Keep the tray status line fresh (same GatewayPort source
                // the gateway_info command reads).
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
                    loop {
                        tick.tick().await;
                        tray::refresh_gateway_status(&handle);
                    }
                });
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = gateway::router::serve(handle).await {
                    tracing::error!(error = %err, "gateway failed");
                }
            });
            Ok(())
        })
        // Close button hides to tray (default); only "quit" behavior or the
        // tray-menu Quit exits.
        .on_window_event(|window, event| {
            if window.label() != tray::MAIN_WINDOW_LABEL {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let behavior = window
                    .app_handle()
                    .try_state::<tray::DesktopPrefs>()
                    .map(|prefs| prefs.get())
                    .unwrap_or_else(|| "tray".to_string());
                if tray::normalize_close_behavior(&behavior) == "tray" {
                    api.prevent_close();
                    let _ = window.hide();
                    tray::refresh_show_hide_label(window.app_handle());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("proxy-dock failed to run")
        .run(|app_handle, event| {
            // macOS dock click with no visible window reopens the app.
            #[cfg(target_os = "macos")]
            if matches!(event, tauri::RunEvent::Reopen { .. }) {
                tray::show_main_window(app_handle);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
