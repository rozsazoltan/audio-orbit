use std::path::PathBuf;

#[cfg(windows)]
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn open_request_dir() -> Option<PathBuf> {
    crate::config::app_data_dir().map(|directory| directory.join("open-requests"))
}

#[cfg(windows)]
fn queue_open_request(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    let directory = open_request_dir()
        .ok_or_else(|| "failed to resolve Audio Orbit data directory".to_owned())?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create open-request directory: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let path = directory.join(format!("{}-{timestamp}.json", std::process::id()));
    let contents = serde_json::to_vec(paths)
        .map_err(|error| format!("failed to serialize open request: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("failed to queue open request {}: {error}", path.display()))
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    const MUTEX_NAME: &str = "Local\\AudioOrbit.RozsaZoltan.SingleInstance";

    pub struct SingleInstanceGuard {
        handle: HANDLE,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    CloseHandle(self.handle);
                }
            }
        }
    }

    pub fn acquire(open_paths: &[PathBuf]) -> Result<Option<SingleInstanceGuard>, String> {
        let mut name = MUTEX_NAME.encode_utf16().collect::<Vec<u16>>();
        name.push(0);

        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
        if handle.is_null() {
            return Err("failed to create the Audio Orbit single-instance mutex".to_owned());
        }

        let last_error = unsafe { GetLastError() };
        if last_error == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            queue_open_request(open_paths)?;
            return Ok(None);
        }

        Ok(Some(SingleInstanceGuard { handle }))
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct SingleInstanceGuard;

    pub fn acquire(_open_paths: &[PathBuf]) -> Result<Option<SingleInstanceGuard>, String> {
        Ok(Some(SingleInstanceGuard))
    }
}

pub use platform::acquire;
