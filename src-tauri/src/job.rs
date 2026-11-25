//! Windows Job Object so the sidecar tree never outlives the shell.
//!
//! The PyInstaller ONEDIR sidecar runs Python inside its bootloader process, but it spawns
//! `--render-worker` children for every CadQuery render, and in dev a venv `python.exe`
//! launcher runs the real interpreter as a child; killing only the direct child would leave
//! those running. Children inherit the job, and every process in a
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job is terminated when the last job handle closes,
//! which also happens automatically if the shell crashes.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

pub struct KillOnCloseJob(HANDLE);

// The handle is an opaque kernel object reference, usable from any thread.
unsafe impl Send for KillOnCloseJob {}
unsafe impl Sync for KillOnCloseJob {}

impl KillOnCloseJob {
    pub fn new() -> io::Result<Self> {
        // SAFETY: plain Win32 calls; the handle is owned by the returned value.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are valid for the duration of the call.
        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Kills every process still in the job, then closes the handle.
    pub fn terminate(self) {
        // SAFETY: valid job handle; Drop closes it afterwards.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: we own the handle; closing it kills remaining members (KILL_ON_JOB_CLOSE).
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Resumes every thread of a process created with `CREATE_SUSPENDED`.
pub fn resume_process(pid: u32) -> io::Result<()> {
    // SAFETY: Toolhelp snapshot iteration with properly sized THREADENTRY32; every handle
    // opened here is closed before returning.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0usize;
        let mut error = None;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    error = Some(io::Error::last_os_error());
                } else {
                    if ResumeThread(thread) == u32::MAX {
                        error = Some(io::Error::last_os_error());
                    } else {
                        resumed += 1;
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        match (resumed, error) {
            (0, Some(e)) => Err(e),
            (0, None) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no threads found for process {pid}"),
            )),
            _ => Ok(()),
        }
    }
}
