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

        // ConPTY 作成前の OpenConsole PID をスナップショット
        #[cfg(windows)]
        let pids_before = snapshot_openconsole_pids();

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
        let job = create_job_for_pty(&*child, &pids_before);

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

/// Create a Job Object and assign both the child and OpenConsole processes.
/// OpenConsole (ConPTY host) の PID は openpty() 前後のスナップショット差分で特定する。
#[cfg(windows)]
pub fn create_job_for_pty(
    child: &dyn portable_pty::Child,
    pids_before: &std::collections::HashSet<u32>,
) -> Option<super::job::PtyJobObject> {
    let pid = child.process_id()?;

    let job = match super::job::PtyJobObject::new() {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("Failed to create Job Object: {e}");
            return None;
        }
    };

    if let Err(e) = job.assign_pid(pid) {
        tracing::warn!("Failed to assign child PID {pid} to Job Object: {e}");
        return None;
    }
    tracing::debug!("Assigned child PID {pid} to Job Object");

    // openpty() で新たに起動した OpenConsole を Job Object に追加
    let pids_after = snapshot_openconsole_pids();
    for &conhost_pid in pids_after.difference(pids_before) {
        match job.assign_pid(conhost_pid) {
            Ok(()) => {
                tracing::debug!("Assigned OpenConsole PID {conhost_pid} to Job Object");
            }
            Err(e) => {
                tracing::debug!("Failed to assign OpenConsole PID {conhost_pid}: {e}");
            }
        }
    }

    Some(job)
}

/// 現在実行中の OpenConsole.exe プロセスの PID セットを返す
#[cfg(windows)]
pub fn snapshot_openconsole_pids() -> std::collections::HashSet<u32> {
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
    };

    let mut pids = HashSet::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() {
            return pids;
        }

        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;

        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                // szExeFile は [i8; 260] (null-terminated)
                let name_bytes: Vec<u8> = entry
                    .szExeFile
                    .iter()
                    .take_while(|&&c| c != 0)
                    .map(|&c| c as u8)
                    .collect();
                if let Ok(name) = std::str::from_utf8(&name_bytes)
                    && name.eq_ignore_ascii_case("OpenConsole.exe")
                {
                    pids.insert(entry.th32ProcessID);
                }

                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
    }

    pids
}
