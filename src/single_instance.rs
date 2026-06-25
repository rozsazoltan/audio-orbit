#[cfg(windows)]
mod platform {
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

    pub fn acquire() -> Result<Option<SingleInstanceGuard>, String> {
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
            return Ok(None);
        }

        Ok(Some(SingleInstanceGuard { handle }))
    }
}

#[cfg(not(windows))]
mod platform {
    pub struct SingleInstanceGuard;

    pub fn acquire() -> Result<Option<SingleInstanceGuard>, String> {
        Ok(Some(SingleInstanceGuard))
    }
}

pub use platform::acquire;
