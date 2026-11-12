//! 接続先のインポート・エクスポート（平文JSON）と入力値の検証。
//!
//! 形式: `{"version":1,"targets":[{name,profile,env,app,localPort?}]}`

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::model::{
    Env, ImportJudgement, ImportPreview, ImportRow, Target, TargetInput, MIN_LOCAL_PORT,
};

pub const IMPORT_VERSION: u64 = 1;
pub const MAX_IMPORT_BYTES: usize = 1024 * 1024;
pub const MAX_IMPORT_TARGETS: usize = 500;
pub const MAX_NAME_CHARS: usize = 64;

const TOP_FIELDS: &[&str] = &["version", "targets"];
const TARGET_FIELDS: &[&str] = &["name", "profile", "env", "app", "localPort"];

// ---------------------------------------------------------------------------
// 検証
// ---------------------------------------------------------------------------

fn validate_name(name: &str, errors: &mut Vec<String>) {
    let t = name.trim();
    if t.is_empty() {
        errors.push("名前が空です".into());
    } else if t.chars().count() > MAX_NAME_CHARS {
        errors.push(format!("名前は{MAX_NAME_CHARS}文字以内にしてください"));
    }
}

fn validate_profile(profile: &str, errors: &mut Vec<String>) {
    if profile.is_empty() {
        errors.push("プロファイル名が空です".into());
    } else if profile.chars().any(char::is_whitespace) {
        errors.push("プロファイル名に空白は使えません".into());
    }
}

fn validate_app(app: &str, errors: &mut Vec<String>) {
    if app.is_empty() {
        errors.push("アプリ名が空です".into());
    } else if !app
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        errors.push("アプリ名は英小文字・数字・ハイフン（-）だけで指定してください".into());
    }
}

fn port_range_error() -> String {
    format!("ポートは{MIN_LOCAL_PORT}〜65535の範囲で指定してください")
}

fn validate_port(port: Option<i64>, errors: &mut Vec<String>) {
    if let Some(p) = port {
        if p < i64::from(MIN_LOCAL_PORT) || p > i64::from(u16::MAX) {
            errors.push(port_range_error());
        }
    }
}

/// 名前・プロファイル・アプリ名・ポートを検証し、誤りを日本語で返す（空なら問題なし）。
/// env は型（`Env`）で保証されるのでここでは見ない。
pub fn validate_fields(name: &str, profile: &str, app: &str, port: Option<i64>) -> Vec<String> {
    let mut errors = Vec::new();
    validate_name(name, &mut errors);
    validate_profile(profile, &mut errors);
    validate_app(app, &mut errors);
    validate_port(port, &mut errors);
    errors
}

// ---------------------------------------------------------------------------
// インポート
// ---------------------------------------------------------------------------

/// 文字列フィールドを取り出す。表示用の値と、検証に使う値（文字列のときだけ）を返す
fn str_field(
    obj: &Map<String, Value>,
    key: &str,
    errors: &mut Vec<String>,
    validate: fn(&str, &mut Vec<String>),
) -> (String, Option<String>) {
    match obj.get(key) {
        Some(Value::String(s)) => {
            validate(s, errors);
            (s.clone(), Some(s.clone()))
        }
        None | Some(Value::Null) => {
            validate("", errors);
            (String::new(), None)
        }
        Some(other) => {
            errors.push(format!("{key} は文字列で指定してください"));
            (other.to_string(), None)
        }
    }
}

fn validate_env_str(s: &str, errors: &mut Vec<String>) {
    if s.is_empty() {
        errors.push("環境が空です".into());
    } else if Env::parse(s).is_none() {
        errors.push(format!("環境は dev / stg / prod のいずれかです（\"{s}\"）"));
    }
}

fn unknown_field_warning(key: &str) -> String {
    format!("未知のフィールド \"{key}\" は無視します")
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "真偽値",
        Value::Number(_) => "数値",
        Value::String(_) => "文字列",
        Value::Array(_) => "配列",
        Value::Object(_) => "オブジェクト",
    }
}

