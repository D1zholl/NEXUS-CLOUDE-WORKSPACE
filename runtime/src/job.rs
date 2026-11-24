//! Windows Job Objects: контроль дерева процессов и лимита памяти.
//!
//! KILL_ON_JOB_CLOSE гарантирует, что при завершении/падении Runtime не
//! останутся фоновые дочерние процессы. Ошибка назначения не скрывается: она
//! логируется, а timeout/cancel всё равно вызывает taskkill /T.

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    pub struct JobGuard {
        handle: HANDLE,
    }

    // HANDLE используется только через Win32 API и не разделяется между потоками.
    unsafe impl Send for JobGuard {}

    impl JobGuard {
        pub fn terminate(&self) {
            unsafe {
                let _ = TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }

    pub fn assign(child: &tokio::process::Child, memory_limit_bytes: u64) -> Option<JobGuard> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                tracing::warn!("CreateJobObjectW не удался");
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if memory_limit_bytes > 0 {
                info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = memory_limit_bytes as usize;
            }
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                tracing::warn!("SetInformationJobObject не удался");
                let _ = CloseHandle(handle);
                return None;
            }
            let Some(process) = child.raw_handle() else {
                tracing::warn!("не удалось получить handle процесса для Job Object");
                let _ = CloseHandle(handle);
                return None;
            };
            if AssignProcessToJobObject(handle, process as HANDLE) == 0 {
                tracing::warn!(
                    "AssignProcessToJobObject не удался; завершение дерева через taskkill"
                );
                let _ = CloseHandle(handle);
                return None;
            }
            Some(JobGuard { handle })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct JobGuard;

    impl JobGuard {
        pub fn terminate(&self) {}
    }

    pub fn assign(_child: &tokio::process::Child, _memory_limit_bytes: u64) -> Option<JobGuard> {
        None
    }
}

#[allow(unused_imports)]
pub use imp::{JobGuard, assign};
