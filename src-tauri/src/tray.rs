// System tray + native app menu, localized.
//
// Menu item texts come from the same locale files the frontend uses
// (locales/<lang>.json, "tray" namespace). The language is resolved from the
// frontend's persisted choice (localStorage `i18nextLng`, read best-effort
// from the webview) with an English fallback; the frontend calls the
// `set_menu_language` command at startup (and after a language switch, which
// reloads the page) so the menus rebuild without an app restart.
//
// The tray menu also hosts the autostart toggle (tauri-plugin-autostart,
// default off — the plugin only registers a login item when enabled) as a
// CheckMenuItem, so the ✓ reflects the OS registration state.
//
// "check-update" (tray + native app menu) shows the main window and emits
// `updater://check-update`; UpdateCheckContext listens for it, opens the
// About dialog and runs the update check.

use std::sync::Mutex;

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt;

/// Supported UI languages (subset of locales/ the frontend ships).
const LANGS: [&str; 4] = ["en", "zh", "fr", "tr"];

/// Explicit language override set via `set_menu_language` (takes precedence
/// over the localStorage probe, which only runs at init before the webview
/// has necessarily loaded).
static CURRENT_LANG: Mutex<Option<String>> = Mutex::new(None);

/// Handle to the tray's autostart CheckMenuItem (Arc-backed, cheap to clone),
/// refreshed on every rebuild — lets the toggle handler update just the ✓.
static TRAY_AUTOSTART_ITEM: Mutex<Option<CheckMenuItem<tauri::Wry>>> = Mutex::new(None);

/// Tray/app-menu strings for one language, sourced from locales/<lang>.json.
#[derive(Clone, Debug, Default)]
struct TrayStrings {
    show: String,
    check_for_updates: String,
    about: String,
    settings: String,
    auto_start: String,
    quit: String,
    // ── native app menu, non-macOS only (window menu bar) ──
    #[cfg(not(target_os = "macos"))]
    m_file: String,
    #[cfg(not(target_os = "macos"))]
    m_help: String,
    m_edit: String,
    m_undo: String,
    m_redo: String,
    m_cut: String,
    m_copy: String,
    m_paste: String,
    m_select_all: String,
    m_close_window: String,
    // ── macOS-only native menu entries ──
    m_view: String,
    m_window: String,
    m_minimize: String,
    m_maximize: String,
    m_full_screen: String,
    m_services: String,
    m_hide: String,
    m_hide_others: String,
    m_quit_app: String,
}

