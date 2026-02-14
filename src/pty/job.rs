//! Windows Job Object wrapper for PTY process group management.
//!
//! Each PTY session creates a Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
//! When the Job Object handle is closed (or terminate() is called),
//! all processes in the group — including OpenConsole.exe — are terminated.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

pub struct PtyJobObject {
    handle: HANDLE,
}

impl PtyJobObject {
    /// Create a new Job Object with kill-on-close semantics.
    pub fn new() -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let result = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if result == 0 {
                CloseHandle(handle);
                return Err(io::Error::last_os_error());
            }

            Ok(Self { handle })
        }
    }

    /// Assign a process (by PID) to this Job Object.
    pub fn assign_pid(&self, pid: u32) -> io::Result<()> {
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }

            let result = AssignProcessToJobObject(self.handle, process);
            CloseHandle(process);

            if result == 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        }
    }

    /// Explicitly terminate all processes in this Job Object.
    pub fn terminate(&self) -> io::Result<()> {
        unsafe {
            if TerminateJobObject(self.handle, 1) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

impl Drop for PtyJobObject {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

// SAFETY: Job Object handles are process-wide and safe to share across threads.
unsafe impl Send for PtyJobObject {}
unsafe impl Sync for PtyJobObject {}
