use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};

/// PTY セッションの生成結果
pub struct PtySession {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    #[cfg(windows)]
    pub job: Option<super::job::PtyJobObject>,
}

pub struct PtyManager;

impl PtyManager {
    /// シェルプロセスを PTY で起動
    pub fn spawn(
        shell: &str,
        cols: u16,
        rows: u16,
    ) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
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
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            cmd.cwd(home);
        }

        let child = pair.slave.spawn_command(cmd)?;

        // slave は spawn 後に drop
        drop(pair.slave);

        // Windows: Job Object でプロセスグループ管理
        #[cfg(windows)]
        let job = create_job_for_child(&*child);

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
}

/// Create a Job Object and assign the child process to it.
/// Returns None (with a warning log) if anything fails.
#[cfg(windows)]
pub fn create_job_for_child(child: &dyn portable_pty::Child) -> Option<super::job::PtyJobObject> {
    let pid = child.process_id()?;

    let job = match super::job::PtyJobObject::new() {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("Failed to create Job Object: {e}");
            return None;
        }
    };

    if let Err(e) = job.assign_pid(pid) {
        tracing::warn!("Failed to assign PID {pid} to Job Object: {e}");
        return None;
    }

    tracing::debug!("Assigned PID {pid} to Job Object");
    Some(job)
}