/// Fill a string field from a JSON object, falling back to the English value.
macro_rules! s {
    ($obj:expr, $en:expr, $key:literal) => {
        $obj
            .get($key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                $en.get($key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
    };
}

/// Parse the "tray" + "menu" namespaces out of an embedded locale file.
fn tray_strings(lang: &str) -> Option<TrayStrings> {
    let raw = match lang {
        "zh" => include_str!("../../locales/zh.json"),
        "fr" => include_str!("../../locales/fr.json"),
        "tr" => include_str!("../../locales/tr.json"),
        _ => include_str!("../../locales/en.json"),
    };
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let tray = v.get("tray")?;
    let menu = v.get("menu").cloned().unwrap_or_default();
    let en: serde_json::Value =
        serde_json::from_str(include_str!("../../locales/en.json")).ok()?;
    let en_menu = en.get("menu").cloned().unwrap_or_default();
    let en_tray = en.get("tray").cloned().unwrap_or_default();
    Some(TrayStrings {
        show: tray.get("show")?.as_str()?.to_string(),
        check_for_updates: tray.get("checkForUpdates")?.as_str()?.to_string(),
        about: s!(tray, en_tray, "about"),
        settings: s!(tray, en_tray, "settings"),
        auto_start: tray.get("autoStart")?.as_str()?.to_string(),
        quit: tray.get("quit")?.as_str()?.to_string(),
        m_edit: s!(menu, en_menu, "edit"),
        #[cfg(not(target_os = "macos"))]
        m_file: s!(menu, en_menu, "file"),
        #[cfg(not(target_os = "macos"))]
        m_help: s!(menu, en_menu, "help"),
        m_undo: s!(menu, en_menu, "undo"),
        m_redo: s!(menu, en_menu, "redo"),
        m_cut: s!(menu, en_menu, "cut"),
        m_copy: s!(menu, en_menu, "copy"),
        m_paste: s!(menu, en_menu, "paste"),
        m_select_all: s!(menu, en_menu, "selectAll"),
        m_close_window: s!(menu, en_menu, "closeWindow"),
        m_view: s!(menu, en_menu, "view"),
        m_window: s!(menu, en_menu, "window"),
        m_minimize: s!(menu, en_menu, "minimize"),
        m_maximize: s!(menu, en_menu, "maximize"),
        m_full_screen: s!(menu, en_menu, "fullScreen"),
        m_services: s!(menu, en_menu, "services"),
        m_hide: s!(menu, en_menu, "hide"),
        m_hide_others: s!(menu, en_menu, "hideOthers"),
        m_quit_app: s!(menu, en_menu, "quitApp").replace("{{app}}", "MCPHub Desktop"),
    })
}

/// Normalize a BCP-47-ish tag to a supported base language, or None.
fn normalize_lang(lang: &str) -> Option<String> {
    let base = lang.split(['-', '_']).next().unwrap_or("").to_string();
    LANGS
        .contains(&base.as_str())
        .then_some(base)
}

/// Resolve the current menu language: explicit override first, then the
/// frontend's persisted localStorage value, then English. Only meaningful at
/// init — later switches come through `set_menu_language`.
fn resolve_lang(app: &AppHandle<tauri::Wry>) -> String {
    if let Some(lang) = CURRENT_LANG.lock().unwrap().clone() {
        return lang;
    }
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    if let Some(win) = app.get_webview_window("main") {
        let js = "localStorage.getItem('i18nextLng')".to_string();
        let ok = win
            .eval_with_callback(js, move |result: String| {
                // result is the JSON-serialized return value: `"zh"` or `null`.
                let parsed: Option<String> = serde_json::from_str(&result).unwrap_or(None);
                let _ = tx.send(parsed.and_then(|s| normalize_lang(&s)));
            })
            .is_ok();
        if ok {
            return rx.recv().ok().flatten().unwrap_or_else(|| "en".into());
        }
    }
    "en".into()
}

/// Show the main window and bring it to front (shared by tray left-click,
/// the "show" menu item, and the check-update action).
fn show_main_window(app: &AppHandle<tauri::Wry>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Frontend command: open a URL in the OS default browser. Tauri 2's webview
/// doesn't reliably open `<a target="_blank">` http(s) links in the system
/// browser, and invoking the shell plugin's `open` over the IPC needs the
/// dedicated JS package; instead we spawn the OS opener directly here so the
/// frontend can use a plain `invoke`.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let result = {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(&url).spawn()
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open").arg(&url).spawn()
        }
    };
    result
        .map(|_| ())
        .map_err(|e| format!("failed to open URL {url}: {e}"))
}

