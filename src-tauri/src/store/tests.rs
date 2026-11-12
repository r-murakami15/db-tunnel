use std::sync::Arc;

use super::*;
use crate::model::{Env, ImportJudgement};

/// キーチェーンが使えない状況の再現用
struct FailingKeyProvider;

impl KeyProvider for FailingKeyProvider {
    fn load_key(&self) -> Result<Option<Zeroizing<[u8; 32]>>, StoreError> {
        Err(StoreError::Keychain("denied".into()))
    }
    fn store_key(&self, _key: &[u8; 32]) -> Result<(), StoreError> {
        Err(StoreError::Keychain("denied".into()))
    }
}

fn input(name: &str, profile: &str, env: Env, app: &str, port: Option<u16>) -> TargetInput {
    TargetInput {
        id: None,
        name: name.into(),
        profile: profile.into(),
        env,
        app: app.into(),
        local_port: port,
    }
}

fn open(dir: &Path, keys: &Arc<MemoryKeyProvider>) -> Store {
    Store::open(dir.to_path_buf(), Box::new(keys.clone()))
}

fn broken_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("config.enc.broken-"))
        .collect()
}

#[test]
fn fresh_start_generates_key_and_roundtrips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("db-tunnel"); // まだ存在しないフォルダ
    let keys = Arc::new(MemoryKeyProvider::new());

    let mut s = open(&dir, &keys);
    assert!(keys.load_key().unwrap().is_some(), "鍵が生成・保存される");
    let info = s.info();
    assert!(info.keychain_ok);
    assert!(info.load_error.is_none());
    assert_eq!(info.encryption, "AES-256-GCM");
    assert!(s.targets().is_empty());

    let r = s
        .save_target(input(" Acme ", "acme", Env::Dev, "acme-portal", None))
        .unwrap();
    assert_eq!(r.target.name, "Acme");
    assert_eq!(r.target.local_port, 13306);
    assert!(uuid::Uuid::parse_str(&r.target.id).is_ok());
    assert!(r.warnings.is_empty());
    s.update_settings(|st| {
        st.last_selected_id = Some(r.target.id.clone());
        st.window_width = Some(1200);
    })
    .unwrap();

    // ファイルは暗号化されている
    let raw = fs::read(s.config_path()).unwrap();
    assert_eq!(&raw[..5], b"RDSC\x01");
    assert!(!String::from_utf8_lossy(&raw).contains("acme"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(s.config_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(!dir.join(TMP_FILE).exists());

    // 読み直し
    let s2 = open(&dir, &keys);
    assert!(s2.info().load_error.is_none());
    assert_eq!(s2.targets(), s.targets());
    assert_eq!(s2.settings(), s.settings());
    assert_eq!(s2.settings().window_width, Some(1200));
    assert!(s2.target(&r.target.id).is_some());
}

#[test]
fn bak_is_created_on_second_save() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);
    s.save_target(input("A", "p", Env::Dev, "a", None)).unwrap();
    assert!(!tmp.path().join(BAK_FILE).exists());
    let first = fs::read(s.config_path()).unwrap();
    s.save_target(input("B", "p", Env::Stg, "b", Some(14000)))
        .unwrap();
    let bak = fs::read(tmp.path().join(BAK_FILE)).unwrap();
    assert_eq!(bak, first, ".bak は直前の版");
    let key = keys.load_key().unwrap().unwrap();
    let old: ConfigData = serde_json::from_slice(&crypto::decrypt(&key, &bak).unwrap()).unwrap();
    assert_eq!(old.targets.len(), 1);
}

#[test]
fn tampered_file_is_quarantined() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);
    s.save_target(input("A", "p", Env::Dev, "a", None)).unwrap();

    let path = s.config_path();
    let mut raw = fs::read(&path).unwrap();
    let last = raw.len() - 1;
    raw[last] ^= 0x01;
    fs::write(&path, &raw).unwrap();

    let mut s2 = open(tmp.path(), &keys);
    assert!(s2.targets().is_empty());
    let err = s2.info().load_error.expect("load_error が設定される");
    assert!(err.contains("JSONを再インポートしてください"), "{err}");
    assert!(err.contains("config.enc.broken-"), "{err}");
    assert!(s2.info().keychain_ok);
    assert!(!path.exists());
    let broken = broken_files(tmp.path());
    assert_eq!(broken.len(), 1);
    assert_eq!(fs::read(tmp.path().join(&broken[0])).unwrap(), raw);
    // 同じ鍵で保存し直せる
    s2.save_target(input("B", "p", Env::Dev, "b", None))
        .unwrap();
    assert_eq!(open(tmp.path(), &keys).targets().len(), 1);
}

