//! 接続管理: 接続先ごとの状態（状態機械）と、接続処理（ログイン → 検索 → シークレット → 転送）。
//!
//! - 状態は `State` に1つのロック（std::sync::Mutex）でまとめ、await をまたいで保持しない。
//! - 接続処理は接続先ごとに tokio タスクで動かす。世代番号（gen）で古いタスクからの更新を無視する。
//! - 停止はキャンセル信号（watch）で知らせ、タスクが子プロセスを止めて終わるのを待つ。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::AbortHandle;
use zeroize::{Zeroize, Zeroizing};

use crate::aws::{self, AwsCli, ToolPaths};
use crate::logbuf::{self, LogBuffer};
use crate::model::{LogLine, LogSource, LogStream, Status, Target, TargetRuntime};
use crate::process::{self, OutputLine};

/// 画面への通知先（Tauri のイベント送信などを差し込む）
pub trait EventSink: Send + Sync + 'static {
    /// 状態が変わるたびに呼ぶ
    fn status(&self, rt: &TargetRuntime);
    /// ログ1行ごとに呼ぶ（マスク済み）
    fn log(&self, line: &LogLine);
}

/// `start-session` と `aws login` 以外の AWS CLI 呼び出しのタイムアウト
pub const CLI_TIMEOUT: Duration = Duration::from_secs(60);
/// `aws login` のタイムアウト（ブラウザでの操作待ち）
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// `start-session` を起動してから "Port N opened" が出るまでの上限
pub const PORT_OPEN_TIMEOUT: Duration = Duration::from_secs(60);
/// 停止時に接続処理の終了を待つ上限（これを超えたらタスクを中断し、drop で強制終了する）
const STOP_WAIT: Duration = Duration::from_secs(10);
/// 子プロセスの終了後、残りの出力を読み切るまで待つ上限
const DRAIN_WAIT: Duration = Duration::from_secs(2);
/// 予期しない終了時に、原因の判定に使う直近の出力行数
const RECENT_LINES: usize = 20;

#[derive(Clone)]
pub struct ConnectionManager {
    inner: Arc<Inner>,
}

