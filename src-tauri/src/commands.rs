//! 画面から呼ばれる Tauri コマンド（src/api.ts と対応）。
//! エラーは画面にそのまま出せる日本語の文字列で返す。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tauri::{AppHandle, Emitter, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
use zeroize::Zeroizing;

use crate::aws::{self, ToolPaths};
use crate::connection::{ConnectionManager, EventSink};
use crate::model::*;
use crate::store::Store;

/// パスワードをコピーしてからクリップボードを消すまでの時間
const CLIPBOARD_CLEAR_AFTER: Duration = Duration::from_secs(30);

pub struct AppState {
    pub store: Mutex<Store>,
    pub conn: ConnectionManager,
}

impl AppState {
    fn store(&self) -> MutexGuard<'_, Store> {
        // 毒化しても中身は使える（設定の読み書きだけなので続行する）
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn target(&self, id: &str) -> Result<Target, String> {
        self.store()
            .target(id)
            .cloned()
            .ok_or_else(|| "接続先が見つかりません".to_string())
    }
}

/// 設定に保存されたツールのパスを ToolPaths にする
pub fn tool_paths(settings: &AppSettings) -> ToolPaths {
    fn non_empty(s: &Option<String>) -> Option<PathBuf> {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }
    ToolPaths {
        aws: non_empty(&settings.aws_cli_path),
        plugin: non_empty(&settings.plugin_path),
    }
}

/// 接続管理からのイベントを画面へ送る
pub struct TauriSink(pub AppHandle);

impl EventSink for TauriSink {
    fn status(&self, rt: &TargetRuntime) {
        let _ = self.0.emit("target-status", rt);
    }

    fn log(&self, line: &LogLine) {
        let _ = self.0.emit("log-line", line);
    }
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---- 接続先の管理 ----

#[tauri::command]
pub fn list_targets(state: State<'_, Arc<AppState>>) -> Vec<Target> {
    state.store().targets().to_vec()
}

#[tauri::command]
pub fn save_target(state: State<'_, Arc<AppState>>, input: TargetInput) -> CmdResult<SaveResult> {
    if let Some(id) = input.id.as_deref() {
        if state.conn.is_active(id) {
            let current = state.target(id)?;
            let new_port = input.local_port.unwrap_or(DEFAULT_LOCAL_PORT);
            if current.env != input.env || current.local_port != new_port {
                return Err(
                    "接続中は環境とポートを変更できません。停止してから変更してください".into(),
                );
            }
        }
    }
    state.store().save_target(input).map_err(err)
}

#[tauri::command]
pub fn delete_target(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    if state.conn.is_active(&id) {
        return Err("接続中の接続先は削除できません。停止してから削除してください".into());
    }
    state.store().delete_target(&id).map_err(err)?;
    state.conn.forget(&id);
    Ok(())
}

#[tauri::command]
pub fn move_target(
    state: State<'_, Arc<AppState>>,
    id: String,
    direction: MoveDirection,
) -> CmdResult<Vec<Target>> {
    let mut store = state.store();
    store.move_target(&id, direction).map_err(err)?;
    Ok(store.targets().to_vec())
}

#[tauri::command]
pub fn duplicate_target(
    state: State<'_, Arc<AppState>>,
    source_id: String,
    input: TargetInput,
) -> CmdResult<SaveResult> {
    state
        .store()
        .duplicate_target(&source_id, input)
        .map_err(err)
}

#[tauri::command]
pub fn list_profiles() -> Vec<String> {
    aws::list_profiles()
}

// ---- 接続 ----

#[tauri::command]
pub fn list_runtimes(state: State<'_, Arc<AppState>>) -> Vec<TargetRuntime> {
    state.conn.runtimes()
}

#[tauri::command]
pub async fn connect(
    state: State<'_, Arc<AppState>>,
    id: String,
    confirmed_prod: bool,
) -> CmdResult<()> {
    let target = state.target(&id)?;
    if target.env == Env::Prod && !confirmed_prod {
        return Err("本番環境（prod）への接続は確認が必要です".into());
    }
    state.conn.connect(target).await
}

#[tauri::command]
pub async fn disconnect(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    state.conn.disconnect(&id).await;
    Ok(())
}

#[tauri::command]
pub fn reveal_password(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<String> {
    state
        .conn
        .password(&id)
        .map(|p| p.to_string())
        .ok_or_else(|| "パスワードがありません（接続すると取得します）".into())
}

#[tauri::command]
pub fn copy_password(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    let password = state
        .conn
        .password(&id)
        .ok_or_else(|| "パスワードがありません（接続すると取得します）".to_string())?;
    app.clipboard()
        .write_text(password.to_string())
        .map_err(|e| format!("クリップボードに書き込めませんでした: {e}"))?;

    // 30秒後、クリップボードの内容がまだ同じなら消す
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLIPBOARD_CLEAR_AFTER).await;
        let current = app2.clipboard().read_text().ok().map(Zeroizing::new);
        if current.as_deref().map(|s| s.as_str()) == Some(password.as_str()) {
            let _ = app2.clipboard().write_text(String::new());
        }
    });
    Ok(())
}

