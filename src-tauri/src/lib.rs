// objc 0.2 宏在新版 Rust 下会产生 unexpected_cfgs 警告，全局抑制
#![allow(unexpected_cfgs)]

pub mod auth;
pub mod commands;
pub mod db;
pub mod mcp;
pub mod models;
pub mod rag;
pub mod services;

use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

// ── crash logging ────────────────────────────────────────────────────────────
// A panic hook + fatal-error writer so a startup crash - notably the Windows
// "launched-by-installer first-run flash-exit" - leaves a trace in
// `<app_data_dir>/crash.log` instead of dying silently with no window. The hook
// is installed early in `run()` (stderr-only until the app data dir is known)
// and `CRASH_DIR` is populated at the top of `setup()` once the real dir is
// resolved, so panics/errors in the DB-init path (the most common silent-crash
// source: `app_data_dir()` failing when launched elevated by the installer) are
// captured to the file. Combined with NSIS `installMode: "currentUser"` (no
// elevated auto-launch), this both fixes the common cause and makes any
// residual crash diagnosable.

static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Write a crash/fatal line to stderr and, if the app data dir is known, append
/// it to `<app_data_dir>/crash.log`. Best-effort: any IO failure is swallowed
/// (stderr already has the line) so this never masks the original error.
fn write_crash_log(prefix: &str, msg: &str) {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("[{}] [{}] {}\n", stamp, prefix, msg);
    eprint!("{}", line);
    if let Some(dir) = CRASH_DIR.get() {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("crash.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = std::io::Write::write_all(&mut f, line.as_bytes());
        }
    }
}

/// Install the global panic hook. `force_capture` yields a backtrace even
/// without `RUST_BACKTRACE=1`. Must run before any code that can panic.
fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        write_crash_log("panic", &format!("{}\n{}", info, bt));
    }));
}

