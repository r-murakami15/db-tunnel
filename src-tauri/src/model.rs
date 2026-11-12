//! 画面とRustの間でやり取りする型（src/api.ts と対応）。
//! JSONのフィールド名は camelCase。

use serde::{Deserialize, Serialize};

pub const DEFAULT_LOCAL_PORT: u16 = 13306;
pub const MIN_LOCAL_PORT: u16 = 1024;
pub const REGION: &str = "ap-northeast-1";

/// 環境（dev / stg / prod）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    Dev,
    Stg,
    Prod,
}

impl Env {
    pub fn as_str(&self) -> &'static str {
        match self {
            Env::Dev => "dev",
            Env::Stg => "stg",
            Env::Prod => "prod",
        }
    }

    pub fn parse(s: &str) -> Option<Env> {
        match s {
            "dev" => Some(Env::Dev),
            "stg" => Some(Env::Stg),
            "prod" => Some(Env::Prod),
            _ => None,
        }
    }
}

/// 登録済みの接続先
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub id: String,
    pub name: String,
    pub profile: String,
    pub env: Env,
    pub app: String,
    pub local_port: u16,
}

impl Target {
    /// 重複判定のキー（profile + env + app）
    pub fn key(&self) -> (String, Env, String) {
        (self.profile.clone(), self.env, self.app.clone())
    }
}

/// 追加・編集フォームから受け取る値。id が None なら新規
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub profile: String,
    pub env: Env,
    pub app: String,
    #[serde(default)]
    pub local_port: Option<u16>,
}

/// save_target の結果。警告（同じポートの接続先がある等）は保存を妨げない
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub target: Target,
    pub warnings: Vec<String>,
}

/// 接続の状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// 未接続
    Idle,
    /// ログイン待ち（aws login 実行中）
    LoggingIn,
    /// 準備中（検索・シークレット取得・転送開始待ち）
    Preparing,
    /// 接続中
    Connected,
    /// 停止処理中
    Stopping,
    /// エラー
    Error,
}

/// 接続先ごとの実行時の状態。`target-status` イベントの本文でもある
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRuntime {
    pub id: String,
    pub status: Status,
    /// 画面に出す1〜2文のメッセージ（エラーの原因と次の行動など）
    pub message: Option<String>,
    /// 接続中になった時刻（UNIXエポックミリ秒）
    pub connected_at: Option<i64>,
    pub bastion_id: Option<String>,
    pub rds_endpoint: Option<String>,
    pub db_user: Option<String>,
    /// パスワードを保持しているか（値そのものは reveal_password / copy_password でのみ渡す）
    pub has_password: bool,
}

impl TargetRuntime {
    pub fn idle(id: &str) -> Self {
        TargetRuntime {
            id: id.to_string(),
            status: Status::Idle,
            message: None,
            connected_at: None,
            bastion_id: None,
            rds_endpoint: None,
            db_user: None,
            has_password: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogSource {
    App,
    Ssm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// ログ1行。`log-line` イベントの本文でもある
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub target_id: String,
    /// 時刻（HH:MM:SS、ローカル時刻）
    pub time: String,
    pub source: LogSource,
    pub stream: LogStream,
    pub text: String,
}

/// 前提ツール1つ分の確認結果
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prerequisites {
    pub aws_cli: ToolStatus,
    pub session_manager_plugin: ToolStatus,
    /// 両方そろっていれば true（接続ボタンを有効にする）
    pub ok: bool,
}

/// アプリ設定（暗号化した設定ファイルに接続先と一緒に保存する）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub last_selected_id: Option<String>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    /// aws コマンドのパス（未指定なら自動検出）
    pub aws_cli_path: Option<String>,
    /// session-manager-plugin のパス（未指定なら自動検出）
    pub plugin_path: Option<String>,
}

/// 設定ファイルの情報（設定画面の「設定ファイル」カード用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
    pub path: String,
    pub encryption: String,
    /// 鍵をキーチェーンから読めたか
    pub keychain_ok: bool,
    /// 起動時に設定を読めなかった場合のメッセージ（退避したファイル名を含む）
    pub load_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportJudgement {
    New,
    Overwrite,
    Error,
}

/// インポートのプレビュー1行
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRow {
    /// ファイル内の targets 配列の添字（0始まり）
    pub index: usize,
    pub name: String,
    pub profile: String,
    pub env: String,
    pub app: String,
    pub local_port: Option<i64>,
    pub judgement: ImportJudgement,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub file_name: String,
    pub rows: Vec<ImportRow>,
    /// ファイル全体に対する警告（未知のフィールドなど）
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportMode {
    /// 同じ組み合わせは上書き
    Overwrite,
    /// 新規のみ追加
    NewOnly,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
}