#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> CmdResult<()> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("クリップボードに書き込めませんでした: {e}"))
}

// ---- ログ ----

#[tauri::command]
pub fn get_logs(state: State<'_, Arc<AppState>>, id: String) -> Vec<LogLine> {
    state.conn.logs(&id)
}

#[tauri::command]
pub fn clear_logs(state: State<'_, Arc<AppState>>, id: String) {
    state.conn.clear_logs(&id);
}

pub fn format_logs(lines: &[LogLine]) -> String {
    let mut out = String::new();
    for l in lines {
        let src = match l.source {
            LogSource::App => "[app]",
            LogSource::Ssm => "[ssm]",
        };
        let stream = match l.stream {
            LogStream::Stdout => "",
            LogStream::Stderr => " (stderr)",
        };
        out.push_str(&format!("{} {}{} {}\n", l.time, src, stream, l.text));
    }
    out
}

#[tauri::command]
pub fn save_logs(state: State<'_, Arc<AppState>>, id: String, path: String) -> CmdResult<()> {
    let text = format_logs(&state.conn.logs(&id));
    std::fs::write(&path, text).map_err(|e| format!("ログを保存できませんでした: {e}"))
}

// ---- 前提ツール・設定 ----

#[tauri::command]
pub async fn check_prerequisites(state: State<'_, Arc<AppState>>) -> CmdResult<Prerequisites> {
    let paths = tool_paths(state.store().settings());
    Ok(aws::check_prerequisites(&paths).await)
}

#[tauri::command]
pub async fn set_tool_paths(
    state: State<'_, Arc<AppState>>,
    aws_cli_path: Option<String>,
    plugin_path: Option<String>,
) -> CmdResult<Prerequisites> {
    let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let paths = {
        let mut store = state.store();
        store
            .update_settings(|s| {
                s.aws_cli_path = clean(aws_cli_path);
                s.plugin_path = clean(plugin_path);
            })
            .map_err(err)?;
        tool_paths(store.settings())
    };
    state.conn.set_tool_paths(paths.clone());
    Ok(aws::check_prerequisites(&paths).await)
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> AppSettings {
    state.store().settings().clone()
}

#[tauri::command]
pub fn set_last_selected(state: State<'_, Arc<AppState>>, id: Option<String>) -> CmdResult<()> {
    let mut store = state.store();
    if store.settings().last_selected_id == id {
        return Ok(());
    }
    store
        .update_settings(|s| s.last_selected_id = id)
        .map_err(err)
}

#[tauri::command]
pub fn get_config_info(state: State<'_, Arc<AppState>>) -> ConfigInfo {
    state.store().info()
}

#[tauri::command]
pub fn open_config_dir(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let path = state.store().config_path();
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    std::fs::create_dir_all(&dir).map_err(err)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("フォルダを開けませんでした: {e}"))
}

// ---- インポート・エクスポート ----

#[tauri::command]
pub fn import_preview(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<ImportPreview> {
    state.store().import_preview(Path::new(&path)).map_err(err)
}

#[tauri::command]
pub fn import_apply(
    state: State<'_, Arc<AppState>>,
    mode: ImportMode,
    indices: Vec<usize>,
) -> CmdResult<ImportResult> {
    state.store().import_apply(mode, &indices).map_err(err)
}

#[tauri::command]
pub fn export_targets(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<usize> {
    state.store().export(Path::new(&path)).map_err(err)
}

// ---- 終了 ----

#[tauri::command]
pub async fn quit_app(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.conn.disconnect_all().await;
    app.exit(0);
    Ok(())
}
