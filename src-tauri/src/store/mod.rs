//! 接続先とアプリ設定の保存（暗号化した config.enc）。
//!
//! - 鍵はOSのキーチェーンに置き（[`KeychainKeyProvider`]）、ファイルは AES-256-GCM で暗号化する。
//! - 保存は一時ファイル → fsync → 直前の版を `config.enc.bak` に複製 → リネーム の順で行う。
//! - 鍵がない・復号できない場合はファイルを `config.enc.broken-<日時>` に退避し、空で起動する。

pub mod crypto;
pub mod keychain;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::import_export;
use crate::model::{
    AppSettings, ConfigInfo, ImportMode, ImportPreview, ImportResult, MoveDirection, SaveResult,
    Target, TargetInput, DEFAULT_LOCAL_PORT,
};

pub use keychain::{KeyProvider, KeychainKeyProvider, MemoryKeyProvider};

pub const CONFIG_FILE: &str = "config.enc";
pub const TMP_FILE: &str = "config.enc.tmp";
pub const BAK_FILE: &str = "config.enc.bak";
pub const ENCRYPTION_NAME: &str = "AES-256-GCM";

#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    #[error("キーチェーンにアクセスできません（{0}）")]
    Keychain(String),
    #[error("暗号化の鍵を読み込めなかったため、設定を保存できません。キーチェーンへのアクセスを許可してアプリを再起動してください")]
    NoKey,
    #[error("{action}に失敗しました（{path}）: {source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("設定の暗号化に失敗しました（{0}）")]
    Crypto(String),
    #[error("設定の変換に失敗しました（{0}）")]
    Serialize(String),
    #[error("入力内容に誤りがあります：{}", .0.join("、"))]
    Validation(Vec<String>),
    #[error("同じプロファイル・環境・アプリ名の接続先がすでにあります（「{0}」）")]
    Duplicate(String),
    #[error("接続先が見つかりません（ID: {0}）")]
    NotFound(String),
    #[error("インポートできません：{0}")]
    Import(String),
    #[error("インポートのプレビューがありません。もう一度ファイルを選んでください")]
    NoPendingImport,
}

impl StoreError {
    fn io(action: &'static str, path: &Path, source: std::io::Error) -> Self {
        StoreError::Io {
            action,
            path: path.display().to_string(),
            source,
        }
    }
}

/// config.enc の中身（暗号化前の平文JSON）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigData {
    pub targets: Vec<Target>,
    pub settings: AppSettings,
}

pub struct Store {
    dir: PathBuf,
    /// 鍵（キーチェーンから読めなかった場合は None で、保存はエラーになる）
    key: Option<Zeroizing<[u8; 32]>>,
    keychain_ok: bool,
    load_error: Option<String>,
    data: ConfigData,
    /// 直前の import_preview の結果（rows と同じ並び、取り込める行だけ Some）
    pending_import: Option<Vec<Option<TargetInput>>>,
}