// ── Windows de-elevation ─────────────────────────────────────────────────────
// A desktop app shouldn't run elevated. When the NSIS installer's "launch on
// finish" auto-starts the app after a per-machine (elevated) install, the
// process runs with a high-integrity token - and that elevated first-launch is
// exactly what crashes on Windows (the non-elevated manual relaunch works).
// `relaunch_if_elevated` detects the elevated token and re-launches this exe
// non-elevated via explorer.exe (the existing non-elevated shell opens the
// target at medium integrity), then exits. The relaunched child is
// non-elevated, so the check returns false there - no loop. This lets us keep
// `installMode: "both"` (per-machine option) WITHOUT the first-launch crash.
// cfg(windows)-gated; on mac/linux this module is absent (cargo check there
// never compiles it - verify via the Windows CI build).
#[cfg(windows)]
mod win_deelevate {
    use std::process::Command;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// True iff the current process runs with an elevated (high-integrity) token.
    fn is_elevated() -> bool {
        unsafe {
            let mut token = windows::Win32::Foundation::HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elev = TOKEN_ELEVATION::default();
            let mut ret_len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elev as *mut _ as *mut std::ffi::c_void),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len,
            );
            let _ = CloseHandle(token);
            ok.is_ok() && elev.TokenIsElevated != 0
        }
    }

    /// If running elevated, relaunch this exe non-elevated via explorer.exe and
    /// exit. No-op when not elevated. If the explorer relaunch somehow fails we
    /// fall through and run elevated (better than not starting at all); the
    /// crash hook + crash.log still catch any subsequent panic.
    pub fn relaunch_if_elevated() {
        if !is_elevated() {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => return,
        };
        if Command::new("explorer.exe")
            .arg(exe.as_os_str())
            .spawn()
            .is_ok()
        {
            std::process::exit(0);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Windows symlink-helper mode: when relaunched with --symlink-helper, create
    // the symlinks from the manifest and exit — never start Tauri UI. Used by
    // skill_service::create_symlinks_elevated for on-demand elevation (one UAC
    // per batch). See doc/agent_20260724.md §3.8.5.
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--symlink-helper") {
            services::skill_service::run_helper_mode();
        }
        // De-elevate: if launched elevated (NSIS installer's "launch on finish"
        // after a per-machine install runs the app with a high-integrity token),
        // relaunch ourselves non-elevated via explorer.exe (the existing
        // non-elevated shell opens the target at medium integrity) and exit.
        // This is what fixes the Windows "first launch from installer crashes,
        // manual relaunch works" symptom - the elevated auto-launch is exactly
        // the crashing path; the non-elevated relaunch is the working one.
        // Must run BEFORE the heavy init (db/runtimes/webview). The relaunched
        // process is non-elevated, so the check returns false there - no loop.
        // Skipped for --symlink-helper (that mode is intentionally elevated).
        win_deelevate::relaunch_if_elevated();
    }

    // On macOS dev mode, set the process display name so the Dock shows "MCPHub Desktop"
    #[cfg(target_os = "macos")]
    unsafe {
        use objc::{class, msg_send, sel, sel_impl};
        use objc::runtime::Object;
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let ns_str: *mut Object = msg_send![class!(NSString),
            stringWithUTF8String: b"MCPHub Desktop\0".as_ptr() as *const std::ffi::c_char];
        let _: () = msg_send![info, setProcessName: ns_str];
    }

    // Initialize env_logger for stderr output with local time format.
    // Database logging is handled separately by app_logger.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            use chrono::Local;
            use std::io::Write;
            let now = Local::now();
            writeln!(
                buf,
                "{} [{:<5}] {}",
                now.format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .init();
    // Install the crash hook BEFORE the builder so panics during plugin init /
    // setup (e.g. DB creation on Windows first launch) are captured to
    // crash.log. stderr-only until `setup()` resolves the app data dir.
    install_crash_hook();
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            log::info!("Single instance triggered with args: {:?}, cwd: {:?}", argv, cwd);

            // 当第二个实例尝试启动时，聚焦到已有窗口
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            // Resolve the app data dir FIRST so the crash hook can write to
            // <app_data_dir>/crash.log during the heavy first-run init below
            // (DB creation, runtime init, MCP start). This is the path that
            // matters for the Windows "first launch from installer" crash.
            if let Ok(dir) = app.path().app_data_dir() {
                let _ = CRASH_DIR.set(dir);
            }

            // Register session state
            app.manage(commands::auth::SessionState(tokio::sync::Mutex::new(None)));

            // Initialize bundled Node.js / Python runtimes.
            // In production, they live in the app's resource directory.
            // In development (cargo tauri dev), fall back to src-tauri/runtimes/.
            {
                let resource_runtimes = app
                    .path()
                    .resource_dir()
                    .ok()
                    .map(|d| d.join("runtimes"))
                    .filter(|p| p.exists());

                let dev_runtimes = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes");

                let runtimes = resource_runtimes.unwrap_or(dev_runtimes);
                services::runtime_env::init(runtimes);
            }

            // Stash the AppHandle so transports/pool can emit server progress
            // events (download progress, update-available) without threading it
            // through every call site. Must be set before any server connects.
            mcp::progress::set_app_handle(app.handle().clone());

            // Initialize the database: spawn async task, block current thread via channel
            let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let result = db::initialize(&app_handle).await
                    .map_err(|e| format!("{:#}", e));
                tx.send(result).ok();
            });
            // `recv().unwrap()` used to panic (silent flash-exit) if the spawned
            // task panicked (e.g. `app_data_dir()` failing under the elevated
            // installer launch) - the sender would drop and recv() return None.
            // Handle both the Err case and the task-panic case explicitly so the
            // reason lands in crash.log instead of vanishing.
            match rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    write_crash_log("fatal", &format!("Database initialization failed: {e}"));
                    std::process::exit(1);
                }
                Err(_) => {
                    // The db::initialize task panicked; the panic hook already
                    // wrote the backtrace to crash.log.
                    write_crash_log("fatal", "Database initialization task panicked (see panic entry above).");
                    std::process::exit(1);
                }
            }

            // Initialize database log writer (+ file mirror under <app_data_dir>/logs/)
            services::app_logger::init(app.path().app_data_dir().ok());
            services::app_logger::log_to_db("info", "Application started, database initialized");

            // Log the enhanced PATH at startup (after DB is ready so it's persisted)
            {
                let path = commands::runtime::get_enhanced_path_for_logging();
                log::info!("[startup] Enhanced PATH: {}", path);
                services::app_logger::log_to_db("info", &format!("[startup] Enhanced PATH: {}", path));
            }

            // Start MCP servers and HTTP server in background after DB is ready
            let app_handle2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Ensure default admin user exists
                if let Err(e) = services::user_service::ensure_default_admin().await {
                    log::error!("Failed to create default admin user: {}", e);
                }

                // Load persisted active runtime versions from DB and apply them
                if let Ok(cfg) = services::config_service::get().await {
                    if let Some(node_ver) = cfg
                        .get("install")
                        .and_then(|i| i.get("nodeVersion"))
                        .and_then(|v| v.as_str())
                    {
                        services::runtime_env::set_active_node(node_ver.to_string());
                    }
                    if let Some(py_ver) = cfg
                        .get("install")
                        .and_then(|i| i.get("pythonVersion"))
                        .and_then(|v| v.as_str())
                    {
                        services::runtime_env::set_active_python(py_ver.to_string());
                    }
                }
                // Reconcile skills: clean up any pending imports/exports left by
                // a crash so the next start doesn't treat them as installed.
                if let Err(e) = services::skill_service::reconcile_pending(&app_handle2).await {
                    log::warn!("[skills] reconcile_pending failed: {}", e);
                }
                if let Err(e) = services::mcp_manager::start_all(&app_handle2).await {
                    log::error!("Failed to start MCP servers: {}", e);
                }
                services::http_server::maybe_start().await;

                // Auto-restore RAG if it was enabled before restart. Reads the
                // persisted `rag.enabled` intent; if true, load the embedding
                // model + open the vector DB so /mcp rag_search/rag_get work
                // immediately. The frontend syncs the switch via rag_status.
                if rag::service::config_enabled().await {
                    let app_for_rag = app_handle2.clone();
                    tokio::spawn(async move {
                        if let Err(e) = rag::service::start(&app_for_rag).await {
                            log::error!("[RAG] auto-start on boot failed: {:#}", e);
                        }
                    });
                }

                // Periodic log cleanup: run every 6 hours, first run after 5 minutes
                tokio::spawn(async {
                    // Wait 5 minutes before first cleanup to avoid startup contention
                    tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;
                    loop {
                        // run_cleanup_with_summary already logs to both stderr and database
                        let _ = services::log_service::run_cleanup_with_summary().await;
                        tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
                    }
                });
            });

            // Set up system tray icon with menu
            let quit = MenuItem::with_id(app, "quit", "Quit MCPHub", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
                .expect("failed to load tray icon");

            TrayIconBuilder::new()
                .icon(tray_icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        // Disconnect all MCP upstream clients (shared pool +
                        // per-session isolated) before quitting so child
                        // processes are reaped via kill_process_tree rather
                        // than orphaned at process exit. Runs synchronously on
                        // the runtime so the teardown completes before exit.
                        let app_handle = app.clone();
                        tauri::async_runtime::block_on(async move {
                            crate::mcp::pool::disconnect_all().await;
                            let _ = app_handle;
                        });
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize to tray on close instead of quitting
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // App update (cancelable install)
            commands::updater::install_update_cancelable,
            commands::updater::cancel_update_install,
            // Auth commands
            commands::auth::login,
            commands::auth::register,
            commands::auth::logout,
            commands::auth::get_current_user,
            commands::auth::change_password,
            // Server commands
            commands::servers::list_servers,
            commands::servers::get_server,
            commands::servers::add_server,
            commands::servers::update_server,
            commands::servers::delete_server,
            commands::servers::toggle_server,
            commands::servers::reload_server,
            commands::servers::reinstall_server,
            commands::servers::clear_cache,
            // Group commands
            commands::groups::list_groups,
            commands::groups::add_group,
            commands::groups::update_group,
            commands::groups::delete_group,
            // Tool commands
            commands::tools::list_tools,
            commands::tools::call_tool,
            // User commands
            commands::users::list_users,
            commands::users::add_user,
            commands::users::update_user,
            commands::users::delete_user,
            // Config commands
            commands::config::get_system_config,
            commands::config::update_system_config,
            commands::config::get_settings,
            commands::config::get_public_config,
            commands::config::import_settings,
            commands::config::export_settings,
            commands::config::save_settings_json,
            commands::config::get_server_config_for_copy,
            // Log commands
            commands::logs::get_logs,
            commands::logs::clear_logs,
            commands::logs::log_event,
            commands::logs::get_activity_available,
            commands::logs::get_activity_filters,
            commands::logs::get_activity_stats,
            commands::logs::get_tool_activities,
            commands::logs::clear_tool_activities,
            commands::logs::cleanup_old_logs,
            commands::logs::cleanup_activity_logs,
            // Bearer key commands
            commands::bearer_keys::list_bearer_keys,
            commands::bearer_keys::create_bearer_key,
            commands::bearer_keys::update_bearer_key,
            commands::bearer_keys::delete_bearer_key,
            // Builtin prompt commands
            commands::prompts::list_builtin_prompts,
            commands::prompts::get_builtin_prompt,
            commands::prompts::create_builtin_prompt,
            commands::prompts::update_builtin_prompt,
            commands::prompts::delete_builtin_prompt,
            commands::prompts::call_builtin_prompt,
            // Builtin resource commands
            commands::resources::list_builtin_resources,
            commands::resources::get_builtin_resource,
            commands::resources::create_builtin_resource,
            commands::resources::update_builtin_resource,
            commands::resources::delete_builtin_resource,
            // Market commands
            commands::market::list_market_servers,
            commands::market::get_market_server,
            commands::market::get_market_categories,
            commands::market::get_market_tags,
            // Registry proxy commands
            commands::registry::list_registry_servers,
            commands::registry::get_registry_server_versions,
            // Cloud/MCPRouter commands
            commands::cloud::list_cloud_servers,
            commands::cloud::get_cloud_server_tools,
            // Per-server tool/prompt/resource config
            commands::server_tool_config::toggle_server_item,
            commands::server_tool_config::update_server_item_description,
            commands::server_tool_config::reset_server_item_description,
            commands::server_tool_config::list_server_item_configs,
            // HTTP server management
            commands::http_server::start_http_server,
            commands::http_server::stop_http_server,
            commands::http_server::get_http_server_status,
            // Runtime version management
            commands::runtime::list_node_versions,
            commands::runtime::list_python_versions,
            commands::runtime::install_node_version,
            commands::runtime::install_python_version,
            commands::runtime::uninstall_node_version,
            commands::runtime::uninstall_python_version,
            commands::runtime::get_active_node_version,
            commands::runtime::get_active_python_version,
            commands::runtime::set_active_node_version,
            commands::runtime::set_active_python_version,
            // Context footprint / cost calculation
            commands::cost::get_server_costs,
            commands::cost::get_group_costs,
            // Skills (技能) — 2.2 agent config; 2.3 scan/list/get/import; 2.4 export; 2.5 uninstall/delete; 2.6 open/pick
            commands::skills::list_skill_agents,
            commands::skills::save_skill_agents,
            commands::skills::create_skill_agent,
            commands::skills::delete_skill_agent,
            commands::skills::scan_skills_for_import,
            commands::skills::list_skills,
            commands::skills::get_skill,
            commands::skills::import_skills,
            commands::skills::scan_folder_for_skills,
            commands::skills::export_skills_to_agents,
            commands::skills::uninstall_skill,
            commands::skills::delete_skill,
            commands::skills::open_path_in_explorer,
            commands::skills::pick_directory,
            commands::skills::open_skill_library_dir,
            // RAG — toggle/status/list/get/pick+upload/delete/search/tags/settings/open-location
            commands::rag::rag_toggle,
            commands::rag::rag_status,
            commands::rag::list_rag_docs,
            commands::rag::get_rag_doc,
            commands::rag::get_rag_chunks,
            commands::rag::pick_rag_files,
            commands::rag::upload_rag_doc,
            commands::rag::update_rag_doc,
            commands::rag::delete_rag_doc,
            commands::rag::rag_search_command,
            commands::rag::rag_tag_search,
            commands::rag::get_rag_settings,
            commands::rag::save_rag_settings,
            commands::rag::rag_model_limits,
            commands::rag::rag_tools,
            commands::rag::set_rag_tags,
            commands::rag::open_rag_file_location,
            commands::rag::rag_reindex_all,
            commands::rag::rag_list_models,
            commands::rag::rag_current_model,
            commands::rag::rag_select_model,
            commands::rag::rag_download_model,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MCPHub application");
}

