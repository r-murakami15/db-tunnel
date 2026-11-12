//! 子プロセスの起動・出力の読み取り・終了（OSごとの違いをここに閉じ込める）。
//!
//! - Unix: 子プロセスを専用のプロセスグループで起動し、停止時はグループに SIGTERM、
//!   3秒待って残っていれば SIGKILL を送る。
//! - Windows: Job Object（JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE）に入れ、ハンドルを閉じると
//!   session-manager-plugin まで終了する。コンソール窓が出ないよう CREATE_NO_WINDOW を付ける。

use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::model::LogStream;

/// 停止時に SIGTERM を送ってから SIGKILL するまでの待ち時間
pub const STOP_GRACE: Duration = Duration::from_secs(3);

/// 1行の最大長（これを超えたら途中で区切る）
const MAX_LINE_BYTES: usize = 64 * 1024;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 子プロセスの出力1行
#[derive(Debug, Clone)]
pub struct OutputLine {
    pub stream: LogStream,
    pub text: String,
}

/// 共通の設定をしたコマンドを作る（stdin なし、終了時に kill、Windows では窓を出さない）。
/// 引数は配列で渡し、シェルを経由しない
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 起動した子プロセス。drop するとプロセス（グループ）ごと強制終了する
pub struct ManagedChild {
    child: Child,
    pid: Option<u32>,
    /// wait() で終了を確認済み
    exited: bool,
    /// グループの後始末まで済んだ
    cleaned: bool,
    #[cfg(windows)]
    job: Option<win::Job>,
}

/// 子プロセスを起動し、stdout / stderr を1行ずつ受け取るチャネルを返す。
/// チャネルは両方の出力がEOFになると閉じる（孫プロセスがパイプを持ち続けると閉じないので、
/// 終了の判定には `wait` を使うこと）
pub fn spawn(mut cmd: Command) -> io::Result<(ManagedChild, mpsc::UnboundedReceiver<OutputLine>)> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn()?;
    let pid = child.id();

    #[cfg(windows)]
    let job = {
        // 失敗しても起動は続ける（停止時は child.kill で直接の子だけは終了できる）
        match child.raw_handle() {
            Some(h) => win::Job::create_and_assign(h).ok(),
            None => None,
        }
    };

    let (tx, rx) = mpsc::unbounded_channel();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(read_lines(out, LogStream::Stdout, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(read_lines(err, LogStream::Stderr, tx));
    }

    Ok((
        ManagedChild {
            child,
            pid,
            exited: false,
            cleaned: false,
            #[cfg(windows)]
            job,
        },
        rx,
    ))
}

/// 出力を読み、`\n` と `\r` を行の区切りとして1行ずつ送る（UTF-8として不正なバイトは置き換える）
async fn read_lines<R: AsyncRead + Unpin>(
    mut reader: R,
    stream: LogStream,
    tx: mpsc::UnboundedSender<OutputLine>,
) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let send = |line: &mut Vec<u8>| -> bool {
        let text = String::from_utf8_lossy(line).into_owned();
        line.clear();
        tx.send(OutputLine { stream, text }).is_ok()
    };
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                // 空行（\r\n の \n 側を含む）は送らない
                if !line.is_empty() && !send(&mut line) {
                    return;
                }
            } else {
                line.push(b);
                if line.len() >= MAX_LINE_BYTES && !send(&mut line) {
                    return;
                }
            }
        }
    }
    if !line.is_empty() {
        send(&mut line);
    }
}

