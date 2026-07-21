pub mod acp;
pub mod biometric;
pub mod broker;
pub mod commands;
pub mod db;
pub mod error;
pub mod format;
pub mod fsio;
pub mod logging;
pub mod store;
pub mod tunnel;
pub mod vault;

use commands::AppState;
use tauri::Manager;

/// Custom macOS menu: the default one binds ⌘W to Close Window, which kills
/// the whole (single-window) app. Rebind it to Close Tab and add ⌘⇧T to
/// reopen; the frontend picks both up as `menu://…` events.
#[cfg(target_os = "macos")]
fn set_app_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{AboutMetadata, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    use tauri::Emitter;

    let handle = app.handle();
    let check_updates =
        MenuItemBuilder::with_id("check-updates", "Check for Updates…").build(handle)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings…")
        .accelerator("CmdOrCtrl+,")
        .build(handle)?;
    let log_viewer =
        MenuItemBuilder::with_id("log-viewer", "Diagnostics Log…").build(handle)?;
    let install_cli =
        MenuItemBuilder::with_id("install-cli", "Install CLI…").build(handle)?;
    let app_menu = SubmenuBuilder::new(handle, "sql-kai")
        .about(Some(AboutMetadata::default()))
        .item(&check_updates)
        .separator()
        .item(&settings)
        .item(&log_viewer)
        .item(&install_cli)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let new_query_tab = MenuItemBuilder::with_id("new-query-tab", "New Query Tab")
        .accelerator("CmdOrCtrl+N")
        .build(handle)?;
    let close_tab = MenuItemBuilder::with_id("close-tab", "Close Tab")
        .accelerator("CmdOrCtrl+W")
        .build(handle)?;
    let reopen_tab = MenuItemBuilder::with_id("reopen-tab", "Reopen Closed Tab")
        .accelerator("CmdOrCtrl+Shift+T")
        .build(handle)?;
    let file_menu = SubmenuBuilder::new(handle, "File")
        .item(&new_query_tab)
        .separator()
        .item(&close_tab)
        .item(&reopen_tab)
        .build()?;
    let edit_menu = SubmenuBuilder::new(handle, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let window_menu = SubmenuBuilder::new(handle, "Window")
        .minimize()
        .separator()
        .fullscreen()
        .build()?;
    let menu = MenuBuilder::new(handle)
        .items(&[&app_menu, &file_menu, &edit_menu, &window_menu])
        .build()?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        let id = event.id().as_ref();
        if matches!(
            id,
            "new-query-tab"
                | "close-tab"
                | "reopen-tab"
                | "settings"
                | "check-updates"
                | "log-viewer"
                | "install-cli"
        ) {
            let _ = app.emit(&format!("menu://{id}"), ());
        }
    });
    Ok(())
}

/// Показывает главное окно (после скрытия по close / клика в трей).
/// Возвращает Regular-политику: без неё окно не получает фокус,
/// а приложения нет в Dock и Cmd-Tab.
#[cfg(target_os = "macos")]
fn show_main_window(app: &tauri::AppHandle) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Показывает главное окно на старте (первый показ скрытого окна). На macOS
/// повторно использует show_main_window (Regular-политика + фокус), на других
/// платформах — простой show + focus.
fn reveal_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    show_main_window(app);
    #[cfg(not(target_os = "macos"))]
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Фронтенд вызывает это после первого кадра отрисовки. Окно создаётся
/// скрытым (visible: false), геометрию восстанавливает window-state-плагин —
/// показ уже готового содержимого убирает «растягивание» окна из дефолтной
/// геометрии на запуске и при перезапуске после автообновления.
#[tauri::command]
fn reveal_main_window(app: tauri::AppHandle) {
    reveal_window(&app);
}

/// Иконка в menu bar: приложение живёт в трее, даже когда окно закрыто
/// (close прячет окно, см. on_window_event ниже).
#[cfg(target_os = "macos")]
fn build_tray_menu(
    app: &tauri::AppHandle,
    connections: &[TrayConnection],
    active_profile_id: Option<&str>,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};

    let open = MenuItemBuilder::with_id("tray-open", "Open sql-kai").build(app)?;
    let quit = MenuItemBuilder::with_id("tray-quit", "Quit sql-kai").build(app)?;
    let mut menu = MenuBuilder::new(app).item(&open);
    if !connections.is_empty() {
        menu = menu.separator();
        for connection in connections {
            // `&` is a native-menu mnemonic marker; double it to preserve
            // profile names verbatim.
            let name = connection.name.replace('&', "&&");
            // A normal item with a stable mark is used instead of a native
            // checkbox: checkbox items toggle themselves off when the already
            // active connection is clicked.
            let label = if active_profile_id == Some(connection.profile_id.as_str()) {
                format!("✓ {name}")
            } else {
                format!("  {name}")
            };
            let item = MenuItemBuilder::with_id(
                format!("tray-connection:{}", connection.profile_id),
                label,
            )
            .build(app)?;
            menu = menu.item(&item);
        }
    }
    menu.separator().item(&quit).build()
}

