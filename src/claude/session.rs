use crate::pty::manager::PtySession;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::connection::ConnectionTarget;

pub type SessionMap = Arc<Mutex<HashMap<String, ClaudeSessionHandle>>>;

/// セッションへの外部参照（PTY writer + 制御チャネル）
#[allow(dead_code)]
pub struct ClaudeSessionHandle {
    pub connection: ConnectionTarget,
    pub working_dir: String,
    pub writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    pub stop_tx: tokio::sync::oneshot::Sender<()>,
    pub task_handle: tokio::task::JoinHandle<()>,
}

pub fn new_session_map() -> SessionMap {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Claude CLI コマンドを組み立て、PTY で起動
pub fn spawn_claude_session(
    connection: &ConnectionTarget,
    working_dir: &str,
    prompt: &str,
    cols: u16,
    rows: u16,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    match connection {
        ConnectionTarget::Local => {
            // ローカル: claude を直接起動（cmd.exe/sh を経由しない）
            let args = vec![
                "-p".to_string(),
                prompt.to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
            ];
            spawn_command_pty("claude", &args, working_dir, cols, rows)
        }
        ConnectionTarget::Ssh { host } => {
            let claude_args = format!(
                "claude -p {} --output-format stream-json --verbose",
                shell_escape_prompt(prompt),
            );
            let remote_cmd = format!("cd {} && {}", shell_escape(working_dir), claude_args);
            let args = vec![
                "-t".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                host.clone(),
                remote_cmd,
            ];
            spawn_command_pty("ssh", &args, working_dir, cols, rows)
        }
    }
}

/// 継続プロンプト送信用：既存セッションの PTY に書き込む
pub fn send_to_session(
    writer: &mut (dyn std::io::Write + Send),
    prompt: &str,
) -> Result<(), std::io::Error> {
    // claude CLI は stdin からの追加プロンプトを受け付ける
    writer.write_all(prompt.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn spawn_command_pty(
    command: &str,
    args: &[String],
    cwd: &str,
    cols: u16,
    rows: u16,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pty_system = native_pty_system();
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pair = pty_system.openpty(size)?;

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(cwd);

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    #[cfg(windows)]
    let job = crate::pty::manager::create_job_for_child(&*child);

    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    Ok(PtySession {
        reader,
        writer,
        child,
        master: pair.master,
        #[cfg(windows)]
        job,
    })
}

/// シングルクォートエスケープ（SSH リモートコマンド用）
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn shell_escape_prompt(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