#[test]
fn wrong_magic_is_quarantined() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);
    s.save_target(input("A", "p", Env::Dev, "a", None)).unwrap();
    let mut raw = fs::read(s.config_path()).unwrap();
    raw[..4].copy_from_slice(b"XXXX");
    fs::write(s.config_path(), &raw).unwrap();

    let s2 = open(tmp.path(), &keys);
    assert!(s2.targets().is_empty());
    let err = s2.info().load_error.unwrap();
    assert!(err.contains("設定ファイルの形式ではありません"), "{err}");
    assert_eq!(broken_files(tmp.path()).len(), 1);
}

#[test]
fn missing_key_with_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);
    s.save_target(input("A", "p", Env::Dev, "a", None)).unwrap();
    let old_key = *keys.load_key().unwrap().unwrap();

    keys.clear();
    let mut s2 = open(tmp.path(), &keys);
    assert!(s2.targets().is_empty());
    assert!(s2.info().keychain_ok);
    let err = s2.info().load_error.unwrap();
    assert!(err.contains("鍵がキーチェーンに見つかりません"), "{err}");
    assert_eq!(broken_files(tmp.path()).len(), 1);
    // 新しい鍵が生成され、保存できる
    let new_key = *keys.load_key().unwrap().unwrap();
    assert_ne!(old_key, new_key);
    s2.save_target(input("B", "p", Env::Dev, "b", None))
        .unwrap();
    let s3 = open(tmp.path(), &keys);
    assert!(s3.info().load_error.is_none());
    assert_eq!(s3.targets()[0].name, "B");
}

#[test]
fn keychain_failure_runs_in_memory_only() {
    let tmp = tempfile::tempdir().unwrap();
    // 既存ファイルがあっても退避しない
    let keys = Arc::new(MemoryKeyProvider::new());
    open(tmp.path(), &keys)
        .save_target(input("A", "p", Env::Dev, "a", None))
        .unwrap();

    let mut s = Store::open(tmp.path().to_path_buf(), Box::new(FailingKeyProvider));
    let info = s.info();
    assert!(!info.keychain_ok);
    assert!(info.load_error.is_some());
    assert!(s.targets().is_empty());
    assert!(matches!(
        s.save_target(input("B", "p", Env::Dev, "b", None)),
        Err(StoreError::NoKey)
    ));
    assert!(s.targets().is_empty(), "保存に失敗したら反映しない");
    assert!(broken_files(tmp.path()).is_empty());
    assert_eq!(open(tmp.path(), &keys).targets().len(), 1);
}

#[test]
fn save_target_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);
    let a = s
        .save_target(input("A", "p", Env::Dev, "a", None))
        .unwrap()
        .target;

    // 検証エラー
    match s.save_target(input("", "p q", Env::Dev, "A", Some(80))) {
        Err(StoreError::Validation(errs)) => assert_eq!(errs.len(), 4, "{errs:?}"),
        other => panic!("{other:?}"),
    }
    // 重複キー
    let e = s
        .save_target(input("Other", "p", Env::Dev, "a", Some(14000)))
        .unwrap_err();
    assert!(matches!(e, StoreError::Duplicate(ref n) if n == "A"), "{e}");
    assert!(e.to_string().contains("すでにあります"));
    // env が違えば別物。ポートが同じなら警告だけ
    let b = s
        .save_target(input("B", "p", Env::Prod, "a", None))
        .unwrap();
    assert_eq!(b.warnings.len(), 1);
    assert!(b.warnings[0].contains("13306") && b.warnings[0].contains("「A」"));
    // 自分自身の更新は重複にならない
    let mut upd = input("A2", "p", Env::Dev, "a", Some(15000));
    upd.id = Some(a.id.clone());
    let r = s.save_target(upd).unwrap();
    assert_eq!(r.target.id, a.id);
    assert!(r.warnings.is_empty());
    assert_eq!(s.targets().len(), 2);
    assert_eq!(s.target(&a.id).unwrap().name, "A2");
    // 更新で他と重複
    let mut upd = input("B", "p", Env::Dev, "a", None);
    upd.id = Some(b.target.id.clone());
    assert!(matches!(s.save_target(upd), Err(StoreError::Duplicate(_))));
    // 未知の id
    let mut unknown = input("C", "p", Env::Stg, "c", None);
    unknown.id = Some("nope".into());
    assert!(matches!(
        s.save_target(unknown),
        Err(StoreError::NotFound(_))
    ));

    // 削除（最後に選んだ接続先もクリア）
    s.update_settings(|st| st.last_selected_id = Some(a.id.clone()))
        .unwrap();
    s.delete_target(&a.id).unwrap();
    assert!(s.target(&a.id).is_none());
    assert_eq!(s.settings().last_selected_id, None);
    assert!(matches!(
        s.delete_target(&a.id),
        Err(StoreError::NotFound(_))
    ));
    assert_eq!(open(tmp.path(), &keys).targets().len(), 1);
}