#[cfg(target_os = "macos")]
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::tray::TrayIconBuilder;
    use tauri::Emitter;

    let handle = app.handle();
    let menu = build_tray_menu(handle, &[], None)?;
    // Template-иконка (чёрная + альфа): macOS сам перекрашивает её под
    // светлую/тёмную строку меню.
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("sql-kai")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                "tray-open" => show_main_window(app),
                "tray-quit" => app.exit(0),
                _ => {
                    if let Some(profile_id) = id.strip_prefix("tray-connection:") {
                        show_main_window(app);
                        let _ = app.emit("tray://select-connection", profile_id.to_owned());
                    }
                }
            }
        })
        .build(app)?;
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrayConnection {
    profile_id: String,
    name: String,
}

/// Keeps the native menu-bar connection switcher in lockstep with the
/// frontend store. The command is a no-op on platforms without this tray.
#[tauri::command]
fn sync_tray_connections(
    app: tauri::AppHandle,
    connections: Vec<TrayConnection>,
    active_profile_id: Option<String>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let menu = build_tray_menu(&app, &connections, active_profile_id.as_deref())
            .map_err(|e| e.to_string())?;
        let tray = app
            .tray_by_id("main-tray")
            .ok_or_else(|| "main tray is not available".to_string())?;
        tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
        let tooltip = match connections.len() {
            0 => "sql-kai".to_string(),
            1 => "sql-kai — 1 active connection".to_string(),
            count => format!("sql-kai — {count} active connections"),
        };
        tray.set_tooltip(Some(tooltip)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, connections, active_profile_id);
    Ok(())
}

