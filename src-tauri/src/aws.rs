//! AWS CLI まわり: ツールの検出、前提確認、プロファイル一覧、コマンドの組み立て、出力の解析。

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use zeroize::Zeroizing;

use crate::model::{Env, Prerequisites, ToolStatus, REGION};
use crate::process;

/// 利用者が設定画面で指定したツールのパス（未指定なら自動検出）
#[derive(Clone, Debug, Default)]
pub struct ToolPaths {
    pub aws: Option<PathBuf>,
    pub plugin: Option<PathBuf>,
}

/// `--version` の確認に使うタイムアウト
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(windows)]
const AWS_NAMES: &[&str] = &["aws.exe", "aws.cmd"];
#[cfg(not(windows))]
const AWS_NAMES: &[&str] = &["aws"];

#[cfg(windows)]
const PLUGIN_NAMES: &[&str] = &["session-manager-plugin.exe"];
#[cfg(not(windows))]
const PLUGIN_NAMES: &[&str] = &["session-manager-plugin"];

/// Finder から起動したアプリはシェルの PATH を引き継がないため、よく使われる場所も探す
fn extra_dirs_aws() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from("/usr/local/bin"));
        v.push(PathBuf::from("/opt/homebrew/bin"));
        v.push(PathBuf::from("/usr/local/aws-cli"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        v.push(PathBuf::from("/usr/local/bin"));
        v.push(PathBuf::from("/usr/bin"));
    }
    #[cfg(windows)]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            v.push(PathBuf::from(pf).join("Amazon").join("AWSCLIV2"));
        }
        v.push(PathBuf::from(r"C:\Program Files\Amazon\AWSCLIV2"));
    }
    v
}