impl Store {
    pub fn default_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("db-tunnel")
    }

    /// 設定を読み込む。失敗しても Store は必ず返す（状況は info() の load_error / keychain_ok で分かる）。
    pub fn open(dir: PathBuf, keys: Box<dyn KeyProvider>) -> Store {
        let mut store = Store {
            dir,
            key: None,
            keychain_ok: true,
            load_error: None,
            data: ConfigData::default(),
            pending_import: None,
        };
        let path = store.config_path();
        let exists = path.exists();

        match keys.load_key() {
            Err(e) => {
                // キーチェーン自体が使えない。ファイルは退避せずそのまま残す（一時的な拒否の可能性があるため）
                store.keychain_ok = false;
                store.load_error = Some(format!(
                    "{e}。設定を読み込めないため、空の状態で起動しました（変更は保存されません）"
                ));
            }
            Ok(Some(key)) => {
                if exists {
                    match read_config(&path, &key) {
                        Ok(data) => store.data = data,
                        Err(reason) => store.quarantine(&reason),
                    }
                }
                store.key = Some(key);
            }
            Ok(None) => {
                if exists {
                    store.quarantine("暗号化の鍵がキーチェーンに見つかりません");
                }
                let key = crypto::generate_key();
                match keys.store_key(&key) {
                    Ok(()) => store.key = Some(key),
                    Err(e) => {
                        store.keychain_ok = false;
                        let msg = format!("{e}。新しい鍵を保存できないため、変更は保存されません");
                        store.load_error = Some(match store.load_error.take() {
                            Some(prev) => format!("{prev}\n{msg}"),
                            None => msg,
                        });
                    }
                }
            }
        }
        store
    }

    /// 読めないファイルを config.enc.broken-<日時> に退避して load_error を設定する
    fn quarantine(&mut self, reason: &str) {
        let path = self.config_path();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let mut name = format!("{CONFIG_FILE}.broken-{stamp}");
        let mut n = 1;
        while self.dir.join(&name).exists() {
            name = format!("{CONFIG_FILE}.broken-{stamp}-{n}");
            n += 1;
        }
        let dest = self.dir.join(&name);
        self.load_error = Some(match fs::rename(&path, &dest) {
            Ok(()) => format!(
                "設定を読み込めませんでした（{reason}）。JSONを再インポートしてください（退避先: {name}）"
            ),
            Err(e) => format!(
                "設定を読み込めませんでした（{reason}）。ファイルの退避にも失敗しました（{e}）。JSONを再インポートしてください"
            ),
        });
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE)
    }

    pub fn info(&self) -> ConfigInfo {
        ConfigInfo {
            path: self.config_path().display().to_string(),
            encryption: ENCRYPTION_NAME.to_string(),
            keychain_ok: self.keychain_ok,
            load_error: self.load_error.clone(),
        }
    }

    pub fn targets(&self) -> &[Target] {
        &self.data.targets
    }

    pub fn target(&self, id: &str) -> Option<&Target> {
        self.data.targets.iter().find(|t| t.id == id)
    }

    pub fn settings(&self) -> &AppSettings {
        &self.data.settings
    }

    /// 接続先を追加・更新して保存する（新規は末尾に追加）
    pub fn save_target(&mut self, input: TargetInput) -> Result<SaveResult, StoreError> {
        self.save_inner(input, None)
    }

    /// 接続先を複製する。input を新しい接続先として、複製元のすぐ下に追加する
    pub fn duplicate_target(
        &mut self,
        source_id: &str,
        mut input: TargetInput,
    ) -> Result<SaveResult, StoreError> {
        if self.target(source_id).is_none() {
            return Err(StoreError::NotFound(source_id.to_string()));
        }
        input.id = None;
        self.save_inner(input, Some(source_id))
    }

    /// 一覧の並び順で1つ上・下へ移動する。端の場合は何もしない
    pub fn move_target(&mut self, id: &str, direction: MoveDirection) -> Result<(), StoreError> {
        let Some(index) = self.data.targets.iter().position(|t| t.id == id) else {
            return Err(StoreError::NotFound(id.to_string()));
        };
        let other = match direction {
            MoveDirection::Up if index > 0 => index - 1,
            MoveDirection::Down if index + 1 < self.data.targets.len() => index + 1,
            _ => return Ok(()),
        };
        let mut data = self.data.clone();
        data.targets.swap(index, other);
        self.commit(data)
    }

    /// 追加・更新の本体。insert_after が指定されていれば新規はその直後に入れる
    fn save_inner(
        &mut self,
        input: TargetInput,
        insert_after: Option<&str>,
    ) -> Result<SaveResult, StoreError> {
        let errors = import_export::validate_fields(
            &input.name,
            &input.profile,
            &input.app,
            input.local_port.map(i64::from),
        );
        if !errors.is_empty() {
            return Err(StoreError::Validation(errors));
        }

        let id = match &input.id {
            Some(id) => {
                if self.target(id).is_none() {
                    return Err(StoreError::NotFound(id.clone()));
                }
                id.clone()
            }
            None => uuid::Uuid::new_v4().to_string(),
        };
        let target = Target {
            id: id.clone(),
            name: input.name.trim().to_string(),
            profile: input.profile,
            env: input.env,
            app: input.app,
            local_port: input.local_port.unwrap_or(DEFAULT_LOCAL_PORT),
        };

        let others = || self.data.targets.iter().filter(|t| t.id != id);
        if let Some(dup) = others().find(|t| t.key() == target.key()) {
            return Err(StoreError::Duplicate(dup.name.clone()));
        }
        let warnings: Vec<String> = others()
            .filter(|t| t.local_port == target.local_port)
            .map(|t| {
                format!(
                    "ポート{}は「{}」でも使われています。同時に接続する場合はポートを変えてください",
                    target.local_port, t.name
                )
            })
            .collect();

        let mut data = self.data.clone();
        match data.targets.iter_mut().find(|t| t.id == id) {
            Some(t) => *t = target.clone(),
            None => {
                let pos = insert_after
                    .and_then(|after| data.targets.iter().position(|t| t.id == after))
                    .map(|i| i + 1)
                    .unwrap_or(data.targets.len());
                data.targets.insert(pos, target.clone());
            }
        }
        self.commit(data)?;
        Ok(SaveResult { target, warnings })
    }

    pub fn delete_target(&mut self, id: &str) -> Result<(), StoreError> {
        if self.target(id).is_none() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let mut data = self.data.clone();
        data.targets.retain(|t| t.id != id);
        if data.settings.last_selected_id.as_deref() == Some(id) {
            data.settings.last_selected_id = None;
        }
        self.commit(data)
    }

    pub fn update_settings(&mut self, f: impl FnOnce(&mut AppSettings)) -> Result<(), StoreError> {
        let mut data = self.data.clone();
        f(&mut data.settings);
        self.commit(data)
    }

    /// インポートするファイルを読み、プレビューを返す（結果は import_apply 用に覚えておく）
    pub fn import_preview(&mut self, path: &Path) -> Result<ImportPreview, StoreError> {
        self.pending_import = None;
        let meta = fs::metadata(path).map_err(|e| StoreError::io("ファイルの読み込み", path, e))?;
        if meta.len() > import_export::MAX_IMPORT_BYTES as u64 {
            return Err(StoreError::Import(format!(
                "ファイルが大きすぎます（{}バイト）。1MBまでのファイルを選んでください",
                meta.len()
            )));
        }
        let bytes = fs::read(path).map_err(|e| StoreError::io("ファイルの読み込み", path, e))?;
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let (preview, inputs) = import_export::parse_import(&bytes, &file_name, &self.data.targets)
            .map_err(StoreError::Import)?;
        self.pending_import = Some(inputs);
        Ok(preview)
    }

    /// 直前のプレビューのうち indices の行を取り込む
    pub fn import_apply(
        &mut self,
        mode: ImportMode,
        indices: &[usize],
    ) -> Result<ImportResult, StoreError> {
        let pending = self
            .pending_import
            .take()
            .ok_or(StoreError::NoPendingImport)?;

        let mut idx: Vec<usize> = indices.to_vec();
        idx.sort_unstable();
        idx.dedup();

        let mut data = self.data.clone();
        let mut result = ImportResult::default();
        for i in idx {
            let Some(input) = pending.get(i).and_then(Option::as_ref) else {
                // 範囲外・誤りのある行は取り込まない
                result.skipped += 1;
                continue;
            };
            let port = input.local_port.unwrap_or(DEFAULT_LOCAL_PORT);
            let existing = data
                .targets
                .iter_mut()
                .find(|t| t.profile == input.profile && t.env == input.env && t.app == input.app);
            match (existing, mode) {
                (Some(t), ImportMode::Overwrite) => {
                    t.name = input.name.trim().to_string();
                    t.local_port = port;
                    result.updated += 1;
                }
                (Some(_), ImportMode::NewOnly) => result.skipped += 1,
                (None, _) => {
                    data.targets.push(Target {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: input.name.trim().to_string(),
                        profile: input.profile.clone(),
                        env: input.env,
                        app: input.app.clone(),
                        local_port: port,
                    });
                    result.added += 1;
                }
            }
        }

        if result.added + result.updated > 0 {
            if let Err(e) = self.commit(data) {
                // 保存に失敗したらやり直せるようにプレビューを戻す
                self.pending_import = Some(pending);
                return Err(e);
            }
        }
        Ok(result)
    }

    /// エクスポート用JSONをファイルに書き、件数を返す
    pub fn export(&self, path: &Path) -> Result<usize, StoreError> {
        let json = import_export::export_json(&self.data.targets);
        fs::write(path, json)
            .map_err(|e| StoreError::io("エクスポートファイルの書き込み", path, e))?;
        Ok(self.data.targets.len())
    }

    /// data を保存し、成功したら現在の内容にする
    fn commit(&mut self, data: ConfigData) -> Result<(), StoreError> {
        self.persist(&data)?;
        self.data = data;
        Ok(())
    }

    fn persist(&self, data: &ConfigData) -> Result<(), StoreError> {
        let key = self.key.as_ref().ok_or(StoreError::NoKey)?;
        let plain = Zeroizing::new(
            serde_json::to_vec(data).map_err(|e| StoreError::Serialize(e.to_string()))?,
        );
        let enc = crypto::encrypt(key, &plain).map_err(StoreError::Crypto)?;
        write_atomic(&self.dir, &enc)
    }
}

