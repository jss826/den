use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};

/// PTY セッションの生成結果
pub struct PtySession {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
}

pub struct PtyManager;

impl PtyManager {
    /// シェルプロセスを PTY で起動
    pub fn spawn(shell: &str, cols: u16, rows: u16) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(size)?;

        let mut cmd = CommandBuilder::new(shell);
        // Windows の場合、ホームディレクトリで起動
        if let Ok(home) = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
        {
            cmd.cwd(home);
        }

        let child = pair.slave.spawn_command(cmd)?;

        // slave は spawn 後に drop
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok(PtySession {
            reader,
            writer,
            child,
            master: pair.master,
        })
    }
}