/// 1行分を解析する。戻り値の TargetInput は誤りがなければ Some
fn parse_row(index: usize, v: &Value) -> (ImportRow, Option<TargetInput>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let Some(obj) = v.as_object() else {
        errors.push(format!(
            "targets[{index}] がオブジェクトではありません（{}）",
            type_name(v)
        ));
        let row = ImportRow {
            index,
            name: String::new(),
            profile: String::new(),
            env: String::new(),
            app: String::new(),
            local_port: None,
            judgement: ImportJudgement::Error,
            errors,
            warnings,
        };
        return (row, None);
    };

    let (name_disp, name) = str_field(obj, "name", &mut errors, validate_name);
    let (profile_disp, profile) = str_field(obj, "profile", &mut errors, validate_profile);
    let (env_disp, env_s) = str_field(obj, "env", &mut errors, validate_env_str);
    let (app_disp, app) = str_field(obj, "app", &mut errors, validate_app);

    // localPort: 省略・null は既定値。整数以外は誤り
    let mut port_disp: Option<i64> = None;
    let mut port: Option<u16> = None;
    match obj.get("localPort") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) if n.as_i64().is_some() => {
            let p = n.as_i64().unwrap_or_default();
            port_disp = Some(p);
            let before = errors.len();
            validate_port(Some(p), &mut errors);
            if errors.len() == before {
                port = u16::try_from(p).ok();
            }
        }
        Some(Value::Number(n)) if n.is_u64() => errors.push(port_range_error()),
        Some(_) => errors.push("localPort は整数で指定してください".into()),
    }

    for key in obj.keys() {
        if !TARGET_FIELDS.contains(&key.as_str()) {
            warnings.push(unknown_field_warning(key));
        }
    }

    let input = if errors.is_empty() {
        match (name, profile, env_s.as_deref().and_then(Env::parse), app) {
            (Some(name), Some(profile), Some(env), Some(app)) => Some(TargetInput {
                id: None,
                name: name.trim().to_string(),
                profile,
                env,
                app,
                local_port: port,
            }),
            _ => None,
        }
    } else {
        None
    };

    let row = ImportRow {
        index,
        name: name_disp,
        profile: profile_disp,
        env: env_disp,
        app: app_disp,
        local_port: port_disp,
        judgement: ImportJudgement::Error, // 呼び出し元で決める
        errors,
        warnings,
    };
    (row, input)
}

/// インポートするJSONを解析してプレビューを作る。
///
/// ファイル全体を拒否する場合（JSONでない、サイズ・件数超過、未対応の version 等）は
/// `Err(日本語メッセージ)`。行ごとの誤りはその行の判定を Error にし、他の行は取り込める。
/// 戻り値の Vec は rows と同じ並びで、取り込める行だけ Some。
pub fn parse_import(
    bytes: &[u8],
    file_name: &str,
    existing: &[Target],
) -> Result<(ImportPreview, Vec<Option<TargetInput>>), String> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "ファイルが大きすぎます（{}バイト）。1MBまでのファイルを選んでください",
            bytes.len()
        ));
    }
    let root: Value = serde_json::from_slice(bytes).map_err(|e| {
        format!(
            "JSONとして読み込めません（{}行目 {}列目）: {}",
            e.line(),
            e.column(),
            e
        )
    })?;
    let Some(top) = root.as_object() else {
        return Err(format!(
            "ファイルの形式が違います（最上位が{}です。オブジェクトが必要です）",
            type_name(&root)
        ));
    };

    match top.get("version") {
        None => return Err("version がありません".into()),
        Some(v) if v.as_u64() == Some(IMPORT_VERSION) => {}
        Some(v) => {
            return Err(format!(
                "version が未対応の値です（{v}）。対応しているのは {IMPORT_VERSION} です"
            ))
        }
    }

    let targets = match top.get("targets") {
        None => return Err("targets がありません".into()),
        Some(Value::Array(a)) => a,
        Some(v) => return Err(format!("targets が配列ではありません（{}）", type_name(v))),
    };
    if targets.len() > MAX_IMPORT_TARGETS {
        return Err(format!(
            "targets が多すぎます（{}件）。{MAX_IMPORT_TARGETS}件までにしてください",
            targets.len()
        ));
    }

    let mut warnings = Vec::new();
    for key in top.keys() {
        if !TOP_FIELDS.contains(&key.as_str()) {
            warnings.push(unknown_field_warning(key));
        }
    }

    let existing_keys: std::collections::HashSet<(String, Env, String)> =
        existing.iter().map(Target::key).collect();
    // ファイル内の重複チェック用（キー → 最初に出てきた添字）
    let mut seen: HashMap<(String, Env, String), usize> = HashMap::new();

    let mut rows = Vec::with_capacity(targets.len());
    let mut inputs = Vec::with_capacity(targets.len());
    for (i, v) in targets.iter().enumerate() {
        let (mut row, mut input) = parse_row(i, v);
        if let Some(inp) = &input {
            let key = (inp.profile.clone(), inp.env, inp.app.clone());
            if let Some(first) = seen.get(&key) {
                row.errors.push(format!(
                    "ファイル内で重複しています（targets[{first}] と同じプロファイル・環境・アプリ名）"
                ));
                input = None;
            } else {
                seen.insert(key.clone(), i);
                row.judgement = if existing_keys.contains(&key) {
                    ImportJudgement::Overwrite
                } else {
                    ImportJudgement::New
                };
            }
        }
        if input.is_none() {
            row.judgement = ImportJudgement::Error;
        }
        rows.push(row);
        inputs.push(input);
    }

    Ok((
        ImportPreview {
            file_name: file_name.to_string(),
            rows,
            warnings,
        },
        inputs,
    ))
}

