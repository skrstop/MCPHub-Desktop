pub mod auth;
pub mod commands;
pub mod db;
pub mod mcp;
pub mod models;
pub mod services;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // On macOS dev mode, set the process display name so the Dock shows "MCPHub Desktop"
    #[cfg(target_os = "macos")]
    unsafe {
        use objc::{class, msg_send};
        use objc::runtime::Object;
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let ns_str: *mut Object = msg_send![class!(NSString),
            stringWithUTF8String: b"MCPHub Desktop\0".as_ptr() as *const std::ffi::c_char];
        let _: () = msg_send![info, setProcessName: ns_str];
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Register session state
            app.manage(commands::auth::SessionState(tokio::sync::Mutex::new(None)));

            // Initialize the database: spawn async task, block current thread via channel
            let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let result = db::initialize(&app_handle).await
                    .map_err(|e| format!("{:#}", e));
                tx.send(result).ok();
            });
            if let Err(e) = rx.recv().unwrap() {
                log::error!("Failed to initialize database: {}", e);
                std::process::exit(1);
            }

            // Start MCP servers and HTTP server in background after DB is ready
            let app_handle2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = services::mcp_manager::start_all(&app_handle2).await {
                    log::error!("Failed to start MCP servers: {}", e);
                }
                services::http_server::maybe_start().await;
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
            commands::config::import_settings,
            commands::config::export_settings,
            // Log commands
            commands::logs::get_logs,
            commands::logs::clear_logs,
            commands::logs::get_activity_available,
            commands::logs::get_activity_filters,
            commands::logs::get_activity_stats,
            commands::logs::get_tool_activities,
            commands::logs::clear_tool_activities,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running MCPHub application");
}