const IMPORT_JSON: &str = r#"{
  "version": 1,
  "targets": [
    {"name": "A new name", "profile": "p", "env": "dev", "app": "a", "localPort": 14000},
    {"name": "B", "profile": "p", "env": "stg", "app": "b"},
    {"name": "Bad", "profile": "p", "env": "xxx", "app": "c"},
    {"name": "C", "profile": "p", "env": "prod", "app": "c", "localPort": 15000}
  ]
}"#;

fn store_with_a(tmp: &Path, keys: &Arc<MemoryKeyProvider>) -> (Store, Target, PathBuf) {
    let mut s = open(tmp, keys);
    let a = s
        .save_target(input("A", "p", Env::Dev, "a", None))
        .unwrap()
        .target;
    let file = tmp.join("import.json");
    fs::write(&file, IMPORT_JSON).unwrap();
    (s, a, file)
}

#[test]
fn import_preview_and_apply_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let (mut s, a, file) = store_with_a(tmp.path(), &keys);

    assert!(matches!(
        s.import_apply(ImportMode::Overwrite, &[0]),
        Err(StoreError::NoPendingImport)
    ));

    let p = s.import_preview(&file).unwrap();
    assert_eq!(p.file_name, "import.json");
    let j: Vec<_> = p.rows.iter().map(|r| r.judgement).collect();
    assert_eq!(
        j,
        vec![
            ImportJudgement::Overwrite,
            ImportJudgement::New,
            ImportJudgement::Error,
            ImportJudgement::New
        ]
    );

    // 3 行目（Error）を選んでも取り込まれない。4 行目は選ばない
    let r = s.import_apply(ImportMode::Overwrite, &[0, 1, 2]).unwrap();
    assert_eq!((r.added, r.updated, r.skipped), (1, 1, 1));
    let s2 = open(tmp.path(), &keys);
    assert_eq!(s2.targets().len(), 2);
    let a2 = s2.target(&a.id).expect("上書きでも id は変わらない");
    assert_eq!(a2.name, "A new name");
    assert_eq!(a2.local_port, 14000);
    let b = s2.targets().iter().find(|t| t.app == "b").unwrap();
    assert_eq!(b.local_port, 13306);

    // プレビューは使い切り
    assert!(matches!(
        s.import_apply(ImportMode::Overwrite, &[3]),
        Err(StoreError::NoPendingImport)
    ));
}

#[test]
fn import_apply_new_only() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let (mut s, a, file) = store_with_a(tmp.path(), &keys);
    s.import_preview(&file).unwrap();
    let r = s.import_apply(ImportMode::NewOnly, &[0, 1, 3, 3]).unwrap();
    assert_eq!((r.added, r.updated, r.skipped), (2, 0, 1));
    assert_eq!(s.targets().len(), 3);
    assert_eq!(s.target(&a.id).unwrap().name, "A");
    assert_eq!(s.target(&a.id).unwrap().local_port, 13306);
    assert_eq!(open(tmp.path(), &keys).targets().len(), 3);
}

#[test]
fn import_preview_rejections() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);

    let big = tmp.path().join("big.json");
    fs::write(&big, vec![b' '; import_export::MAX_IMPORT_BYTES + 1]).unwrap();
    let e = s.import_preview(&big).unwrap_err();
    assert!(matches!(e, StoreError::Import(_)), "{e}");

    let v2 = tmp.path().join("v2.json");
    fs::write(&v2, r#"{"version":2,"targets":[]}"#).unwrap();
    assert!(matches!(s.import_preview(&v2), Err(StoreError::Import(_))));
    assert!(matches!(
        s.import_apply(ImportMode::Overwrite, &[]),
        Err(StoreError::NoPendingImport)
    ));

    assert!(matches!(
        s.import_preview(&tmp.path().join("missing.json")),
        Err(StoreError::Io { .. })
    ));
}