struct Inner {
    sink: Arc<dyn EventSink>,
    paths: Mutex<ToolPaths>,
    state: Mutex<State>,
    /// プロファイルごとのログイン用ロック（同じプロファイルでブラウザを2つ開かないため）
    login_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct State {
    runtimes: HashMap<String, TargetRuntime>,
    passwords: HashMap<String, Zeroizing<String>>,
    sessions: HashMap<String, Session>,
    logs: LogBuffer,
    next_gen: u64,
}

/// 実行中の接続処理1つ分
struct Session {
    gen: u64,
    target: Target,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
    abort: Option<AbortHandle>,
}

fn is_active_status(s: Status) -> bool {
    matches!(
        s,
        Status::LoggingIn | Status::Preparing | Status::Connected | Status::Stopping
    )
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl ConnectionManager {
    pub fn new(sink: Arc<dyn EventSink>, paths: ToolPaths) -> Self {
        ConnectionManager {
            inner: Arc::new(Inner {
                sink,
                paths: Mutex::new(paths),
                state: Mutex::new(State::default()),
                login_locks: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn set_tool_paths(&self, paths: ToolPaths) {
        *lock(&self.inner.paths) = paths;
    }

    /// 接続を開始する。入力チェックに失敗したら日本語のメッセージを返す。
    /// 成功したら接続処理をタスクとして起動し、すぐに戻る
    pub async fn connect(&self, target: Target) -> Result<(), String> {
        let paths = lock(&self.inner.paths).clone();
        let aws_path = aws::resolve_aws(paths.aws.as_deref()).ok_or_else(|| {
            "AWS CLIが見つかりません。インストールするか、設定画面でパスを指定してください"
                .to_string()
        })?;
        let plugin_path = aws::resolve_plugin(paths.plugin.as_deref()).ok_or_else(|| {
            "Session Manager Pluginが見つかりません。インストールするか、設定画面でパスを指定してください"
                .to_string()
        })?;

        let id = target.id.clone();
        let port = target.local_port;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (done_tx, done_rx) = watch::channel(false);
        let gen;
        let rt;
        {
            let mut st = lock(&self.inner.state);
            if st
                .runtimes
                .get(&id)
                .is_some_and(|r| is_active_status(r.status))
                || st.sessions.contains_key(&id)
            {
                return Err("この接続先はすでに接続中（または処理中）です".to_string());
            }
            if let Some(other) = st
                .sessions
                .values()
                .find(|s| s.target.id != id && s.target.local_port == port)
            {
                return Err(format!(
                    "ポート{}は使用中です（{}）",
                    port, other.target.name
                ));
            }
            if !process::port_available(port) {
                return Err(format!("ポート{port}は使用中です"));
            }
            st.next_gen += 1;
            gen = st.next_gen;
            st.passwords.remove(&id);
            let mut r = TargetRuntime::idle(&id);
            r.status = Status::Preparing;
            st.runtimes.insert(id.clone(), r.clone());
            st.sessions.insert(
                id.clone(),
                Session {
                    gen,
                    target: target.clone(),
                    cancel: cancel_tx,
                    done: done_rx,
                    abort: None,
                },
            );
            rt = r;
        }
        self.inner.sink.status(&rt);

        let profile = target.profile.clone();
        let run = Run {
            inner: self.inner.clone(),
            id: id.clone(),
            gen,
            target,
            cli: AwsCli {
                aws: aws_path,
                plugin: Some(plugin_path),
                profile,
            },
            cancel: cancel_rx,
        };
        // 接続処理のタスクと、その終了（失敗・パニック）を見届けるタスク
        let cancel = run.cancel.clone();
        let handle = tokio::spawn(run.run());
        let abort = handle.abort_handle();
        let inner = self.inner.clone();
        let sid = id.clone();
        tokio::spawn(async move {
            let failure = match handle.await {
                Ok(Ok(())) | Ok(Err(StepError::Cancelled)) => None,
                Ok(Err(StepError::Fail(msg))) => Some(msg),
                Err(e) if e.is_cancelled() => None,
                Err(_) => Some("内部エラーが発生しました。もう一度接続してください".to_string()),
            };
            // 停止の指示が出ている場合は、停止側（disconnect）が状態を戻す
            if let Some(msg) = failure {
                if !*cancel.borrow() {
                    inner.fail(&sid, gen, &msg);
                }
            }
            let _ = done_tx.send(true);
        });
        {
            let mut st = lock(&self.inner.state);
            if let Some(s) = st.sessions.get_mut(&id) {
                if s.gen == gen {
                    s.abort = Some(abort);
                }
            }
        }
        Ok(())
    }

    /// 接続を停止する（start-session とその子プロセスを終了し、パスワードを消して未接続に戻す）
    pub async fn disconnect(&self, id: &str) {
        let (gen, mut done, abort) = {
            let mut st = lock(&self.inner.state);
            let Some(s) = st.sessions.get(id) else {
                // 処理中でなければ、エラー表示などを未接続に戻すだけ
                let rt = st.runtimes.get_mut(id).map(|r| {
                    if r.status != Status::Idle {
                        r.status = Status::Idle;
                        r.message = None;
                        r.connected_at = None;
                    }
                    r.has_password = false;
                    r.clone()
                });
                st.passwords.remove(id);
                drop(st);
                if let Some(rt) = rt {
                    self.inner.sink.status(&rt);
                }
                return;
            };
            let gen = s.gen;
            let done = s.done.clone();
            let abort = s.abort.clone();
            let _ = s.cancel.send(true);
            let rt = st.runtimes.get_mut(id).map(|r| {
                r.status = Status::Stopping;
                r.message = None;
                r.clone()
            });
            drop(st);
            if let Some(rt) = rt {
                self.inner.sink.status(&rt);
            }
            (gen, done, abort)
        };
        self.inner
            .log(id, LogSource::App, LogStream::Stdout, "停止しています…");

        let finished = tokio::time::timeout(STOP_WAIT, wait_flag(&mut done))
            .await
            .is_ok();
        if !finished {
            if let Some(a) = abort {
                // タスクを中断すると、子プロセスは drop で強制終了される
                a.abort();
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), wait_flag(&mut done)).await;
        }

        let rt = {
            let mut st = lock(&self.inner.state);
            if st.sessions.get(id).is_some_and(|s| s.gen == gen) {
                st.sessions.remove(id);
            }
            st.passwords.remove(id);
            let r = st
                .runtimes
                .entry(id.to_string())
                .or_insert_with(|| TargetRuntime::idle(id));
            r.status = Status::Idle;
            r.message = None;
            r.connected_at = None;
            r.has_password = false;
            r.clone()
        };
        self.inner.sink.status(&rt);
        self.inner
            .log(id, LogSource::App, LogStream::Stdout, "停止しました");
    }

    /// すべての接続を停止する（アプリ終了時）
    pub async fn disconnect_all(&self) {
        let ids: Vec<String> = {
            let st = lock(&self.inner.state);
            st.sessions.keys().cloned().collect()
        };
        // 停止はそれぞれ数秒かかるため、同時に進める
        let mut set = tokio::task::JoinSet::new();
        for id in ids {
            let m = self.clone();
            set.spawn(async move { m.disconnect(&id).await });
        }
        while set.join_next().await.is_some() {}
    }

    pub fn runtime(&self, id: &str) -> TargetRuntime {
        lock(&self.inner.state)
            .runtimes
            .get(id)
            .cloned()
            .unwrap_or_else(|| TargetRuntime::idle(id))
    }

    pub fn runtimes(&self) -> Vec<TargetRuntime> {
        let mut v: Vec<TargetRuntime> =
            lock(&self.inner.state).runtimes.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn password(&self, id: &str) -> Option<Zeroizing<String>> {
        lock(&self.inner.state).passwords.get(id).cloned()
    }

    /// ログイン待ち・準備中・接続中・停止処理中なら true
    pub fn is_active(&self, id: &str) -> bool {
        let st = lock(&self.inner.state);
        st.sessions.contains_key(id)
            || st
                .runtimes
                .get(id)
                .is_some_and(|r| is_active_status(r.status))
    }

    pub fn active_count(&self) -> usize {
        let st = lock(&self.inner.state);
        st.runtimes
            .values()
            .filter(|r| is_active_status(r.status) || st.sessions.contains_key(&r.id))
            .count()
    }

    pub fn logs(&self, id: &str) -> Vec<LogLine> {
        lock(&self.inner.state).logs.get(id)
    }

    pub fn clear_logs(&self, id: &str) {
        lock(&self.inner.state).logs.clear(id);
    }

    /// 接続先が削除されたとき、ログと状態を捨てる（処理中なら何もしない）
    pub fn forget(&self, id: &str) {
        let mut st = lock(&self.inner.state);
        if st.sessions.contains_key(id)
            || st
                .runtimes
                .get(id)
                .is_some_and(|r| is_active_status(r.status))
        {
            return;
        }
        st.runtimes.remove(id);
        st.passwords.remove(id);
        st.logs.remove(id);
    }
}

/// フラグが true になるのを待つ（送信側がなくなった場合も true とみなす）
async fn wait_flag(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|c| *c).await;
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // ほかのスレッドがパニックしても状態は使い続ける
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    /// ログ1行を記録して通知する（既知のパスワードと JSON 中の秘密情報をマスク）
    fn log(&self, id: &str, source: LogSource, stream: LogStream, text: &str) {
        let line = {
            let mut st = lock(&self.state);
            let known: Vec<&str> = st.passwords.values().map(|p| p.as_str()).collect();
            let masked = logbuf::mask_secrets(text, &known);
            let line = logbuf::make_line(id, source, stream, &masked);
            st.logs.push(line.clone());
            line
        };
        self.sink.log(&line);
    }

    /// 世代が一致するときだけ実行時状態を更新して通知する。一致しなければ false
    fn update(&self, id: &str, gen: u64, f: impl FnOnce(&mut TargetRuntime)) -> bool {
        let rt = {
            let mut st = lock(&self.state);
            if !st.sessions.get(id).is_some_and(|s| s.gen == gen) {
                return false;
            }
            let r = st
                .runtimes
                .entry(id.to_string())
                .or_insert_with(|| TargetRuntime::idle(id));
            f(r);
            r.clone()
        };
        self.sink.status(&rt);
        true
    }

    /// 接続処理の失敗: エラーにして、パスワードを消し、セッションを外す
    fn fail(&self, id: &str, gen: u64, msg: &str) {
        let rt = {
            let mut st = lock(&self.state);
            if !st.sessions.get(id).is_some_and(|s| s.gen == gen) {
                return;
            }
            st.sessions.remove(id);
            st.passwords.remove(id);
            let r = st
                .runtimes
                .entry(id.to_string())
                .or_insert_with(|| TargetRuntime::idle(id));
            r.status = Status::Error;
            r.message = Some(msg.to_string());
            r.connected_at = None;
            r.has_password = false;
            r.clone()
        };
        self.log(
            id,
            LogSource::App,
            LogStream::Stderr,
            &format!("エラー: {msg}"),
        );
        self.sink.status(&rt);
    }

    fn login_lock(&self, profile: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.login_locks)
            .entry(profile.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

// ---------------------------------------------------------------------------
// 接続処理
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum StepError {
    /// 停止の指示で中断した
    Cancelled,
    /// 失敗（画面に出すメッセージ）
    Fail(String),
}

/// 子プロセス1回分の結果
struct CliOut {
    success: bool,
    exit: String,
    stdout: Zeroizing<String>,
    stderr: String,
}

enum CliErr {
    Cancelled,
    Timeout,
    Spawn(String),
}

/// stdout の扱い
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutMode {
    /// 集めるだけ（JSON など。ログには出さない）
    Collect,
    /// 集めて、ログにも出す
    CollectAndLog,
    /// 秘密情報。ログには一切出さない
    Secret,
}

struct Run {
    inner: Arc<Inner>,
    id: String,
    gen: u64,
    target: Target,
    cli: AwsCli,
    cancel: watch::Receiver<bool>,
}

impl Run {
    fn app(&self, text: &str) {
        self.inner
            .log(&self.id, LogSource::App, LogStream::Stdout, text);
    }

    fn out(&self, stream: LogStream, text: &str) {
        self.inner.log(&self.id, LogSource::Ssm, stream, text);
    }

    fn set(&self, f: impl FnOnce(&mut TargetRuntime)) -> Result<(), StepError> {
        if self.inner.update(&self.id, self.gen, f) {
            Ok(())
        } else {
            Err(StepError::Cancelled)
        }
    }

    fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    async fn run(self) -> Result<(), StepError> {
        let t = self.target.clone();
        self.app(&format!(
            "接続を開始します: {}（プロファイル {} / {} / {} / localhost:{}）",
            t.name,
            t.profile,
            t.env.as_str(),
            t.app,
            t.local_port
        ));

        // 1. ツールの確認（connect で確認済みだが、念のためパスをログに出す）
        self.app(&format!("AWS CLI: {}", self.cli.aws.display()));
        if let Some(p) = &self.cli.plugin {
            self.app(&format!("Session Manager Plugin: {}", p.display()));
        }

        // 2. ログイン
        let arn = self.ensure_login().await?;

        // 3. ユーザー確認
        self.app(&format!("ログイン: {arn}"));
        self.set(|r| {
            r.status = Status::Preparing;
            r.message = None;
        })?;

        // 4. 踏み台
        let bastion = self.find_bastion().await?;
        // 5. RDS
        let (endpoint, secret_arn) = self.find_rds().await?;
        // 6. シークレット
        self.fetch_secret(&secret_arn).await?;
        // 7. 転送
        self.port_forward(&bastion, &endpoint).await
    }

    /// AWS CLI を実行し、終了まで待つ（タイムアウト・停止の指示に対応）
    async fn run_cli(
        &self,
        args: &[&str],
        timeout: Duration,
        mode: OutMode,
    ) -> Result<CliOut, CliErr> {
        if self.cancelled() {
            return Err(CliErr::Cancelled);
        }
        let cmd = self.cli.command(args);
        let (mut child, mut rx) = process::spawn(cmd).map_err(|e| CliErr::Spawn(e.to_string()))?;
        let mut stdout = Zeroizing::new(String::new());
        let mut stderr = String::new();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut cancel = self.cancel.clone();
        let mut rx_open = true;

        let status = loop {
            tokio::select! {
                biased;
                _ = wait_flag(&mut cancel) => {
                    child.kill_now();
                    return Err(CliErr::Cancelled);
                }
                _ = tokio::time::sleep_until(deadline) => {
                    child.stop().await;
                    return Err(CliErr::Timeout);
                }
                line = rx.recv(), if rx_open => match line {
                    Some(l) => self.handle_line(l, mode, &mut stdout, &mut stderr),
                    None => rx_open = false,
                },
                st = child.wait() => break st,
            }
        };
        // 残りの出力を読み切る（孫プロセスがパイプを持ち続ける場合に備えて上限つき）
        if rx_open {
            let _ = tokio::time::timeout(DRAIN_WAIT, async {
                while let Some(l) = rx.recv().await {
                    self.handle_line(l, mode, &mut stdout, &mut stderr);
                }
            })
            .await;
        }
        child.cleanup_after_exit();
        match status {
            Ok(st) => Ok(CliOut {
                success: st.success(),
                exit: process::exit_text(&st),
                stdout,
                stderr,
            }),
            Err(e) => Err(CliErr::Spawn(e.to_string())),
        }
    }

    fn handle_line(
        &self,
        mut l: OutputLine,
        mode: OutMode,
        stdout: &mut Zeroizing<String>,
        stderr: &mut String,
    ) {
        match l.stream {
            LogStream::Stdout => {
                stdout.push_str(&l.text);
                stdout.push('\n');
                if mode == OutMode::CollectAndLog {
                    self.out(LogStream::Stdout, &l.text);
                }
                if mode == OutMode::Secret {
                    l.text.zeroize();
                }
            }
            LogStream::Stderr => {
                // stderr はマスクしてログに出す（シークレット取得時も同じ）
                self.out(LogStream::Stderr, &l.text);
                if stderr.len() < 64 * 1024 {
                    stderr.push_str(&l.text);
                    stderr.push('\n');
                }
            }
        }
    }

    /// CLI の失敗を画面用のメッセージにする
    fn cli_error(&self, label: &str, err: CliErr) -> StepError {
        match err {
            CliErr::Cancelled => StepError::Cancelled,
            CliErr::Timeout => StepError::Fail(format!(
                "{label}がタイムアウトしました（{}秒）。ネットワークを確認して、もう一度接続してください",
                CLI_TIMEOUT.as_secs()
            )),
            CliErr::Spawn(e) => {
                self.app(&format!("AWS CLIを起動できませんでした: {e}"));
                StepError::Fail("AWS CLIを起動できませんでした。設定画面でパスを確認してください".to_string())
            }
        }
    }

    /// 終了コードが0以外のときのメッセージ（権限不足を優先）
    fn exit_error(&self, label: &str, out: &CliOut) -> StepError {
        if let Some(code) = aws::access_denied_code(&out.stderr) {
            return StepError::Fail(format!("権限がありません（{code}）"));
        }
        let detail = aws::parse_error_code(&out.stderr)
            .unwrap_or_else(|| format!("終了コード {}", out.exit));
        StepError::Fail(format!(
            "{label}に失敗しました（{detail}）。詳細はログを確認してください"
        ))
    }

    /// get-caller-identity。成功したら Arn
    async fn caller_identity(&self) -> Result<Option<String>, StepError> {
        match self
            .run_cli(
                &["sts", "get-caller-identity", "--output", "json"],
                CLI_TIMEOUT,
                OutMode::Collect,
            )
            .await
        {
            Ok(out) if out.success => Ok(Some(
                aws::parse_caller_arn(&out.stdout).unwrap_or_else(|| "（不明）".to_string()),
            )),
            Ok(_) => Ok(None),
            Err(e) => Err(self.cli_error("ログイン状態の確認", e)),
        }
    }

    async fn ensure_login(&self) -> Result<String, StepError> {
        self.app("ログイン状態を確認しています…");
        if let Some(arn) = self.caller_identity().await? {
            return Ok(arn);
        }
        self.set(|r| {
            r.status = Status::LoggingIn;
            r.message = None;
        })?;
        let profile = self.target.profile.clone();
        let login_lock = self.inner.login_lock(&profile);
        let mut cancel = self.cancel.clone();
        let _guard = tokio::select! {
            biased;
            _ = wait_flag(&mut cancel) => return Err(StepError::Cancelled),
            g = login_lock.lock() => g,
        };
        // 待っている間に別の接続先がログインを済ませたかもしれない
        if let Some(arn) = self.caller_identity().await? {
            return Ok(arn);
        }

        self.app(&format!("プロファイル {profile} で aws login を実行します"));
        self.app("ブラウザでアカウントを選択してください…");
        let out = match self
            .run_cli(&["login"], LOGIN_TIMEOUT, OutMode::CollectAndLog)
            .await
        {
            Ok(out) => out,
            Err(CliErr::Cancelled) => return Err(StepError::Cancelled),
            Err(CliErr::Timeout) => {
                return Err(StepError::Fail(
                    "ログインがタイムアウトしました。もう一度接続してください".to_string(),
                ))
            }
            Err(e) => return Err(self.cli_error("ログイン", e)),
        };
        if !out.success {
            self.app(&format!(
                "aws login が終了しました（終了コード {}）",
                out.exit
            ));
            return Err(StepError::Fail(
                "ログインできませんでした。もう一度接続してください".to_string(),
            ));
        }
        match self.caller_identity().await? {
            Some(arn) => Ok(arn),
            None => Err(StepError::Fail(
                "ログインできませんでした。もう一度接続してください".to_string(),
            )),
        }
    }

    async fn find_bastion(&self) -> Result<String, StepError> {
        let filter = aws::bastion_name_filter(&self.target.app, self.target.env);
        self.app(&format!("踏み台を検索しています（tag:Name = {filter}）…"));
        let name_filter = format!("Name=tag:Name,Values={filter}");
        let label = "踏み台の検索";
        let out = self
            .run_cli(
                &[
                    "ec2",
                    "describe-instances",
                    "--filters",
                    &name_filter,
                    "Name=instance-state-name,Values=running",
                    "--query",
                    "Reservations[].Instances[].{id:InstanceId,name:Tags[?Key=='Name']|[0].Value}",
                    "--output",
                    "json",
                ],
                CLI_TIMEOUT,
                OutMode::Collect,
            )
            .await
            .map_err(|e| self.cli_error(label, e))?;
        if !out.success {
            return Err(self.exit_error(label, &out));
        }
        let list = aws::parse_bastions(&out.stdout).map_err(|e| {
            self.app(&format!("踏み台の検索結果を読めませんでした: {e}"));
            StepError::Fail(
                "踏み台の検索結果を読めませんでした。詳細はログを確認してください".to_string(),
            )
        })?;
        match list.as_slice() {
            [] => Err(StepError::Fail(format!(
                "踏み台が見つかりません。アプリ名と環境を確認してください（条件: tag:Name = {filter}）"
            ))),
            [b] => {
                let id = b.id.clone();
                let name = b.name.clone();
                self.set(|r| r.bastion_id = Some(id.clone()))?;
                self.app(&format!("踏み台を検索しました: {id}（{name}）"));
                Ok(id)
            }
            many => {
                let names: Vec<String> = many
                    .iter()
                    .map(|b| format!("{}({})", b.name, b.id))
                    .collect();
                Err(StepError::Fail(format!(
                    "踏み台が複数見つかりました: {}",
                    names.join(", ")
                )))
            }
        }
    }

    async fn find_rds(&self) -> Result<(String, String), StepError> {
        let cond = aws::rds_condition(&self.target.app, self.target.env);
        self.app(&format!("RDSを検索しています（{cond}）…"));
        let label = "RDSの検索";
        let out = self
            .run_cli(
                &["rds", "describe-db-instances", "--output", "json"],
                CLI_TIMEOUT,
                OutMode::Collect,
            )
            .await
            .map_err(|e| self.cli_error(label, e))?;
        if !out.success {
            return Err(self.exit_error(label, &out));
        }
        let list = aws::parse_rds(&out.stdout, &self.target.app, self.target.env).map_err(|e| {
            self.app(&format!("RDSの検索結果を読めませんでした: {e}"));
            StepError::Fail(
                "RDSの検索結果を読めませんでした。詳細はログを確認してください".to_string(),
            )
        })?;
        let db = match list.as_slice() {
            [] => {
                return Err(StepError::Fail(format!(
                    "RDSが見つかりません。アプリ名と環境を確認してください（条件: {cond}）"
                )))
            }
            [db] => db.clone(),
            many => {
                let names: Vec<&str> = many.iter().map(|d| d.identifier.as_str()).collect();
                return Err(StepError::Fail(format!(
                    "RDSが複数見つかりました: {}",
                    names.join(", ")
                )));
            }
        };
        let Some(endpoint) = db.endpoint.clone() else {
            return Err(StepError::Fail(format!(
                "RDS {} のエンドポイントを取得できませんでした。RDSの状態を確認してください",
                db.identifier
            )));
        };
        let ep = endpoint.clone();
        self.set(|r| r.rds_endpoint = Some(ep))?;
        self.app(&format!(
            "RDSを検索しました: {endpoint}（{}）",
            db.identifier
        ));
        let Some(secret_arn) = db.secret_arn.clone() else {
            return Err(StepError::Fail(format!(
                "RDS {} のシークレットが見つかりません（Secrets Managerでパスワードを管理していません）",
                db.identifier
            )));
        };
        Ok((endpoint, secret_arn))
    }

    async fn fetch_secret(&self, secret_arn: &str) -> Result<(), StepError> {
        self.app("シークレットを取得しています…");
        let label = "シークレットの取得";
        // stdout は秘密情報なので、エラー時も含めてログに出さない
        let out = self
            .run_cli(
                &[
                    "secretsmanager",
                    "get-secret-value",
                    "--secret-id",
                    secret_arn,
                    "--query",
                    "SecretString",
                    "--output",
                    "text",
                ],
                CLI_TIMEOUT,
                OutMode::Secret,
            )
            .await
            .map_err(|e| self.cli_error(label, e))?;
        if !out.success {
            if let Some(code) = aws::access_denied_code(&out.stderr) {
                return Err(StepError::Fail(format!("権限がありません（{code}）")));
            }
            return Err(StepError::Fail(
                "接続情報を取得できませんでした。詳細はログを確認してください".to_string(),
            ));
        }
        let Some((user, password)) = aws::parse_secret(&out.stdout) else {
            self.app("シークレットの形式が想定と違います（username / password を含むJSONではありません）");
            return Err(StepError::Fail(
                "接続情報を取得できませんでした。シークレットの形式を確認してください".to_string(),
            ));
        };
        drop(out);
        {
            let mut st = lock(&self.inner.state);
            if !st.sessions.get(&self.id).is_some_and(|s| s.gen == self.gen) {
                return Err(StepError::Cancelled);
            }
            st.passwords.insert(self.id.clone(), password);
        }
        self.set(|r| {
            r.db_user = Some(user);
            r.has_password = true;
        })?;
        self.app("シークレットを取得しました（パスワードはログに出力しません）");
        Ok(())
    }

    async fn port_forward(&self, bastion: &str, endpoint: &str) -> Result<(), StepError> {
        let port = self.target.local_port;
        self.app(&format!(
            "ポートフォワーディングを開始します（localhost:{port} → RDS:3306）"
        ));
        if self.cancelled() {
            return Err(StepError::Cancelled);
        }
        let params = format!("host={endpoint},portNumber=3306,localPortNumber={port}");
        let cmd = self.cli.command(&[
            "ssm",
            "start-session",
            "--target",
            bastion,
            "--document-name",
            "AWS-StartPortForwardingSessionToRemoteHost",
            "--parameters",
            &params,
        ]);
        let (mut child, mut rx) = process::spawn(cmd).map_err(|e| {
            self.app(&format!("start-session を起動できませんでした: {e}"));
            StepError::Fail(
                "ポートフォワーディングを開始できませんでした。AWS CLIのパスを確認してください"
                    .to_string(),
            )
        })?;
        if let Some(pid) = child.pid() {
            self.app(&format!("start-session を起動しました（PID {pid}）"));
        }

        let mut cancel = self.cancel.clone();
        let mut recent: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        let mut stderr_all = String::new();
        let mut connected = false;
        let mut rx_open = true;
        let open_deadline = tokio::time::Instant::now() + PORT_OPEN_TIMEOUT;

        let status = loop {
            tokio::select! {
                biased;
                _ = wait_flag(&mut cancel) => {
                    self.app("start-session を終了しています…");
                    child.stop().await;
                    return Err(StepError::Cancelled);
                }
                _ = tokio::time::sleep_until(open_deadline), if !connected => {
                    child.stop().await;
                    return Err(StepError::Fail(format!(
                        "ポートフォワーディングを開始できませんでした（{}秒以内に応答がありません）。もう一度接続してください",
                        PORT_OPEN_TIMEOUT.as_secs()
                    )));
                }
                line = rx.recv(), if rx_open => match line {
                    Some(l) => {
                        self.out(l.stream, &l.text);
                        if l.stream == LogStream::Stderr && stderr_all.len() < 64 * 1024 {
                            stderr_all.push_str(&l.text);
                            stderr_all.push('\n');
                        }
                        if !connected && aws::is_port_opened(&l.text, port) {
                            connected = true;
                            self.set(|r| {
                                r.status = Status::Connected;
                                r.message = None;
                                r.connected_at = Some(now_ms());
                            })?;
                            self.app(&format!("接続しました（127.0.0.1:{port}）"));
                        }
                        if recent.len() >= RECENT_LINES {
                            recent.pop_front();
                        }
                        recent.push_back(l.text);
                    }
                    None => rx_open = false,
                },
                st = child.wait() => break st,
            }
        };
        // 残りの出力
        if rx_open {
            let _ = tokio::time::timeout(DRAIN_WAIT, async {
                while let Some(l) = rx.recv().await {
                    self.out(l.stream, &l.text);
                    if l.stream == LogStream::Stderr {
                        stderr_all.push_str(&l.text);
                        stderr_all.push('\n');
                    }
                    if recent.len() >= RECENT_LINES {
                        recent.pop_front();
                    }
                    recent.push_back(l.text);
                }
            })
            .await;
        }
        child.cleanup_after_exit();
        if self.cancelled() {
            return Err(StepError::Cancelled);
        }

        let exit = match &status {
            Ok(st) => process::exit_text(st),
            Err(_) => "不明".to_string(),
        };
        self.app(&format!(
            "start-session が終了しました（終了コード {exit}）"
        ));
        if let Some(last) = recent.back() {
            let masked = {
                let st = lock(&self.inner.state);
                let known: Vec<&str> = st.passwords.values().map(|p| p.as_str()).collect();
                logbuf::mask_secrets(last, &known)
            };
            self.app(&format!("最後の出力: {masked}"));
        }
        if let Some(code) = aws::access_denied_code(&stderr_all) {
            return Err(StepError::Fail(format!("権限がありません（{code}）")));
        }
        let recent_text: Vec<&str> = recent.iter().map(String::as_str).collect();
        if aws::mentions_timeout(&recent_text.join("\n")) {
            return Err(StepError::Fail(
                "セッションがタイムアウトしました。再接続してください".to_string(),
            ));
        }
        if connected {
            Err(StepError::Fail(format!(
                "接続が切れました（終了コード {exit}）。再接続してください"
            )))
        } else {
            Err(StepError::Fail(format!(
                "ポートフォワーディングを開始できませんでした（終了コード {exit}）。詳細はログを確認してください"
            )))
        }
    }
}