/// config.enc を読んで復号・解析する。失敗理由は日本語で返す
fn read_config(path: &Path, key: &[u8; 32]) -> Result<ConfigData, String> {
    let bytes = fs::read(path).map_err(|e| format!("ファイルを読めません: {e}"))?;
    let plain = crypto::decrypt(key, &bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&plain).map_err(|e| format!("中身の形式が正しくありません: {e}"))
}

/// 一時ファイルに書いて fsync → 現在のファイルを .bak に複製 → リネーム
fn write_atomic(dir: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if !dir.exists() {
        fs::create_dir_all(dir).map_err(|e| StoreError::io("フォルダの作成", dir, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = dir.join(TMP_FILE);
    let target = dir.join(CONFIG_FILE);
    let bak = dir.join(BAK_FILE);

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| StoreError::io("設定ファイルの書き込み", &tmp, e))?;
        #[cfg(unix)]
        {
            // 既存の tmp が残っていた場合にも 0600 にする
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(fs::Permissions::from_mode(0o600));
        }
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .map_err(|e| StoreError::io("設定ファイルの書き込み", &tmp, e))?;
    }

    if target.exists() {
        fs::copy(&target, &bak).map_err(|e| StoreError::io("バックアップの作成", &bak, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&bak, fs::Permissions::from_mode(0o600));
        }
    }

    fs::rename(&tmp, &target).map_err(|e| StoreError::io("設定ファイルの置き換え", &target, e))?;

    #[cfg(unix)]
    {
        // リネームをディスクに反映させる（失敗しても致命的ではない）
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