// ---------------------------------------------------------------------------
// エクスポート
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportTarget<'a> {
    name: &'a str,
    profile: &'a str,
    env: Env,
    app: &'a str,
    local_port: u16,
}

#[derive(Serialize)]
struct ExportFile<'a> {
    version: u64,
    targets: Vec<ExportTarget<'a>>,
}

/// エクスポート用のJSON（整形済み、id は含めない）を作る
pub fn export_json(targets: &[Target]) -> String {
    let file = ExportFile {
        version: IMPORT_VERSION,
        targets: targets
            .iter()
            .map(|t| ExportTarget {
                name: &t.name,
                profile: &t.profile,
                env: t.env,
                app: &t.app,
                local_port: t.local_port,
            })
            .collect(),
    };
    // 文字列と数値だけなので失敗しない
    let mut s = serde_json::to_string_pretty(&file).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ImportJudgement as J;

    fn target(id: &str, name: &str, profile: &str, env: Env, app: &str, port: u16) -> Target {
        Target {
            id: id.into(),
            name: name.into(),
            profile: profile.into(),
            env,
            app: app.into(),
            local_port: port,
        }
    }

    #[test]
    fn validate() {
        assert!(validate_fields("A", "p", "a-1", None).is_empty());
        assert!(validate_fields("A", "p", "a", Some(1024)).is_empty());
        assert!(validate_fields("A", "p", "a", Some(65535)).is_empty());
        assert_eq!(validate_fields("  ", "p", "a", None), vec!["名前が空です"]);
        assert_eq!(validate_fields(&"あ".repeat(64), "p", "a", None).len(), 0);
        assert_eq!(validate_fields(&"あ".repeat(65), "p", "a", None).len(), 1);
        assert_eq!(
            validate_fields("A", "", "a", None),
            vec!["プロファイル名が空です"]
        );
        assert_eq!(validate_fields("A", "a b", "a", None).len(), 1);
        assert_eq!(
            validate_fields("A", "p", "", None),
            vec!["アプリ名が空です"]
        );
        assert_eq!(validate_fields("A", "p", "Abc", None).len(), 1);
        assert_eq!(validate_fields("A", "p", "a_b", None).len(), 1);
        assert_eq!(validate_fields("A", "p", "a", Some(1023)).len(), 1);
        assert_eq!(validate_fields("A", "p", "a", Some(65536)).len(), 1);
        assert_eq!(validate_fields("", "", "", Some(0)).len(), 4);
    }

    #[test]
    fn preview_judgements() {
        let existing = vec![target("x", "Old", "p1", Env::Dev, "app1", 13306)];
        let json = r#"{
            "version": 1,
            "comment": "hello",
            "targets": [
                {"name": "A", "profile": "p1", "env": "dev", "app": "app1"},
                {"name": "B", "profile": "p2", "env": "stg", "app": "app2", "localPort": 13307, "foo": 1},
                {"name": "C", "profile": "p3", "env": "qa", "app": "app3"},
                {"name": "D", "profile": "p2", "env": "stg", "app": "app2"},
                {"name": "", "profile": "p4", "env": "prod", "app": "Bad_App", "localPort": 80},
                {"name": 5, "profile": "p5", "env": "dev", "app": "a", "localPort": "x"},
                "oops",
                {"name": "H", "profile": "p8", "env": "prod", "app": "h", "localPort": null}
            ]
        }"#;
        let (p, inputs) = parse_import(json.as_bytes(), "t.json", &existing).unwrap();
        assert_eq!(p.file_name, "t.json");
        assert_eq!(
            p.warnings,
            vec!["未知のフィールド \"comment\" は無視します"]
        );
        let j: Vec<J> = p.rows.iter().map(|r| r.judgement).collect();
        assert_eq!(
            j,
            vec![
                J::Overwrite,
                J::New,
                J::Error,
                J::Error,
                J::Error,
                J::Error,
                J::Error,
                J::New
            ]
        );
        assert_eq!(inputs.len(), 8);
        assert!(inputs[0].is_some() && inputs[1].is_some() && inputs[7].is_some());
        assert!(inputs[2..7].iter().all(Option::is_none));

        // 未知フィールドの警告（行）
        assert_eq!(
            p.rows[1].warnings,
            vec!["未知のフィールド \"foo\" は無視します"]
        );
        assert_eq!(p.rows[1].local_port, Some(13307));
        assert_eq!(inputs[1].as_ref().unwrap().local_port, Some(13307));
        // env 不正
        assert_eq!(p.rows[2].env, "qa");
        assert_eq!(p.rows[2].errors.len(), 1);
        // ファイル内の重複
        assert!(p.rows[3].errors[0].contains("ファイル内で重複しています"));
        // 複数の誤り
        assert_eq!(p.rows[4].errors.len(), 3, "{:?}", p.rows[4].errors);
        assert_eq!(p.rows[4].local_port, Some(80));
        // 型違い
        assert_eq!(p.rows[5].name, "5");
        assert_eq!(p.rows[5].errors.len(), 2, "{:?}", p.rows[5].errors);
        assert_eq!(p.rows[5].local_port, None);
        // オブジェクトでない
        assert!(p.rows[6].errors[0].contains("targets[6]"));
        // null ポートは既定値扱い
        assert_eq!(inputs[7].as_ref().unwrap().local_port, None);
        assert_eq!(
            p.rows.iter().map(|r| r.index).collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
    }

    #[test]
    fn whole_file_rejections() {
        let e = parse_import(b"{not json", "f", &[]).unwrap_err();
        assert!(e.contains("1行目"), "{e}");
        assert!(parse_import(b"[]", "f", &[]).is_err());
        assert!(parse_import(br#"{"targets":[]}"#, "f", &[])
            .unwrap_err()
            .contains("version"));
        assert!(parse_import(br#"{"version":2,"targets":[]}"#, "f", &[])
            .unwrap_err()
            .contains("未対応"));
        assert!(parse_import(br#"{"version":"1","targets":[]}"#, "f", &[]).is_err());
        assert!(parse_import(br#"{"version":1}"#, "f", &[]).is_err());
        assert!(parse_import(br#"{"version":1,"targets":{}}"#, "f", &[]).is_err());

        let one = r#"{"name":"A","profile":"p","env":"dev","app":"a"}"#;
        let many = format!(
            r#"{{"version":1,"targets":[{}]}}"#,
            vec![one; MAX_IMPORT_TARGETS + 1].join(",")
        );
        assert!(parse_import(many.as_bytes(), "f", &[])
            .unwrap_err()
            .contains("多すぎ"));
        let ok = format!(
            r#"{{"version":1,"targets":[{}]}}"#,
            vec![one; MAX_IMPORT_TARGETS].join(",")
        );
        assert!(parse_import(ok.as_bytes(), "f", &[]).is_ok());

        let big = vec![b' '; MAX_IMPORT_BYTES + 1];
        assert!(parse_import(&big, "f", &[])
            .unwrap_err()
            .contains("大きすぎ"));
    }

    #[test]
    fn export_has_no_id_and_roundtrips() {
        let ts = vec![
            target("id-1", "A", "p1", Env::Dev, "a1", 13306),
            target("id-2", "B", "p2", Env::Prod, "a2", 20000),
        ];
        let s = export_json(&ts);
        assert!(!s.contains("id-1") && !s.contains("\"id\""));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["targets"][1]["localPort"], 20000);
        assert_eq!(v["targets"][1]["env"], "prod");

        let (p, inputs) = parse_import(s.as_bytes(), "e.json", &[]).unwrap();
        assert!(p.warnings.is_empty());
        assert!(p
            .rows
            .iter()
            .all(|r| r.judgement == J::New && r.warnings.is_empty()));
        let back: Vec<_> = inputs.into_iter().map(Option::unwrap).collect();
        assert_eq!(back[1].name, "B");
        assert_eq!(back[1].env, Env::Prod);
        assert_eq!(back[1].local_port, Some(20000));
    }
}