#[test]
fn export_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut s = open(tmp.path(), &keys);
    s.save_target(input("A", "p", Env::Dev, "a", None)).unwrap();
    s.save_target(input("B", "q", Env::Prod, "b", Some(20000)))
        .unwrap();
    let out = tmp.path().join("export.json");
    assert_eq!(s.export(&out).unwrap(), 2);
    let text = fs::read_to_string(&out).unwrap();
    assert!(!text.contains("\"id\""));

    // 別の環境（空のストア）に取り込む
    let tmp2 = tempfile::tempdir().unwrap();
    let mut other = open(tmp2.path(), &Arc::new(MemoryKeyProvider::new()));
    let p = other.import_preview(&out).unwrap();
    assert!(p.rows.iter().all(|r| r.judgement == ImportJudgement::New));
    let r = other.import_apply(ImportMode::Overwrite, &[0, 1]).unwrap();
    assert_eq!(r.added, 2);
    let strip = |ts: &[Target]| {
        ts.iter()
            .map(|t| (t.name.clone(), t.key(), t.local_port))
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(other.targets()), strip(s.targets()));

    // 同じストアに戻すとすべて上書き判定
    let p = s.import_preview(&out).unwrap();
    assert!(p
        .rows
        .iter()
        .all(|r| r.judgement == ImportJudgement::Overwrite));
}

fn names(store: &Store) -> Vec<String> {
    store.targets().iter().map(|t| t.name.clone()).collect()
}

#[test]
fn move_target_swaps_neighbours_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut store = open(dir.path(), &keys);
    let a = store
        .save_target(input("A", "p", Env::Dev, "a", None))
        .unwrap()
        .target;
    store
        .save_target(input("B", "p", Env::Stg, "a", None))
        .unwrap();
    let c = store
        .save_target(input("C", "p", Env::Prod, "a", None))
        .unwrap()
        .target;

    store.move_target(&c.id, MoveDirection::Up).unwrap();
    assert_eq!(names(&store), ["A", "C", "B"]);
    store.move_target(&a.id, MoveDirection::Down).unwrap();
    assert_eq!(names(&store), ["C", "A", "B"]);

    // 端では何もしない
    store.move_target(&c.id, MoveDirection::Up).unwrap();
    store
        .move_target(&store.targets()[2].id.clone(), MoveDirection::Down)
        .unwrap();
    assert_eq!(names(&store), ["C", "A", "B"]);

    assert!(matches!(
        store.move_target("nope", MoveDirection::Up),
        Err(StoreError::NotFound(_))
    ));

    // 再起動後も並び順が保たれる
    let reopened = open(dir.path(), &keys);
    assert_eq!(names(&reopened), ["C", "A", "B"]);
}

#[test]
fn duplicate_target_inserts_after_source() {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(MemoryKeyProvider::new());
    let mut store = open(dir.path(), &keys);
    let a = store
        .save_target(input("A", "p", Env::Dev, "a", None))
        .unwrap()
        .target;
    store
        .save_target(input("B", "p", Env::Stg, "a", None))
        .unwrap();

    // 同じ組み合わせのままでは追加できない
    let mut same = input("A のコピー", "p", Env::Dev, "a", None);
    same.id = Some(a.id.clone());
    assert!(matches!(
        store.duplicate_target(&a.id, same),
        Err(StoreError::Duplicate(_))
    ));
    assert_eq!(names(&store), ["A", "B"]);

    // id を渡しても無視され、新しい接続先として複製元の直後に入る
    let mut copy = input("A のコピー", "p", Env::Prod, "a", Some(13306));
    copy.id = Some(a.id.clone());
    let res = store.duplicate_target(&a.id, copy).unwrap();
    assert_ne!(res.target.id, a.id);
    assert_eq!(names(&store), ["A", "A のコピー", "B"]);
    assert!(!res.warnings.is_empty(), "同じポートの警告が出る");

    assert!(matches!(
        store.duplicate_target("nope", input("X", "p", Env::Dev, "x", None)),
        Err(StoreError::NotFound(_))
    ));
}
