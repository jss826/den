use crate::pty::manager::PtySession;

use super::connection::ConnectionTarget;

/// Claude CLI の共通フラグを構築（Local/SSH 共通）
///
/// `skip_permissions` が true の場合 `--dangerously-skip-permissions` を追加。
fn build_claude_flags(skip_permissions: bool) -> Vec<String> {
    let mut flags = vec![
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ];
    if skip_permissions {
        flags.push("--dangerously-skip-permissions".to_string());
    }
    flags
}

/// Claude CLI コマンドを組み立て、PTY で起動
///
/// `is_continuation` が true の場合 `--continue` フラグを追加し、
/// 同一 cwd での前回セッションを継続する。
#[allow(clippy::too_many_arguments)]
pub fn spawn_claude_session(
    connection: &ConnectionTarget,
    working_dir: &str,
    prompt: &str,
    is_continuation: bool,
    agent_forwarding: bool,
    skip_permissions: bool,
    cols: u16,
    rows: u16,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    let mut flags = build_claude_flags(skip_permissions);
    if is_continuation {
        flags.push("--continue".to_string());
    }

    match connection {
        ConnectionTarget::Local => {
            let mut args = vec!["-p".to_string(), prompt.to_string()];
            args.extend(flags);
            spawn_command_pty("claude", &args, working_dir, cols, rows)
        }
        ConnectionTarget::Ssh { host } => {
            let flags_str = flags.join(" ");
            let remote_cmd = format!(
                "cd {} && claude -p {} {}",
                shell_escape(working_dir),
                shell_escape(prompt),
                flags_str,
            );
            let args = build_ssh_args(host, &remote_cmd, agent_forwarding);
            spawn_command_pty("ssh", &args, working_dir, cols, rows)
        }
    }
}

/// Claude CLI をストリーミングモードで起動（`-p --input-format stream-json --output-format stream-json`）
///
/// Claude CLI 2.x では `--output-format` は `--print` モードでのみ有効。
/// `--input-format stream-json` を併用すると、stdin から NDJSON でユーザーメッセージを
/// 受け取り続ける長寿命プロセスとして動作する。
pub fn spawn_claude_interactive(
    connection: &ConnectionTarget,
    working_dir: &str,
    agent_forwarding: bool,
    skip_permissions: bool,
    cols: u16,
    rows: u16,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    let mut flags = build_claude_flags(skip_permissions);
    // stream-json 入力はフラグリストの先頭に追加（--output-format の前に配置）
    flags.splice(
        0..0,
        [
            "-p".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
        ],
    );

    match connection {
        ConnectionTarget::Local => spawn_command_pty("claude", &flags, working_dir, cols, rows),
        ConnectionTarget::Ssh { host } => {
            let flags_str = flags.join(" ");
            let remote_cmd = format!("cd {} && claude {}", shell_escape(working_dir), flags_str);
            let args = build_ssh_args(host, &remote_cmd, agent_forwarding);
            spawn_command_pty("ssh", &args, working_dir, cols, rows)
        }
    }
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

    #[cfg(windows)]
    let pids_before = crate::pty::manager::snapshot_openconsole_pids();

    let pair = pty_system.openpty(size)?;

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(cwd);
    // Den 自体が Claude Code 内から起動された場合、子プロセスの claude CLI が
    // ネストセッション検出で拒否されるのを防ぐ
    cmd.env_remove("CLAUDECODE");

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    #[cfg(windows)]
    let job = crate::pty::manager::create_job_for_pty(&*child, &pids_before);

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

/// SSH コマンドの共通引数を構築（`agent_forwarding` が true の場合のみ `-A` を追加）
fn build_ssh_args(host: &str, remote_cmd: &str, agent_forwarding: bool) -> Vec<String> {
    let mut args = vec!["-t".to_string()];
    if agent_forwarding {
        args.push("-A".to_string());
    }
    args.push("-o".to_string());
    args.push("BatchMode=yes".to_string());
    args.push(host.to_string());
    args.push(remote_cmd.to_string());
    args
}

/// シングルクォートエスケープ（SSH リモートコマンド用、POSIX sh 前提）
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_claude_flags_with_permissions() {
        let flags = build_claude_flags(true);
        assert!(flags.contains(&"--output-format".to_string()));
        assert!(flags.contains(&"stream-json".to_string()));
        assert!(flags.contains(&"--verbose".to_string()));
        assert!(flags.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn build_claude_flags_without_permissions() {
        let flags = build_claude_flags(false);
        assert!(flags.contains(&"--verbose".to_string()));
        assert!(!flags.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn ssh_args_with_agent_forwarding() {
        let args = build_ssh_args("user@remote", "echo hello", true);
        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "-A");
        assert_eq!(args[4], "user@remote");
    }

    #[test]
    fn ssh_args_without_agent_forwarding() {
        let args = build_ssh_args("user@remote", "echo hello", false);
        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "-o");
        assert_eq!(args[2], "BatchMode=yes");
        assert!(!args.contains(&"-A".to_string()));
    }

    #[test]
    fn shell_escape_basic() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_with_quotes() {
        assert_eq!(shell_escape("it's a test"), "'it'\\''s a test'");
    }
}
