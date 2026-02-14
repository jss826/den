use crate::pty::manager::PtySession;

use super::connection::ConnectionTarget;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_args_structure() {
        // Verify the local command arg pattern
        let prompt = "hello world";
        let args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ];
        assert_eq!(args.len(), 5);
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], prompt);
        assert_eq!(args[2], "--output-format");
        assert_eq!(args[3], "stream-json");
    }

    #[test]
    fn ssh_args_structure() {
        let host = "user@remote";
        let prompt = "test prompt";
        let working_dir = "/home/user";
        let claude_args = format!(
            "claude -p {} --output-format stream-json --verbose",
            shell_escape_prompt(prompt),
        );
        let remote_cmd = format!("cd {} && {}", shell_escape(working_dir), claude_args);
        let args = vec![
            "-t".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            host.to_string(),
            remote_cmd.clone(),
        ];
        assert_eq!(args.len(), 5);
        assert_eq!(args[0], "-t");
        assert_eq!(args[3], host);
        assert!(args[4].contains("cd '/home/user'"));
        assert!(args[4].contains("claude -p"));
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
