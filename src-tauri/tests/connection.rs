//! ConnectionManager の結合テスト。AWS CLI をダミースクリプト（tests/fixtures/fake-aws.sh）に差し替えて動かす。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use db_tunnel_lib::aws::{self, ToolPaths};
use db_tunnel_lib::connection::{ConnectionManager, EventSink};
use db_tunnel_lib::model::{Env, LogLine, Status, Target, TargetRuntime};

const SECRET: &str = "s3cret!";

#[derive(Default)]
struct Collect {
    statuses: Mutex<Vec<TargetRuntime>>,
    logs: Mutex<Vec<LogLine>>,
}

impl EventSink for Collect {
    fn status(&self, rt: &TargetRuntime) {
        self.statuses.lock().unwrap().push(rt.clone());
    }
    fn log(&self, line: &LogLine) {
        self.logs.lock().unwrap().push(line.clone());
    }
}

impl Collect {
    fn statuses_of(&self, id: &str) -> Vec<Status> {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.id == id)
            .map(|r| r.status)
            .collect()
    }
    fn all_log_text(&self) -> String {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// テストごとの一時ディレクトリ。`aws` はダミースクリプトへのシンボリックリンク
struct Scenario {
    dir: tempfile::TempDir,
}

impl Scenario {
    fn new(env: &str) -> Scenario {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(fixtures().join("fake-aws.sh"), dir.path().join("aws")).unwrap();
        std::fs::write(dir.path().join("scenario.env"), env).unwrap();
        Scenario { dir }
    }

    fn paths(&self) -> ToolPaths {
        ToolPaths {
            aws: Some(self.dir.path().join("aws")),
            plugin: Some(fixtures().join("fake-session-manager-plugin.sh")),
        }
    }

    fn manager(&self) -> (ConnectionManager, Arc<Collect>) {
        let sink = Arc::new(Collect::default());
        let m = ConnectionManager::new(sink.clone(), self.paths());
        (m, sink)
    }

    fn read_pid(&self, name: &str) -> Option<i32> {
        std::fs::read_to_string(self.dir.path().join(name))
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn target(id: &str, name: &str, port: u16) -> Target {
    Target {
        id: id.to_string(),
        name: name.to_string(),
        profile: "test".to_string(),
        env: Env::Dev,
        app: "acme".to_string(),
        local_port: port,
    }
}

async fn wait_status(m: &ConnectionManager, id: &str, want: Status) -> TargetRuntime {
    let start = Instant::now();
    loop {
        let rt = m.runtime(id);
        if rt.status == want {
            return rt;
        }
        if start.elapsed() > Duration::from_secs(20) {
            panic!(
                "timeout waiting for {want:?}; now {:?} message={:?}\nlogs:\n{}",
                rt.status,
                rt.message,
                m.logs(id)
                    .iter()
                    .map(|l| l.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// プロセスが生きているか（Linux ではゾンビを終了済みとみなす）
fn alive(pid: i32) -> bool {
    if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => !s
            .rsplit_once(')')
            .map(|(_, rest)| rest.trim_start().starts_with('Z'))
            .unwrap_or(false),
        Err(_) => true,
    }
}

async fn wait_gone(pid: i32) -> bool {
    for _ in 0..100 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn wait_file(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{} not created", path.display());
}

fn assert_no_secret(m: &ConnectionManager, sink: &Collect, id: &str) {
    let buf = m
        .logs(id)
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!buf.contains(SECRET), "secret in log buffer:\n{buf}");
    let all = sink.all_log_text();
    assert!(!all.contains(SECRET), "secret in emitted logs:\n{all}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn happy_path_connects_and_disconnect_kills_children() {
    let sc = Scenario::new("");
    let (m, sink) = sc.manager();
    let t = target("t1", "Acme Portal", 40101);
    m.connect(t.clone()).await.unwrap();
    assert!(m.is_active("t1"));

    let rt = wait_status(&m, "t1", Status::Connected).await;
    assert!(rt.connected_at.is_some());
    assert_eq!(rt.bastion_id.as_deref(), Some("i-0123456789abcdef0"));
    assert_eq!(
        rt.rds_endpoint.as_deref(),
        Some("acme-portal-dev-mysql.abc.ap-northeast-1.rds.amazonaws.com")
    );
    assert_eq!(rt.db_user.as_deref(), Some("admin"));
    assert!(rt.has_password);
    assert_eq!(
        m.password("t1").as_deref().map(String::as_str),
        Some(SECRET)
    );
    assert_eq!(m.active_count(), 1);
    assert_eq!(m.runtimes().len(), 1);

    // 状態の通知: 準備中 → 接続中（ログインは省略）
    let st = sink.statuses_of("t1");
    assert_eq!(st.first(), Some(&Status::Preparing));
    assert!(!st.contains(&Status::LoggingIn));

    // CLI の呼び出し: プロファイル・リージョン・パラメータの形式
    let calls = sc.calls();
    assert!(calls
        .iter()
        .all(|c| c == "--version" || c.contains("--profile test --region ap-northeast-1")));
    let ssm = calls
        .iter()
        .find(|c| c.starts_with("ssm start-session"))
        .unwrap();
    assert!(ssm.contains("--target i-0123456789abcdef0"));
    assert!(ssm.contains(
        "--parameters host=acme-portal-dev-mysql.abc.ap-northeast-1.rds.amazonaws.com,portNumber=3306,localPortNumber=40101"
    ));
    assert!(calls
        .iter()
        .any(|c| c.contains("Name=tag:Name,Values=*acme*-dev*bastion*")));

    // ログ: Port opened の行が [ssm] で出ている。パスワードは出ていない
    let logs = m.logs("t1");
    assert!(logs.iter().any(|l| l.text.contains("Port 40101 opened")
        && l.source == db_tunnel_lib::model::LogSource::Ssm));
    assert!(logs
        .iter()
        .any(|l| l.text.contains("シークレットを取得しました")));
    assert_no_secret(&m, &sink, "t1");

    // 停止: start-session と孫プロセスが終了し、パスワードが消える
    let session_pid = sc.read_pid("session.pid").unwrap();
    wait_file(&sc.dir.path().join("plugin.pid")).await;
    let plugin_pid = sc.read_pid("plugin.pid").unwrap();
    assert!(alive(session_pid));
    assert!(alive(plugin_pid));

    m.disconnect("t1").await;
    let rt = m.runtime("t1");
    assert_eq!(rt.status, Status::Idle);
    assert!(!rt.has_password);
    assert!(rt.connected_at.is_none());
    assert!(m.password("t1").is_none());
    assert!(!m.is_active("t1"));
    assert_eq!(m.active_count(), 0);
    assert!(wait_gone(session_pid).await, "start-session still alive");
    assert!(wait_gone(plugin_pid).await, "plugin still alive");
    let st = sink.statuses_of("t1");
    assert!(st.contains(&Status::Stopping));
    assert_eq!(st.last(), Some(&Status::Idle));

    // 停止後は同じポートで再接続できる
    m.connect(t).await.unwrap();
    wait_status(&m, "t1", Status::Connected).await;
    m.disconnect_all().await;
    assert_eq!(m.runtime("t1").status, Status::Idle);
    assert_no_secret(&m, &sink, "t1");

    // forget でログと状態を捨てる
    m.forget("t1");
    assert!(m.logs("t1").is_empty());
    assert!(m.runtimes().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_when_not_authenticated() {
    let sc = Scenario::new("STS=login\nLOGIN=ok\n");
    let (m, sink) = sc.manager();
    m.connect(target("t2", "Login", 40102)).await.unwrap();
    wait_status(&m, "t2", Status::Connected).await;
    let st = sink.statuses_of("t2");
    assert!(st.contains(&Status::LoggingIn), "{st:?}");
    let calls = sc.calls();
    assert_eq!(calls.iter().filter(|c| c.starts_with("login")).count(), 1);
    assert!(calls
        .iter()
        .any(|c| c == "login --profile test --region ap-northeast-1"));
    let text = sink.all_log_text();
    assert!(text.contains("プロファイル test で aws login を実行します"));
    assert!(text.contains("ログイン: arn:aws:sts::123456789012:assumed-role/Dev/test"));
    m.disconnect_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_is_serialized_per_profile() {
    let sc = Scenario::new("STS=login\nLOGIN=slow\n");
    let (m, _sink) = sc.manager();
    m.connect(target("a", "A", 40103)).await.unwrap();
    m.connect(target("b", "B", 40104)).await.unwrap();
    wait_status(&m, "a", Status::Connected).await;
    wait_status(&m, "b", Status::Connected).await;
    assert_eq!(m.active_count(), 2);
    let logins = sc.calls().iter().filter(|c| c.starts_with("login")).count();
    assert_eq!(logins, 1, "aws login should run once per profile");
    m.disconnect_all().await;
    assert_eq!(m.active_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_failure_is_error() {
    let sc = Scenario::new("STS=login\nLOGIN=fail\n");
    let (m, _sink) = sc.manager();
    m.connect(target("t3", "LoginFail", 40105)).await.unwrap();
    let rt = wait_status(&m, "t3", Status::Error).await;
    assert!(rt.message.unwrap().contains("ログインできませんでした"));
    assert!(!m.is_active("t3"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bastion_not_found() {
    let sc = Scenario::new("BASTIONS=0\n");
    let (m, _sink) = sc.manager();
    m.connect(target("t4", "NoBastion", 40106)).await.unwrap();
    let rt = wait_status(&m, "t4", Status::Error).await;
    let msg = rt.message.unwrap();
    assert!(msg.contains("踏み台が見つかりません"), "{msg}");
    assert!(msg.contains("*acme*-dev*bastion*"), "{msg}");
    assert!(!rt.has_password);
    // エラーのあとは再接続できる（ポートも空いている）
    assert!(!m.is_active("t4"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bastion_multiple() {
    let sc = Scenario::new("BASTIONS=2\n");
    let (m, _sink) = sc.manager();
    m.connect(target("t5", "TwoBastions", 40107)).await.unwrap();
    let rt = wait_status(&m, "t5", Status::Error).await;
    let msg = rt.message.unwrap();
    assert!(msg.contains("踏み台が複数見つかりました"), "{msg}");
    assert!(msg.contains("acme-portal-dev-bastion(i-0aaa)"), "{msg}");
    assert!(msg.contains("acme-batch-dev-bastion(i-0bbb)"), "{msg}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_denied() {
    let sc = Scenario::new("BASTIONS=denied\n");
    let (m, sink) = sc.manager();
    m.connect(target("t6", "Denied", 40108)).await.unwrap();
    let rt = wait_status(&m, "t6", Status::Error).await;
    assert_eq!(
        rt.message.as_deref(),
        Some("権限がありません（UnauthorizedOperation）")
    );
    // stderr は [ssm] stderr としてログに残る
    assert!(sink.all_log_text().contains("You are not authorized"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rds_errors() {
    let sc = Scenario::new("RDS=none\n");
    let (m, _sink) = sc.manager();
    m.connect(target("r1", "NoRds", 40109)).await.unwrap();
    let msg = wait_status(&m, "r1", Status::Error).await.message.unwrap();
    assert!(msg.contains("RDSが見つかりません"), "{msg}");

    let sc = Scenario::new("RDS=nosecret\n");
    let (m, _sink) = sc.manager();
    m.connect(target("r2", "NoSecret", 40110)).await.unwrap();
    let rt = wait_status(&m, "r2", Status::Error).await;
    assert!(rt.message.unwrap().contains("シークレットが見つかりません"));
    // 表示用にエンドポイントは残る
    assert!(rt.rds_endpoint.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_secret_is_never_logged() {
    let sc = Scenario::new("SECRET=bad\n");
    let (m, sink) = sc.manager();
    m.connect(target("s1", "BadSecret", 40111)).await.unwrap();
    let rt = wait_status(&m, "s1", Status::Error).await;
    assert!(rt
        .message
        .unwrap()
        .contains("接続情報を取得できませんでした"));
    assert!(m.password("s1").is_none());
    assert_no_secret(&m, &sink, "s1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unexpected_exit_is_error() {
    let sc = Scenario::new("SSM=exit\n");
    let (m, sink) = sc.manager();
    m.connect(target("u1", "Exit", 40112)).await.unwrap();
    wait_status(&m, "u1", Status::Connected).await;
    let rt = wait_status(&m, "u1", Status::Error).await;
    assert_eq!(
        rt.message.as_deref(),
        Some("接続が切れました（終了コード 3）。再接続してください")
    );
    assert!(!rt.has_password);
    assert!(m.password("u1").is_none());
    let text = sink.all_log_text();
    assert!(text.contains("start-session が終了しました（終了コード 3）"));
    assert!(text.contains("最後の出力: Connection lost"));
    // エラー後の disconnect は未接続に戻すだけ
    m.disconnect("u1").await;
    assert_eq!(m.runtime("u1").status, Status::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_timeout_message() {
    let sc = Scenario::new("SSM=timeout\n");
    let (m, _sink) = sc.manager();
    m.connect(target("u2", "Timeout", 40113)).await.unwrap();
    let rt = wait_status(&m, "u2", Status::Error).await;
    assert_eq!(
        rt.message.as_deref(),
        Some("セッションがタイムアウトしました。再接続してください")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn port_in_use_is_rejected() {
    let sc = Scenario::new("");
    let (m, _sink) = sc.manager();

    // アプリ外のプロセスが使っている
    let listener = std::net::TcpListener::bind(("127.0.0.1", 40114)).unwrap();
    let err = m.connect(target("p1", "Busy", 40114)).await.unwrap_err();
    assert_eq!(err, "ポート40114は使用中です");
    assert_eq!(m.runtime("p1").status, Status::Idle);
    drop(listener);

    // アプリ内の別の接続先が使っている
    m.connect(target("p2", "Acme Portal", 40115)).await.unwrap();
    let err = m.connect(target("p3", "Other", 40115)).await.unwrap_err();
    assert_eq!(err, "ポート40115は使用中です（Acme Portal）");

    // 同じ接続先の二重接続
    let err = m
        .connect(target("p2", "Acme Portal", 40115))
        .await
        .unwrap_err();
    assert!(err.contains("すでに接続中"), "{err}");

    wait_status(&m, "p2", Status::Connected).await;
    m.disconnect_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_tools_are_rejected() {
    let sc = Scenario::new("");
    let (m, _sink) = sc.manager();
    m.set_tool_paths(ToolPaths {
        aws: Some(sc.dir.path().join("no-such-aws")),
        plugin: sc.paths().plugin,
    });
    let err = m.connect(target("m1", "NoAws", 40116)).await.unwrap_err();
    assert!(err.contains("AWS CLIが見つかりません"), "{err}");
    m.set_tool_paths(ToolPaths {
        aws: sc.paths().aws,
        plugin: Some(sc.dir.path().join("no-such-plugin")),
    });
    let err = m
        .connect(target("m1", "NoPlugin", 40116))
        .await
        .unwrap_err();
    assert!(
        err.contains("Session Manager Pluginが見つかりません"),
        "{err}"
    );
    assert!(!m.is_active("m1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_during_login_cancels() {
    // LOGIN=fail ではなく、ログインが終わらない状態を作るため sts を失敗させ、login を遅くする
    let sc = Scenario::new("STS=fail\nLOGIN=slow\n");
    let (m, _sink) = sc.manager();
    m.connect(target("c1", "Cancel", 40117)).await.unwrap();
    wait_status(&m, "c1", Status::LoggingIn).await;
    m.disconnect("c1").await;
    assert_eq!(m.runtime("c1").status, Status::Idle);
    // 中断されたタスクがあとから状態を変えないこと
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(m.runtime("c1").status, Status::Idle);
    assert!(!m.is_active("c1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_do_not_affect_other_targets() {
    let ok = Scenario::new("");
    let bad = Scenario::new("BASTIONS=0\n");
    let sink = Arc::new(Collect::default());
    let m = ConnectionManager::new(sink.clone(), ok.paths());
    m.connect(target("g1", "Good", 40118)).await.unwrap();
    wait_status(&m, "g1", Status::Connected).await;
    m.set_tool_paths(bad.paths());
    m.connect(target("g2", "Bad", 40119)).await.unwrap();
    wait_status(&m, "g2", Status::Error).await;
    assert_eq!(m.runtime("g1").status, Status::Connected);
    assert_eq!(m.active_count(), 1);
    m.disconnect_all().await;
    assert_eq!(m.runtime("g1").status, Status::Idle);
}

#[tokio::test]
async fn prerequisites_with_fake_tools() {
    let sc = Scenario::new("");
    let p = aws::check_prerequisites(&sc.paths()).await;
    assert!(p.ok);
    assert_eq!(p.aws_cli.version.as_deref(), Some("2.31.3"));
    assert_eq!(
        p.session_manager_plugin.version.as_deref(),
        Some("1.2.650.0")
    );

    let p = aws::check_prerequisites(&ToolPaths {
        aws: Some(sc.dir.path().join("missing")),
        plugin: None,
    })
    .await;
    assert!(!p.ok);
    assert!(!p.aws_cli.found);
    assert!(p.aws_cli.error.is_some());
}