impl ManagedChild {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// 終了を待つ（キャンセルしても安全）
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let st = self.child.wait().await?;
        self.exited = true;
        Ok(st)
    }

    /// 穏やかに停止する。Unix はグループに SIGTERM → 最大3秒待つ → SIGKILL。
    /// Windows は Job Object ごと終了する
    pub async fn stop(&mut self) {
        #[cfg(unix)]
        {
            if !self.exited {
                self.signal_group(nix::sys::signal::Signal::SIGTERM);
                let _ = tokio::time::timeout(STOP_GRACE, self.wait()).await;
            }
        }
        self.kill_now();
        if !self.exited {
            let _ = tokio::time::timeout(Duration::from_secs(5), self.wait()).await;
        }
    }

    /// すぐに強制終了する（グループ / Job ごと）。待たない
    pub fn kill_now(&mut self) {
        #[cfg(unix)]
        {
            if !self.cleaned {
                // リーダーが終了済みでも、グループに残った孫（plugin）を終了させる
                self.signal_group(nix::sys::signal::Signal::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            if let Some(job) = self.job.take() {
                job.terminate();
                // drop でハンドルを閉じる（KILL_ON_JOB_CLOSE）
            }
        }
        if !self.exited {
            let _ = self.child.start_kill();
        }
        self.cleaned = true;
    }

    /// 終了後に、グループに残ったプロセスを片付ける
    pub fn cleanup_after_exit(&mut self) {
        if !self.cleaned {
            self.kill_now();
        }
    }

    #[cfg(unix)]
    fn signal_group(&self, sig: nix::sys::signal::Signal) {
        if let Some(pid) = self.pid {
            if let Ok(pid) = i32::try_from(pid) {
                if pid > 0 {
                    let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), sig);
                }
            }
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if !self.cleaned {
            self.kill_now();
        }
    }
}

/// 終了コードを表示用の文字列にする（シグナルで終了した場合はシグナル番号）
pub fn exit_text(st: &ExitStatus) -> String {
    if let Some(code) = st.code() {
        return code.to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() {
            return format!("シグナル {sig}");
        }
    }
    "不明".to_string()
}

/// 127.0.0.1:<port> に bind できるか（空いているか）
pub fn port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Job Object のハンドル。drop で閉じると、中のプロセスがすべて終了する
    pub struct Job(HANDLE);

    // HANDLE は生ポインタだが、カーネルオブジェクトのハンドルなのでスレッド間で渡してよい
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn create_and_assign(process: RawHandle) -> io::Result<Job> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                if AssignProcessToJobObject(job.0, process as HANDLE) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_check() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!port_available(port));
        drop(l);
        assert!(port_available(port));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reads_lines_with_cr() {
        let mut cmd = command(Path::new("/bin/sh"));
        cmd.args([
            "-c",
            "printf 'a\\r\\nb\\rc\\n'; printf 'err\\n' >&2; printf 'tail'",
        ]);
        let (mut child, mut rx) = spawn(cmd).unwrap();
        let st = child.wait().await.unwrap();
        assert!(st.success());
        let mut out = Vec::new();
        let mut err = Vec::new();
        while let Some(l) = rx.recv().await {
            match l.stream {
                LogStream::Stdout => out.push(l.text),
                LogStream::Stderr => err.push(l.text),
            }
        }
        assert_eq!(out, vec!["a", "b", "c", "tail"]);
        assert_eq!(err, vec!["err"]);
        assert_eq!(exit_text(&st), "0");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_kills_group() {
        let mut cmd = command(Path::new("/bin/sh"));
        // 孫プロセス（sleep）も同じグループに入る
        cmd.args(["-c", "sleep 30 & echo $!; wait"]);
        let (mut child, mut rx) = spawn(cmd).unwrap();
        let grandchild: i32 = rx.recv().await.unwrap().text.trim().parse().unwrap();
        let started = std::time::Instant::now();
        child.stop().await;
        assert!(started.elapsed() < Duration::from_secs(3));
        // 孫が消えるまで少し待つ（ゾンビは死んだとみなす）
        let mut gone = false;
        for _ in 0..50 {
            if !alive(grandchild) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(gone, "grandchild {grandchild} still alive");
    }

    #[cfg(unix)]
    fn alive(pid: i32) -> bool {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
            return false;
        }
        // Linux ではゾンビ（Z）を終了済みとみなす
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => !s
                .rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
                .unwrap_or(false),
            Err(_) => true,
        }
    }
}