fn extra_dirs_plugin() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from("/usr/local/bin"));
        v.push(PathBuf::from("/opt/homebrew/bin"));
        v.push(PathBuf::from("/usr/local/sessionmanagerplugin/bin"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        v.push(PathBuf::from("/usr/local/bin"));
        v.push(PathBuf::from("/usr/bin"));
        v.push(PathBuf::from("/usr/local/sessionmanagerplugin/bin"));
    }
    #[cfg(windows)]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            v.push(
                PathBuf::from(pf)
                    .join("Amazon")
                    .join("SessionManagerPlugin")
                    .join("bin"),
            );
        }
        v.push(PathBuf::from(
            r"C:\Program Files\Amazon\SessionManagerPlugin\bin",
        ));
    }
    v
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_tool(override_path: Option<&Path>, names: &[&str], extra: Vec<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        if p.as_os_str().is_empty() {
            // 空文字は未指定として扱う
        } else if is_executable(p) {
            return Some(p.to_path_buf());
        } else {
            return None;
        }
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(extra);
    for d in dirs {
        for n in names {
            let cand = d.join(n);
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// aws コマンドを探す。override_path があればそれだけを見る（実行できなければ None）
pub fn resolve_aws(override_path: Option<&Path>) -> Option<PathBuf> {
    find_tool(override_path, AWS_NAMES, extra_dirs_aws())
}

/// session-manager-plugin を探す。override_path があればそれだけを見る
pub fn resolve_plugin(override_path: Option<&Path>) -> Option<PathBuf> {
    find_tool(override_path, PLUGIN_NAMES, extra_dirs_plugin())
}

/// 子プロセス用の PATH。aws と plugin のあるディレクトリ、（Macでは）/usr/local/bin と
/// /opt/homebrew/bin を先頭に足す。`aws ssm start-session` が plugin を見つけられるようにするため
pub fn augmented_path(aws: Option<&Path>, plugin: Option<&Path>) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for p in [aws, plugin].into_iter().flatten() {
        if let Some(d) = p.parent() {
            if !d.as_os_str().is_empty() {
                dirs.push(d.to_path_buf());
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    let mut seen = BTreeSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    std::env::join_paths(dirs).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

/// 子プロセス用の共通設定（PATH・ページャ無効・自動プロンプト無効）をしたコマンド
pub fn tool_command(program: &Path, aws: Option<&Path>, plugin: Option<&Path>) -> Command {
    let mut cmd = process::command(program);
    cmd.env("PATH", augmented_path(aws, plugin))
        .env("AWS_PAGER", "")
        .env("AWS_CLI_AUTO_PROMPT", "off");
    cmd
}

/// 1つの接続先で使う AWS CLI（プロファイルとリージョンを必ず付ける）
#[derive(Clone, Debug)]
pub struct AwsCli {
    pub aws: PathBuf,
    pub plugin: Option<PathBuf>,
    pub profile: String,
}

impl AwsCli {
    /// `aws <args> --profile <p> --region ap-northeast-1` のコマンドを作る
    pub fn command<S: AsRef<str>>(&self, args: &[S]) -> Command {
        let mut cmd = tool_command(&self.aws, Some(&self.aws), self.plugin.as_deref());
        for a in args {
            cmd.arg(a.as_ref());
        }
        cmd.args(["--profile", &self.profile, "--region", REGION]);
        cmd
    }
}

// ---------------------------------------------------------------------------
// 前提確認
// ---------------------------------------------------------------------------

/// AWS CLI と Session Manager Plugin の有無とバージョンを確認する
pub async fn check_prerequisites(paths: &ToolPaths) -> Prerequisites {
    let aws = resolve_aws(paths.aws.as_deref());
    let plugin = resolve_plugin(paths.plugin.as_deref());

    let aws_cli = match &aws {
        None => not_found(paths.aws.as_deref(), "AWS CLIが見つかりません"),
        Some(p) => {
            check_tool(p, aws.as_deref(), plugin.as_deref(), |out| {
                parse_aws_version(out)
            })
            .await
        }
    };
    let session_manager_plugin = match &plugin {
        None => not_found(
            paths.plugin.as_deref(),
            "Session Manager Pluginが見つかりません",
        ),
        Some(p) => {
            check_tool(p, aws.as_deref(), plugin.as_deref(), |out| {
                parse_plugin_version(out)
            })
            .await
        }
    };
    let ok = aws_cli.found && session_manager_plugin.found;
    Prerequisites {
        aws_cli,
        session_manager_plugin,
        ok,
    }
}

fn not_found(override_path: Option<&Path>, msg: &str) -> ToolStatus {
    let error = match override_path {
        Some(p) if !p.as_os_str().is_empty() => {
            format!("指定されたパスに実行ファイルがありません: {}", p.display())
        }
        _ => format!("{msg}。インストールするか、設定画面でパスを指定してください"),
    };
    ToolStatus {
        found: false,
        path: None,
        version: None,
        error: Some(error),
    }
}

async fn check_tool(
    program: &Path,
    aws: Option<&Path>,
    plugin: Option<&Path>,
    parse: impl Fn(&str) -> Option<String>,
) -> ToolStatus {
    let mut cmd = tool_command(program, aws, plugin);
    cmd.arg("--version");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let path = Some(program.display().to_string());
    match tokio::time::timeout(VERSION_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => {
            // AWS CLI v1 は stderr にバージョンを出すので両方を見る
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let version = parse(&text);
            let error = if out.status.success() && version.is_some() {
                None
            } else if !out.status.success() {
                Some(format!(
                    "バージョンを確認できませんでした（終了コード {}）",
                    process::exit_text(&out.status)
                ))
            } else {
                Some("バージョンを確認できませんでした".to_string())
            };
            ToolStatus {
                found: true,
                path,
                version,
                error,
            }
        }
        Ok(Err(e)) => ToolStatus {
            found: false,
            path,
            version: None,
            error: Some(format!("起動できませんでした: {e}")),
        },
        Err(_) => ToolStatus {
            found: true,
            path,
            version: None,
            error: Some("バージョンの確認がタイムアウトしました".to_string()),
        },
    }
}

/// "aws-cli/2.31.3 Python/3.13.7 ..." → "2.31.3"
pub fn parse_aws_version(text: &str) -> Option<String> {
    let idx = text.find("aws-cli/")?;
    let rest = &text[idx + "aws-cli/".len()..];
    let v: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// "1.2.650.0" → "1.2.650.0"（最初の空でない行。数字で始まること）
pub fn parse_plugin_version(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// プロファイル一覧
// ---------------------------------------------------------------------------

/// `~/.aws/config`（AWS_CONFIG_FILE があればそちら）のプロファイル名一覧
pub fn list_profiles() -> Vec<String> {
    let path = match std::env::var_os("AWS_CONFIG_FILE") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => match dirs::home_dir() {
            Some(h) => h.join(".aws").join("config"),
            None => return Vec::new(),
        },
    };
    match std::fs::read(&path) {
        Ok(bytes) => parse_profiles(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

/// config の内容からプロファイル名を取り出す（"[default]" → default、"[profile x]" → x）。並べ替え・重複除去済み
pub fn parse_profiles(content: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    for line in content.lines() {
        let line = line.trim();
        let Some(inner) = line.strip_prefix('[').and_then(|l| {
            // 行末コメントを許す: "[profile x] # comment"
            l.split_once(']').map(|(a, _)| a)
        }) else {
            continue;
        };
        let inner = inner.trim();
        if inner == "default" {
            set.insert("default".to_string());
        } else if let Some(name) = inner.strip_prefix("profile") {
            // "profile" の直後は空白であること（"profilex" は除外）
            if name.starts_with(char::is_whitespace) {
                let name = name.trim();
                if !name.is_empty() {
                    set.insert(name.to_string());
                }
            }
        }
    }
    set.into_iter().collect()
}

// ---------------------------------------------------------------------------
// 検索条件
// ---------------------------------------------------------------------------

/// 踏み台の Name タグの検索条件: "*{app}*-{env}*bastion*"
pub fn bastion_name_filter(app: &str, env: Env) -> String {
    format!("*{}*-{}*bastion*", app, env.as_str())
}

/// RDS の識別子が対象か: app で始まり "-{env}-mysql" で終わる
pub fn rds_matches(identifier: &str, app: &str, env: Env) -> bool {
    identifier.starts_with(app) && identifier.ends_with(&format!("-{}-mysql", env.as_str()))
}

/// RDS の検索条件（エラー表示用）
pub fn rds_condition(app: &str, env: Env) -> String {
    format!("{}*-{}-mysql", app, env.as_str())
}

// ---------------------------------------------------------------------------
// 出力の解析
// ---------------------------------------------------------------------------

/// `sts get-caller-identity --output json` の Arn
pub fn parse_caller_arn(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    v.get("Arn")?.as_str().map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bastion {
    pub id: String,
    pub name: String,
}

/// describe-instances（--query で {id, name} の配列にしたもの）の解析
pub fn parse_bastions(json: &str) -> Result<Vec<Bastion>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let arr = match v {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        _ => return Err("配列ではありません".to_string()),
    };
    let mut out = Vec::new();
    for item in arr {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push(Bastion {
            id: id.to_string(),
            name,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdsInstance {
    pub identifier: String,
    pub endpoint: Option<String>,
    pub secret_arn: Option<String>,
}

/// describe-db-instances の JSON から、条件に合うものを取り出す
pub fn parse_rds(json: &str, app: &str, env: Env) -> Result<Vec<RdsInstance>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = v
        .get("DBInstances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|db| {
            let identifier = db.get("DBInstanceIdentifier")?.as_str()?;
            if !rds_matches(identifier, app, env) {
                return None;
            }
            Some(RdsInstance {
                identifier: identifier.to_string(),
                endpoint: db
                    .pointer("/Endpoint/Address")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                secret_arn: db
                    .pointer("/MasterUserSecret/SecretArn")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect())
}

/// SecretString（JSON）からユーザー名とパスワードを取り出す
pub fn parse_secret(text: &str) -> Option<(String, Zeroizing<String>)> {
    #[derive(serde::Deserialize)]
    struct Secret {
        username: String,
        password: String,
    }
    let s: Secret = serde_json::from_str(text.trim()).ok()?;
    // String をそのまま移すのでヒープ上の値はコピーされない
    let password = Zeroizing::new(s.password);
    if s.username.is_empty() || password.is_empty() {
        return None;
    }
    Some((s.username, password))
}

/// stderr から AWS のエラーコードを取り出す
/// （"An error occurred (AccessDeniedException) when calling ..." → "AccessDeniedException"）
pub fn parse_error_code(stderr: &str) -> Option<String> {
    let marker = "An error occurred (";
    let idx = stderr.find(marker)?;
    let rest = &stderr[idx + marker.len()..];
    let end = rest.find(')')?;
    let code = rest[..end].trim();
    if code.is_empty() {
        None
    } else {
        Some(code.to_string())
    }
}

/// 権限不足のエラーなら、そのエラーコードを返す
pub fn access_denied_code(stderr: &str) -> Option<String> {
    if let Some(code) = parse_error_code(stderr) {
        if code.contains("AccessDenied")
            || code == "UnauthorizedOperation"
            || code == "UnauthorizedAccess"
        {
            return Some(code);
        }
    }
    if stderr.contains("AccessDenied") {
        return Some("AccessDenied".to_string());
    }
    if stderr.contains("UnauthorizedOperation") {
        return Some("UnauthorizedOperation".to_string());
    }
    None
}

/// start-session の出力行が "Port <N> opened" を含むか
pub fn is_port_opened(line: &str, port: u16) -> bool {
    line.contains(&format!("Port {port} opened"))
}

/// 出力にタイムアウトを示す文言があるか
pub fn mentions_timeout(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("timed out") || l.contains("timeout")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_aws_version("aws-cli/2.31.3 Python/3.13.7 Darwin/25.0.0 source/arm64\n"),
            Some("2.31.3".to_string())
        );
        assert_eq!(
            parse_aws_version("\nfoo aws-cli/1.18.0 Python"),
            Some("1.18.0".to_string())
        );
        assert_eq!(parse_aws_version("command not found"), None);
        assert_eq!(
            parse_plugin_version("\n1.2.650.0\n"),
            Some("1.2.650.0".to_string())
        );
        assert_eq!(parse_plugin_version("error\n"), None);
    }

    #[test]
    fn profile_parsing() {
        let cfg = "\
[default]
region = ap-northeast-1

[profile zeta]
sso_session = x
[profile  alpha ] # comment
[sso-session x]
[services s]
[profilex]
[profile alpha]
  [profile beta]
; [profile commented]
";
        assert_eq!(
            parse_profiles(cfg),
            vec!["alpha", "beta", "default", "zeta"]
        );
        assert!(parse_profiles("").is_empty());
    }

    #[test]
    fn bastion_filter() {
        assert_eq!(bastion_name_filter("acme", Env::Dev), "*acme*-dev*bastion*");
        assert_eq!(bastion_name_filter("x", Env::Prod), "*x*-prod*bastion*");
    }

    #[test]
    fn rds_matching() {
        assert!(rds_matches("acme-portal-dev-mysql", "acme", Env::Dev));
        assert!(rds_matches("acme-dev-mysql", "acme", Env::Dev));
        assert!(!rds_matches("acme-portal-stg-mysql", "acme", Env::Dev));
        assert!(!rds_matches("other-acme-dev-mysql", "acme", Env::Dev));
        assert!(!rds_matches("acme-dev-mysql-replica", "acme", Env::Dev));
        assert!(!rds_matches("acme-portal-dev-postgres", "acme", Env::Dev));
    }

    #[test]
    fn bastion_parsing() {
        assert_eq!(parse_bastions("[]").unwrap(), vec![]);
        assert_eq!(parse_bastions("null").unwrap(), vec![]);
        let b = parse_bastions(r#"[{"id":"i-1","name":"a-dev-bastion"},{"id":"i-2","name":null}]"#)
            .unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].id, "i-1");
        assert_eq!(b[1].name, "");
        assert!(parse_bastions("not json").is_err());
    }

    #[test]
    fn rds_parsing() {
        let json = r#"{"DBInstances":[
            {"DBInstanceIdentifier":"acme-portal-dev-mysql","Endpoint":{"Address":"acme.xxx.rds.amazonaws.com","Port":3306},
             "MasterUserSecret":{"SecretArn":"arn:aws:secretsmanager:ap-northeast-1:1:secret:rds!db-1"}},
            {"DBInstanceIdentifier":"acme-portal-stg-mysql"},
            {"DBInstanceIdentifier":"acme-batch-dev-mysql"}
        ]}"#;
        let r = parse_rds(json, "acme", Env::Dev).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].endpoint.as_deref(), Some("acme.xxx.rds.amazonaws.com"));
        assert!(r[0].secret_arn.as_deref().unwrap().contains("rds!db-1"));
        assert_eq!(r[1].endpoint, None);
        assert_eq!(r[1].secret_arn, None);
        assert!(parse_rds(json, "zzz", Env::Dev).unwrap().is_empty());
    }

    #[test]
    fn secret_parsing() {
        let (u, p) = parse_secret("{\"username\":\"admin\",\"password\":\"s3cret!\"}\n").unwrap();
        assert_eq!(u, "admin");
        assert_eq!(p.as_str(), "s3cret!");
        assert!(parse_secret("not json").is_none());
        assert!(parse_secret("{\"username\":\"admin\"}").is_none());
    }

    #[test]
    fn caller_arn() {
        assert_eq!(
            parse_caller_arn(
                r#"{"UserId":"x","Account":"1","Arn":"arn:aws:sts::1:assumed-role/r/s"}"#
            ),
            Some("arn:aws:sts::1:assumed-role/r/s".to_string())
        );
        assert_eq!(parse_caller_arn("oops"), None);
    }

    #[test]
    fn error_codes() {
        let e = "\nAn error occurred (AccessDeniedException) when calling the GetSecretValue operation: User: x is not authorized";
        assert_eq!(
            parse_error_code(e).as_deref(),
            Some("AccessDeniedException")
        );
        assert_eq!(
            access_denied_code(e).as_deref(),
            Some("AccessDeniedException")
        );
        let e2 = "An error occurred (UnauthorizedOperation) when calling the DescribeInstances operation: You are not authorized";
        assert_eq!(
            access_denied_code(e2).as_deref(),
            Some("UnauthorizedOperation")
        );
        let e3 = "An error occurred (ExpiredToken) when calling the X operation";
        assert_eq!(access_denied_code(e3), None);
        assert_eq!(parse_error_code(e3).as_deref(), Some("ExpiredToken"));
    }

    #[test]
    fn port_opened_and_timeout() {
        assert!(is_port_opened("Port 13306 opened for sessionId x.", 13306));
        assert!(!is_port_opened("Port 13307 opened for sessionId x.", 13306));
        assert!(mentions_timeout("Session timed out"));
        assert!(mentions_timeout("read TIMEOUT"));
        assert!(!mentions_timeout("Exiting session"));
    }

    #[test]
    fn path_augmented() {
        let p = augmented_path(
            Some(Path::new("/opt/x/bin/aws")),
            Some(Path::new("/opt/y/smp")),
        );
        let dirs: Vec<PathBuf> = std::env::split_paths(&p).collect();
        assert_eq!(dirs[0], PathBuf::from("/opt/x/bin"));
        assert_eq!(dirs[1], PathBuf::from("/opt/y"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_override() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("aws");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(resolve_aws(Some(&f)), None);
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(resolve_aws(Some(&f)), Some(f.clone()));
        assert_eq!(resolve_plugin(Some(&dir.path().join("missing"))), None);
    }
}
