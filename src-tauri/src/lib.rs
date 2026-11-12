pub mod aws;
pub mod commands;
pub mod connection;
pub mod import_export;
pub mod logbuf;
pub mod model;
pub mod process;
pub mod store;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::commands::{AppState, TauriSink};
use crate::connection::ConnectionManager;
use crate::store::{KeychainKeyProvider, Store};

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let store = Store::open(Store::default_dir(), Box::new(KeychainKeyProvider));
            let paths = commands::tool_paths(store.settings());
            let (width, height) = (
                store.settings().window_width,
                store.settings().window_height,
            );
            let conn = ConnectionManager::new(Arc::new(TauriSink(app.handle().clone())), paths);
            app.manage(Arc::new(AppState {
                store: Mutex::new(store),
                conn,
            }));

            // 前回のウィンドウサイズを復元する（最小サイズ未満は無視）
            if let (Some(w), Some(h), Some(win)) = (width, height, app.get_webview_window("main")) {
                if w >= 960 && h >= 640 {
                    let _ = win.set_size(tauri::LogicalSize::new(w, h));
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<Arc<AppState>>();
                save_window_size(window, &state);
                let active = state.conn.active_count();
                if active > 0 {
                    // 接続中があれば画面で確認してから quit_app で終了する
                    api.prevent_close();
                    let _ = window.emit("quit-requested", active);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_targets,
            commands::save_target,
            commands::delete_target,
            commands::list_profiles,
            commands::list_runtimes,
            commands::connect,
            commands::disconnect,
            commands::reveal_password,
            commands::copy_password,
            commands::copy_text,
            commands::get_logs,
            commands::clear_logs,
            commands::save_logs,
            commands::check_prerequisites,
            commands::set_tool_paths,
            commands::get_settings,
            commands::set_last_selected,
            commands::get_config_info,
            commands::open_config_dir,
            commands::import_preview,
            commands::import_apply,
            commands::export_targets,
            commands::quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // どの経路で終了しても子プロセス（session-manager-plugin まで）を残さない
            if let Some(state) = handle.try_state::<Arc<AppState>>() {
                let conn = state.conn.clone();
                tauri::async_runtime::block_on(conn.disconnect_all());
            }
        }
    });
}

fn save_window_size(window: &tauri::Window, state: &AppState) {
    let Ok(size) = window.inner_size() else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let logical = size.to_logical::<u32>(scale);
    let mut store = state.store.lock().unwrap_or_else(|e| e.into_inner());
    let s = store.settings();
    if s.window_width == Some(logical.width) && s.window_height == Some(logical.height) {
        return;
    }
    let _ = store.update_settings(|s| {
        s.window_width = Some(logical.width);
        s.window_height = Some(logical.height);
    });
}