/// Поднимает брокер-сокет для sql-kai: GUI-процесс выполняет его запросы своими
/// сессиями/vault'ом. Ошибка бинда не мешает приложению — sql-kai просто пойдёт
/// автономным путём.
#[cfg(unix)]
fn start_broker(app: &tauri::App, state: &std::sync::Arc<broker::BrokerState>) {
    use tauri::Emitter;

    // tokio's UnixListener::bind needs a runtime context, but setup() runs
    // outside it (on the ObjC did_finish_launching callback) — bind inside the
    // Tauri async runtime so it doesn't panic-and-abort at launch.
    let listener = match tauri::async_runtime::block_on(async { broker::bind() }) {
        Ok(l) => l,
        Err(e) => {
            logging::log("broker", &format!("socket bind failed: {e}"));
            return;
        }
    };
    let gui = app.handle().clone();
    let notify = app.handle().clone();
    let notify_profiles = app.handle().clone();
    let notify_open = app.handle().clone();
    let ask_gui = app.handle().clone();
    let hooks = std::sync::Arc::new(broker::BrokerHooks {
        gui_sessions: Box::new(move || {
            let state = gui.state::<AppState>();
            let sessions = state.sessions.lock().unwrap();
            sessions
                .values()
                .filter(|s| !s.isolated)
                .map(|s| broker::BrokerSessionInfo::from_session(s, "gui", None))
                .collect()
        }),
        changed: Box::new(move || {
            let _ = notify.emit("broker://changed", ());
        }),
        profiles_changed: Box::new(move || {
            let _ = notify_profiles.emit("profiles://changed", ());
        }),
        shutdown: None, // GUI-брокер по сокету не гасится
        open_in_gui: Some(Box::new(move |open| {
            let _ = notify_open.emit("agent://open", &open);
        })),
        gui_selection: Some(Box::new(move |profile_id| {
            let app = ask_gui.clone();
            Box::pin(async move {
                // event в webview с id запроса; фронт собирает состояние из
                // стора и отвечает командой agent_gui_reply — oneshot
                let (tx, rx) = tokio::sync::oneshot::channel();
                let id = uuid::Uuid::new_v4().to_string();
                app.state::<AppState>()
                    .gui_requests
                    .lock()
                    .unwrap()
                    .insert(id.clone(), tx);
                let _ = app.emit(
                    "agent://gui-request",
                    serde_json::json!({ "id": id, "kind": "selection", "profileId": profile_id }),
                );
                let res = match tokio::time::timeout(std::time::Duration::from_secs(4), rx).await
                {
                    Ok(Ok(v)) => Ok(v),
                    Ok(Err(_)) => Err("GUI отменил запрос".to_string()),
                    Err(_) => Err("GUI не ответил на запрос состояния".to_string()),
                };
                app.state::<AppState>()
                    .gui_requests
                    .lock()
                    .unwrap()
                    .remove(&id);
                res
            })
        })),
    });
    tauri::async_runtime::spawn(broker::serve(listener, state.clone(), hooks));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let broker_state = std::sync::Arc::new(broker::BrokerState::default());
    tauri::Builder::default()
        // Геометрия окна между запусками. Окно в конфиге создаётся скрытым,
        // плагин восстанавливает размер/позицию до show() в setup — без
        // прыжка из дефолтной геометрии. VISIBLE не сохраняем: close прячет
        // окно в трей, и quit со спрятанным окном записал бы «невидимость»
        // в стейт следующего запуска. FULLSCREEN/DECORATIONS тоже вне флагов
        // (глючное восстановление нативного fullscreen на macOS).
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::default())
        .manage(broker_state.clone())
        .manage(acp::AcpState::default())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            set_app_menu(app)?;
            #[cfg(target_os = "macos")]
            setup_tray(app)?;
            #[cfg(unix)]
            start_broker(app, &broker_state);
            // Пинги, не дающие простаивающим GUI-сессиям пасть от внешних
            // idle-таймаутов — «вернулся через полчаса, а соединение живое».
            commands::spawn_keepalive(app.handle());
            // Окно создано скрытым (visible: false в tauri.conf.json), а
            // window-state уже восстановил его геометрию. Показ отдан фронтенду
            // (команда reveal_main_window, вызывается после первого кадра
            // отрисовки) — так окно появляется уже с готовым содержимым и
            // восстановленной геометрией, без «растягивания» из дефолта. Тот же
            // путь проходит холодный старт и перезапуск после автообновления.
            // Страховка: если фронтенд не показал окно (ошибка JS, зависание
            // загрузки) — показываем сами по таймауту, чтобы окно не осталось
            // навсегда невидимым.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if let Some(win) = handle.get_webview_window("main") {
                    if !win.is_visible().unwrap_or(true) {
                        reveal_window(&handle);
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Close (red button / Close Window) прячет окно — приложение
            // остаётся жить в трее, сессии и туннели не рвутся. Accessory
            // убирает иконку из Dock и Cmd-Tab, пока окно скрыто.
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                let _ = window
                    .app_handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory);
                api.prevent_close();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
        })
        .invoke_handler(tauri::generate_handler![
            commands::vault_status,
            commands::vault_setup,
            commands::vault_unlock,
            commands::vault_unlock_biometric,
            commands::vault_enable_biometric,
            commands::vault_disable_biometric,
            commands::vault_lock,
            commands::list_profiles,
            commands::save_profile,
            commands::duplicate_profile,
            commands::delete_profile,
            commands::list_queries,
            commands::save_query,
            commands::delete_query,
            commands::get_settings,
            commands::save_settings,
            commands::settings_path,
            commands::log_path,
            commands::log_event,
            commands::read_log,
            commands::list_history,
            commands::record_history,
            commands::delete_history_entry,
            commands::clear_history,
            commands::import_history,
            commands::connect_profile,
            commands::open_isolated_session,
            commands::disconnect_session,
            commands::list_sessions,
            commands::test_profile,
            commands::execute_sql,
            commands::export_sql,
            commands::session_tx_status,
            commands::cancel_query,
            commands::list_tables,
            commands::list_columns,
            commands::list_all_columns,
            commands::get_table_ddl,
            commands::list_indexes,
            commands::list_relations,
            commands::list_triggers,
            commands::list_enums,
            commands::list_policies,
            commands::get_table_page,
            commands::save_text_file,
            commands::save_rows_xlsx,
            commands::copy_text_concealed,
            commands::list_cli_sessions,
            commands::install_cli,
            commands::relaunch_app,
            commands::cli_bin_path,
            commands::agent_gui_reply,
            sync_tray_connections,
            reveal_main_window,
            acp::acp_spawn,
            acp::acp_send,
            acp::acp_kill,
            acp::acp_installed_version,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            match event {
                // Клик по иконке в Dock, когда окно спрятано в трей.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => show_main_window(app_handle),
                // Make sure ssh tunnel children (incl. the broker's) die with the app.
                tauri::RunEvent::Exit => {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        state.sessions.lock().unwrap().clear();
                    }
                    // ACP-агенты — дочерние процессы, не должны пережить приложение
                    if let Some(acp) = app_handle.try_state::<acp::AcpState>() {
                        acp.clear();
                    }
                    if let Some(broker) =
                        app_handle.try_state::<std::sync::Arc<broker::BrokerState>>()
                    {
                        broker.clear();
                    }
                    if let Ok(path) = broker::socket_path() {
                        let _ = std::fs::remove_file(path);
                    }
                }
                _ => {}
            }
        });
}