/// Handle a custom menu item id from the tray menu or the native app menu.
fn on_menu_action(app: &AppHandle<tauri::Wry>, id: &str) {
    match id {
        "quit" => {
            // Disconnect all MCP upstream clients (shared pool + per-session
            // isolated) before quitting so child processes are reaped via
            // kill_process_tree rather than orphaned at process exit.
            let app_handle = app.clone();
            tauri::async_runtime::block_on(async move {
                crate::mcp::pool::disconnect_all().await;
                let _ = app_handle;
            });
            app.exit(0);
        }
        "show" => show_main_window(app),
        "settings" => {
            // Frontend navigates to /settings (see the `nav://navigate`
            // listener in UpdateCheckContext).
            show_main_window(app);
            let _ = app.emit("nav://navigate", "settings");
        }
        "check-update" => {
            // Frontend opens the About dialog and runs the update check
            // (see UpdateCheckContext's `updater://check-update` listener).
            show_main_window(app);
            let _ = app.emit("updater://check-update", ());
        }
        "about" => {
            // Frontend opens the About dialog (same as check-update but
            // without triggering a fresh update check).
            show_main_window(app);
            let _ = app.emit("updater://open-about", ());
        }
        "autostart" => {
            // Toggle off the main thread: the menu-event callback runs ON the
            // main thread, and menu mutations dispatch back to the main thread
            // synchronously (run_item_main_thread!) — rebuilding menus here
            // would deadlock. The spawned task flips only the check item's ✓
            // via the stashed handle instead of rebuilding both menus.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let autostart = app.autolaunch();
                let enabled = if autostart.is_enabled().unwrap_or(false) {
                    autostart.disable().is_ok()
                } else {
                    autostart.enable().is_ok()
                };
                // Re-read the OS state so the ✓ reflects reality even when
                // enable/disable failed (state unchanged).
                let now = app.autolaunch().is_enabled().unwrap_or(false);
                log::info!("[tray] autostart toggled -> requested={}, actual={}", enabled, now);
                if let Some(item) = TRAY_AUTOSTART_ITEM.lock().unwrap().clone() {
                    let _ = item.set_checked(now);
                }
            });
        }
        _ => {}
    }
}

/// Build (or rebuild) the tray icon menu and the native app menu with the
/// strings for the current language. Idempotent: safe to call on every
/// language change; the tray's check state is re-read from the OS each time.
pub fn rebuild_menus(app: &AppHandle<tauri::Wry>) -> tauri::Result<()> {
    let strings = tray_strings(&resolve_lang(app)).unwrap_or_default();

    // ── Tray menu ──────────────────────────────────────────────────────────
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let show = MenuItem::with_id(app, "show", &strings.show, true, None::<&str>)?;
    let check_update =
        MenuItem::with_id(app, "check-update", &strings.check_for_updates, true, None::<&str>)?;
    let about = MenuItem::with_id(app, "about", &strings.about, true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", &strings.settings, true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        &strings.auto_start,
        true,
        autostart_enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", &strings.quit, true, None::<&str>)?;
    let tray_menu =
        Menu::with_items(app, &[&show, &settings, &autostart, &check_update, &about, &quit])?;
    // Keep the check-item handle so the toggle handler can flip the ✓ without
    // rebuilding both menus (a full rebuild from the main-thread menu callback
    // would deadlock on run_item_main_thread dispatch).
    *TRAY_AUTOSTART_ITEM.lock().unwrap() = Some(autostart);

    if let Some(tray) = app.tray_by_id("mcphub-tray") {
        tray.set_menu(Some(tray_menu))?;
    } else {
        let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
            .expect("failed to load tray icon");
        TrayIconBuilder::with_id("mcphub-tray")
            .icon(tray_icon)
            .menu(&tray_menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| on_menu_action(app, event.id.as_ref()))
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main_window(tray.app_handle());
                }
            })
            .build(app)?;
    }

    // ── Native app menu ────────────────────────────────────────────────────
    // Fully self-built (NOT Menu::default, whose submenu titles and predefined
    // item texts are hardcoded English): macOS gets the app-name submenu with
    // About/Services/Hide/Quit, plus Edit/View/Window; other platforms get
    // File/Edit/Window/Help as a window menu bar. Predefined items keep their
    // native behavior/accelerators but take localized text.
    let app_menu = build_app_menu(app, &strings)?;
    app_menu.set_as_app_menu()?;

    // Global menu-event handler — covers the native app menu (macOS top-left
    // "Check for Updates") in addition to the tray handler registered above.
    // Safe to register repeatedly: each call adds a listener, but
    // rebuild_menus only runs at init / language switch, so at most a
    // handful of identical handlers exist.
    app.on_menu_event(|app, event| on_menu_action(app, event.id.as_ref()));

    Ok(())
}

