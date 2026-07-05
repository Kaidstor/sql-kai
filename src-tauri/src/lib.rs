pub mod biometric;
pub mod commands;
pub mod db;
pub mod error;
pub mod fsio;
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
    let app_menu = SubmenuBuilder::new(handle, "sql-kai")
        .about(Some(AboutMetadata::default()))
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
        if matches!(id, "new-query-tab" | "close-tab" | "reopen-tab") {
            let _ = app.emit(&format!("menu://{id}"), ());
        }
    });
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            set_app_menu(app)?;
            #[cfg(not(target_os = "macos"))]
            let _ = app;
            Ok(())
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
            commands::connect_profile,
            commands::disconnect_session,
            commands::list_sessions,
            commands::test_profile,
            commands::execute_sql,
            commands::cancel_query,
            commands::get_tables,
            commands::get_columns,
            commands::get_all_columns,
            commands::get_table_ddl,
            commands::get_indexes,
            commands::get_relations,
            commands::get_triggers,
            commands::get_table_page,
            commands::copy_text_concealed,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Make sure ssh tunnel children die with the app.
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    state.sessions.lock().unwrap().clear();
                }
            }
        });
}
