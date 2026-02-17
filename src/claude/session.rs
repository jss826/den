use crate::pty::manager::PtySession;

use super::connection::ConnectionTarget;

/// Claude CLI コマンドを組み立て、PTY で起動
///
/// `is_continuation` が true の場合 `--continue` フラグを追加し、
/// 同一 cwd での前回セッションを継続する。
pub fn spawn_claude_session(
    connection: &ConnectionTarget,
    working_dir: &str,
    prompt: &str,
    is_continuation: bool,
    agent_forwarding: bool,
    cols: u16,
    rows: u16,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    match connection {
        ConnectionTarget::Local => {
            // ローカル: claude を直接起動（cmd.exe/sh を経由しない）
            let mut args = vec![
                "-p".to_string(),
                prompt.to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ];
            if is_continuation {
                args.push("--continue".to_string());
            }
            spawn_command_pty("claude", &args, working_dir, cols, rows)
        }
        ConnectionTarget::Ssh { host } => {
            let mut claude_args = format!(
                "claude -p {} --output-format stream-json --verbose --dangerously-skip-permissions",
                shell_escape_prompt(prompt),
            );
            if is_continuation {
                claude_args.push_str(" --continue");
            }
            let remote_cmd = format!("cd {} && {}", shell_escape(working_dir), claude_args);
            let args = build_ssh_args(host, &remote_cmd, agent_forwarding);
            spawn_command_pty("ssh", &args, working_dir, cols, rows)
        }
    }
}

/// Claude CLI をインタラクティブモードで起動（`-p` なし、プロンプトは stdin から入力）
pub fn spawn_claude_interactive(
    connection: &ConnectionTarget,
    working_dir: &str,
    agent_forwarding: bool,
    cols: u16,
    rows: u16,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    match connection {
        ConnectionTarget::Local => {
            let args = vec![
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ];
            spawn_command_pty("claude", &args, working_dir, cols, rows)
        }
        ConnectionTarget::Ssh { host } => {
            let claude_args =
                "claude --output-format stream-json --verbose --dangerously-skip-permissions";
            let remote_cmd = format!("cd {} && {}", shell_escape(working_dir), claude_args);
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

/// シングルクォートエスケープ（SSH リモートコマンド用）
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn shell_escape_prompt(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_args_structure() {
        let prompt = "hello world";
        let args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--dangerously-skip-permissions".to_string(),
        ];
        assert_eq!(args.len(), 6);
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], prompt);
        assert_eq!(args[2], "--output-format");
        assert_eq!(args[3], "stream-json");
        assert_eq!(args[4], "--verbose");
        assert_eq!(args[5], "--dangerously-skip-permissions");
    }

    #[test]
    fn local_args_continuation() {
        let prompt = "follow up";
        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--dangerously-skip-permissions".to_string(),
        ];
        args.push("--continue".to_string());
        assert_eq!(args.len(), 7);
        assert_eq!(args[6], "--continue");
    }

    #[test]
    fn ssh_args_with_agent_forwarding() {
        let host = "user@remote";
        let prompt = "test prompt";
        let working_dir = "/home/user";
        let claude_args = format!(
            "claude -p {} --output-format stream-json --verbose --dangerously-skip-permissions",
            shell_escape_prompt(prompt),
        );
        let remote_cmd = format!("cd {} && {}", shell_escape(working_dir), claude_args);
        let args = build_ssh_args(host, &remote_cmd, true);
        assert_eq!(args.len(), 6);
        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "-A");
        assert_eq!(args[4], host);
        assert!(args[5].contains("cd '/home/user'"));
        assert!(args[5].contains("claude -p"));
    }

    #[test]
    fn ssh_args_without_agent_forwarding() {
        let host = "user@remote";
        let remote_cmd = "echo hello";
        let args = build_ssh_args(host, remote_cmd, false);
        assert_eq!(args.len(), 5);
        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "-o");
        assert_eq!(args[2], "BatchMode=yes");
        assert_eq!(args[3], host);
        assert!(!args.contains(&"-A".to_string()));
    }

    #[test]
    fn interactive_local_args() {
        let args = vec![
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--dangerously-skip-permissions".to_string(),
        ];
        assert_eq!(args.len(), 4);
        assert!(!args.iter().any(|a| a == "-p"));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn interactive_ssh_args() {
        let host = "user@remote";
        let working_dir = "/home/user";
        let claude_args =
            "claude --output-format stream-json --verbose --dangerously-skip-permissions";
        let remote_cmd = format!("cd {} && {}", shell_escape(working_dir), claude_args);
        let args = build_ssh_args(host, &remote_cmd, true);
        assert_eq!(args.len(), 6);
        assert_eq!(args[1], "-A"); // agent forwarding enabled
        assert!(!remote_cmd.contains("claude -p"));
        assert!(remote_cmd.contains("--output-format stream-json"));
    }

    #[test]
    fn ssh_args_continuation() {
        let prompt = "follow up";
        let mut claude_args = format!(
            "claude -p {} --output-format stream-json --verbose --dangerously-skip-permissions",
            shell_escape_prompt(prompt),
        );
        claude_args.push_str(" --continue");
        assert!(claude_args.contains("--continue"));
        assert!(claude_args.contains("--verbose"));
        assert!(claude_args.contains("--dangerously-skip-permissions"));
    }

    #[test]
    fn shell_escape_basic() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_prompt_with_quotes() {
        let result = shell_escape_prompt("it's a test");
        assert_eq!(result, "'it'\\''s a test'");
    }
}