/// Build the localized native app menu. Mirrors Tauri's `Menu::default`
/// structure (macOS: app submenu + Edit/View/Window; others: File/Edit/
/// Window/Help) but with every title/text taken from the locale files.
fn build_app_menu(
    app: &AppHandle<tauri::Wry>,
    s: &TrayStrings,
) -> tauri::Result<Menu<tauri::Wry>> {
    let check_update = MenuItem::with_id(
        app,
        "check-update",
        &s.check_for_updates,
        true,
        None::<&str>,
    )?;

    let settings = MenuItem::with_id(app, "settings", &s.settings, true, None::<&str>)?;

    let edit_menu = Submenu::with_id_and_items(
        app,
        HELP_SUBMENU_ID_EDIT,
        &s.m_edit,
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(&s.m_undo))?,
            &PredefinedMenuItem::redo(app, Some(&s.m_redo))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some(&s.m_cut))?,
            &PredefinedMenuItem::copy(app, Some(&s.m_copy))?,
            &PredefinedMenuItem::paste(app, Some(&s.m_paste))?,
            &PredefinedMenuItem::select_all(app, Some(&s.m_select_all))?,
        ],
    )?;

    let window_menu = Submenu::with_id_and_items(
        app,
        "__mcphub_window_menu__",
        &s.m_window,
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some(&s.m_minimize))?,
            &PredefinedMenuItem::maximize(app, Some(&s.m_maximize))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some(&s.m_close_window))?,
        ],
    )?;

    let about = MenuItem::with_id(app, "about", &s.about, true, None::<&str>)?;

    #[cfg(target_os = "macos")]
    {
        // The app-name submenu: About (frontend dialog) / separator /
        // Settings / Check for Updates / separator / Services / separator /
        // Hide / Hide Others / separator / Quit.
        let app_sub = Submenu::with_items(
            app,
            "MCPHub Desktop",
            true,
            &[
                &about,
                &PredefinedMenuItem::separator(app)?,
                &settings,
                &check_update,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, Some(&s.m_services))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, Some(&s.m_hide))?,
                &PredefinedMenuItem::hide_others(app, Some(&s.m_hide_others))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, Some(&s.m_quit_app))?,
            ],
        )?;
        let view_menu = Submenu::with_items(
            app,
            &s.m_view,
            true,
            &[&PredefinedMenuItem::fullscreen(app, Some(&s.m_full_screen))?],
        )?;
        Menu::with_items(app, &[&app_sub, &edit_menu, &view_menu, &window_menu])
    }

    #[cfg(not(target_os = "macos"))]
    {
        let file_menu = Submenu::with_items(
            app,
            &s.m_file,
            true,
            &[
                &settings,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::close_window(app, Some(&s.m_close_window))?,
                &PredefinedMenuItem::quit(app, Some(&s.m_quit_app))?,
            ],
        )?;
        let help_menu = Submenu::with_id_and_items(
            app,
            "__mcphub_help_menu__",
            &s.m_help,
            true,
            &[&check_update, &about],
        )?;
        Menu::with_items(app, &[&file_menu, &edit_menu, &window_menu, &help_menu])
    }
}

const HELP_SUBMENU_ID_EDIT: &str = "__mcphub_edit_menu__";

/// Frontend command: switch the tray/app menu language (called at startup
/// once i18n initializes, and by the language switcher after a change).
#[tauri::command]
pub fn set_menu_language(app: AppHandle, lang: String) -> Result<(), String> {
    if let Some(base) = normalize_lang(&lang) {
        *CURRENT_LANG.lock().unwrap() = Some(base);
    }
    rebuild_menus(&app).map_err(|e| e.to_string())
}
