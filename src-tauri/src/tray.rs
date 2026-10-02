//! Stream A: tray icon + main-window lifecycle.
//!
//! macOS keeps both Dock and tray icons (default). Closing the main window
//! hides to tray on both OSes; only the tray-menu Quit (or Cmd+Q) exits.
//! Gateway status is read from the shared [`crate::GatewayPort`] state — the
//! existing `gateway_info` command stays the single IPC status source.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, Runtime, State};

/// Stable tray menu item ids (other streams may reference these).
pub const MENU_SHOW_HIDE: &str = "show-hide";
pub const MENU_GATEWAY_STATUS: &str = "gateway-status";
pub const MENU_OPEN_DASHBOARD: &str = "open-dashboard";
pub const MENU_LAUNCH_ON_LOGIN: &str = "launch-on-login";
pub const MENU_QUIT: &str = "quit";

pub const TRAY_ID: &str = "main-tray";
pub const MAIN_WINDOW_LABEL: &str = "main";

const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon.png");

/// Close-behavior preference, `"tray"` (default) or `"quit"`.
pub struct DesktopPrefs {
    pub close_behavior: Mutex<String>,
}

impl Default for DesktopPrefs {
    fn default() -> Self {
        Self { close_behavior: Mutex::new("tray".to_string()) }
    }
}

impl DesktopPrefs {
    pub fn get(&self) -> String {
        self.close_behavior.lock().ok().map(|b| b.clone()).unwrap_or_else(|| "tray".to_string())
    }
}

/// Live menu-item handles for text updates. Stored as managed state.
pub struct TrayMenuRefs<R: Runtime> {
    pub show_hide: tauri::menu::MenuItem<R>,
    pub status: tauri::menu::MenuItem<R>,
    pub autostart: tauri::menu::CheckMenuItem<R>,
}

/// Normalize a close-behavior value; anything but `"quit"` means `"tray"`.
pub fn normalize_close_behavior(behavior: &str) -> &'static str {
    if behavior == "quit" {
        "quit"
    } else {
        "tray"
    }
}

/// Tray status line for a bound gateway port. Pure for unit tests.
pub fn gateway_status_text(port: Option<u16>) -> String {
    match port {
        Some(p) => format!("Gateway running :{p}"),
        None => "Gateway stopped".to_string(),
    }
}

/// Toggle label for the show/hide menu item given window visibility.
pub fn show_hide_label(visible: bool) -> &'static str {
    if visible {
        "Hide Proxy Dock"
    } else {
        "Show Proxy Dock"
    }
}

/// Set the close behavior (`"tray"` | `"quit"`); returns the normalized value.
#[tauri::command]
pub fn set_close_behavior(prefs: State<'_, DesktopPrefs>, behavior: String) -> String {
    let normalized = normalize_close_behavior(&behavior).to_string();
    if let Ok(mut guard) = prefs.close_behavior.lock() {
        *guard = normalized.clone();
    }
    normalized
}

/// Hide the main window (to tray).
#[tauri::command]
pub fn hide_main_window(app: AppHandle<tauri::Wry>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.hide();
    }
    refresh_show_hide_label(&app);
}

/// Exit the application immediately.
#[tauri::command]
pub fn quit_app(app: AppHandle<tauri::Wry>) {
    app.exit(0);
}

/// Show, unminimize, and focus the main window.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    refresh_show_hide_label(app);
}

/// Toggle main-window visibility (tray `show-hide` item).
pub fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) {
    match app.get_webview_window(MAIN_WINDOW_LABEL) {
        Some(window) => {
            if window.is_visible().unwrap_or(true) {
                let _ = window.hide();
                refresh_show_hide_label(app);
            } else {
                show_main_window(app);
            }
        }
        None => {}
    }
}

/// Sync the `show-hide` label with actual window visibility.
pub fn refresh_show_hide_label<R: Runtime>(app: &AppHandle<R>) {
    let visible = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if let Some(refs) = app.try_state::<TrayMenuRefs<R>>() {
        let _ = refs.show_hide.set_text(show_hide_label(visible));
    }
}

/// Read the bound port from [`crate::GatewayPort`] and update the
/// `gateway-status` menu line. No duplicate status command: same source as
/// the existing `gateway_info` command.
pub fn refresh_gateway_status<R: Runtime>(app: &AppHandle<R>) {
    let port = app
        .try_state::<crate::GatewayPort>()
        .and_then(|slot| slot.0.lock().ok().and_then(|guard| *guard));
    if let Some(refs) = app.try_state::<TrayMenuRefs<R>>() {
        let _ = refs.status.set_text(gateway_status_text(port));
    }
}

fn toggle_launch_on_login<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let next = !manager.is_enabled().unwrap_or(false);
    let _ = if next { manager.enable() } else { manager.disable() };
    if let Some(refs) = app.try_state::<TrayMenuRefs<R>>() {
        let _ = refs.autostart.set_checked(next);
    }
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        MENU_SHOW_HIDE => toggle_main_window(app),
        MENU_OPEN_DASHBOARD => show_main_window(app),
        MENU_LAUNCH_ON_LOGIN => toggle_launch_on_login(app),
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

/// Build the tray icon + menu. The bundled `tray-icon.png` wins; falls back
/// to the app window icon so a missing/invalid placeholder never breaks setup.
pub fn build_tray<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;

    let autostart_checked = {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().unwrap_or(false)
    };

    let show_hide = MenuItem::with_id(app, MENU_SHOW_HIDE, "Show Proxy Dock", true, None::<&str>)?;
    let status = MenuItem::with_id(app, MENU_GATEWAY_STATUS, "Gateway starting…", false, None::<&str>)?;
    let open_dashboard = MenuItem::with_id(app, MENU_OPEN_DASHBOARD, "Open Dashboard", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        MENU_LAUNCH_ON_LOGIN,
        "Launch on Login",
        true,
        autostart_checked,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit Proxy Dock", true, None::<&str>)?;
    let sep_a = PredefinedMenuItem::separator(app)?;
    let sep_b = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[&show_hide, &status, &sep_a, &open_dashboard, &autostart, &sep_b, &quit],
    )?;

    app.manage(TrayMenuRefs { show_hide: show_hide.clone(), status: status.clone(), autostart: autostart.clone() });

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Proxy Dock")
        .on_menu_event(handle_menu_event);
    let icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)
        .ok()
        .or_else(|| app.default_window_icon().cloned());
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }
    builder.build(app)?;

    refresh_show_hide_label(&app.handle());
    refresh_gateway_status(&app.handle());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_shows_running_port() {
        assert_eq!(gateway_status_text(Some(11434)), "Gateway running :11434");
        assert_eq!(gateway_status_text(Some(1)), "Gateway running :1");
    }

    #[test]
    fn status_text_without_port_means_stopped() {
        assert_eq!(gateway_status_text(None), "Gateway stopped");
    }

    #[test]
    fn close_behavior_normalizes_to_tray_or_quit() {
        assert_eq!(normalize_close_behavior("tray"), "tray");
        assert_eq!(normalize_close_behavior("quit"), "quit");
        assert_eq!(normalize_close_behavior(""), "tray");
        assert_eq!(normalize_close_behavior("QUIT"), "tray");
        assert_eq!(normalize_close_behavior("anything-else"), "tray");
    }

    #[test]
    fn prefs_default_to_tray() {
        assert_eq!(DesktopPrefs::default().get(), "tray");
    }

    #[test]
    fn show_hide_label_follows_visibility() {
        assert_eq!(show_hide_label(true), "Hide Proxy Dock");
        assert_eq!(show_hide_label(false), "Show Proxy Dock");
    }
}
