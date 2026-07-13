use std::path::PathBuf;

#[cfg(windows)]
use std::sync::mpsc;

#[derive(Clone, Debug)]
pub(crate) struct FolderWatchChange {
    pub(crate) path: PathBuf,
    pub(crate) scan_directory_if_present: bool,
}

#[derive(Debug)]
pub(crate) enum FolderWatchEvent {
    Changes(Vec<FolderWatchChange>),
    Overflow,
    Failed(String),
}

#[cfg(windows)]
pub(crate) struct FolderWatcher {
    receiver: mpsc::Receiver<FolderWatchEvent>,
    stop_event: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl FolderWatcher {
    pub(crate) fn start(root: PathBuf, context: eframe::egui::Context) -> Result<Self, String> {
        use std::{os::windows::ffi::OsStrExt, ptr};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
                FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
                FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
                OPEN_EXISTING,
            },
            System::Threading::{CreateEventW, SetEvent},
        };

        let metadata = std::fs::metadata(&root).map_err(|error| {
            format!(
                "failed to inspect watched folder {}: {error}",
                root.display()
            )
        })?;
        if !metadata.is_dir() {
            return Err(format!("watched path is not a folder: {}", root.display()));
        }

        let mut wide_root = root.as_os_str().encode_wide().collect::<Vec<_>>();
        wide_root.push(0);

        // SAFETY: wide_root is NUL-terminated and remains alive for the call.
        // Null security/template pointers are allowed by CreateFileW.
        let directory_handle = unsafe {
            CreateFileW(
                wide_root.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if directory_handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "failed to open watched folder {}: {}",
                root.display(),
                std::io::Error::last_os_error()
            ));
        }

        // SAFETY: Null security/name pointers request default unnamed event.
        let stop_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if stop_event.is_null() {
            // SAFETY: directory_handle is valid and still exclusively owned here.
            unsafe {
                CloseHandle(directory_handle);
            }
            return Err(format!(
                "failed to create folder watcher stop event: {}",
                std::io::Error::last_os_error()
            ));
        }

        // SAFETY: Null security/name pointers request default unnamed event.
        let io_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if io_event.is_null() {
            // SAFETY: Both handles are valid and still exclusively owned here.
            unsafe {
                CloseHandle(stop_event);
                CloseHandle(directory_handle);
            }
            return Err(format!(
                "failed to create folder watcher I/O event: {}",
                std::io::Error::last_os_error()
            ));
        }

        let (sender, receiver) = mpsc::channel();
        let thread_root = root.clone();
        // Raw Win32 HANDLE pointers are not Send. Transfer their integer values,
        // then reconstruct handles inside watcher thread that owns directory I/O.
        let thread_directory_handle = directory_handle as isize;
        let thread_stop_event = stop_event as isize;
        let thread_io_event = io_event as isize;
        let thread = match std::thread::Builder::new()
            .name("audio-orbit-folder-watcher".to_owned())
            .spawn(move || {
                let directory_handle = ThreadOwnedHandle(
                    thread_directory_handle as windows_sys::Win32::Foundation::HANDLE,
                );
                let stop_event =
                    thread_stop_event as windows_sys::Win32::Foundation::HANDLE;
                let io_event = ThreadOwnedHandle(
                    thread_io_event as windows_sys::Win32::Foundation::HANDLE,
                );
                run_watcher_thread(
                    thread_root,
                    directory_handle.get(),
                    stop_event,
                    io_event.get(),
                    sender,
                    context,
                );
            })
        {
            Ok(thread) => thread,
            Err(error) => {
                // SAFETY: Thread did not start, so all handles remain exclusively
                // owned by this scope and can be closed exactly once.
                unsafe {
                    SetEvent(stop_event);
                    CloseHandle(io_event);
                    CloseHandle(stop_event);
                    CloseHandle(directory_handle);
                }
                return Err(format!("failed to start folder watcher thread: {error}"));
            }
        };

        Ok(Self {
            receiver,
            stop_event: stop_event as isize,
            thread: Some(thread),
        })
    }

    pub(crate) fn try_recv(&self) -> Option<FolderWatchEvent> {
        self.receiver.try_recv().ok()
    }
}

#[cfg(windows)]
struct ThreadOwnedHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ThreadOwnedHandle {
    fn get(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }
}

#[cfg(windows)]
impl Drop for ThreadOwnedHandle {
    fn drop(&mut self) {
        // SAFETY: Handle was created successfully, is owned by watcher thread, and
        // is closed exactly once when thread scope ends, including panic unwinding.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn run_watcher_thread(
    root: PathBuf,
    directory_handle: windows_sys::Win32::Foundation::HANDLE,
    stop_event: windows_sys::Win32::Foundation::HANDLE,
    io_event: windows_sys::Win32::Foundation::HANDLE,
    sender: mpsc::Sender<FolderWatchEvent>,
    context: eframe::egui::Context,
) {
    use std::{ffi::c_void, mem, ptr};
    use windows_sys::Win32::{
        Storage::FileSystem::{
            ReadDirectoryChangesW, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        },
        System::{
            IO::{GetOverlappedResult, OVERLAPPED},
            Threading::{ResetEvent, WaitForMultipleObjects, INFINITE},
        },
    };

    const WAIT_OBJECT_0_VALUE: u32 = 0;
    const WAIT_FAILED_VALUE: u32 = u32::MAX;
    const BUFFER_BYTES: usize = 64 * 1024;

    // u32 storage guarantees DWORD alignment required by ReadDirectoryChangesW.
    let mut buffer = vec![0_u32; BUFFER_BYTES / mem::size_of::<u32>()];
    let handles = [stop_event, io_event];

    loop {
        // SAFETY: io_event is a valid manual-reset event owned by watcher thread.
        unsafe {
            ResetEvent(io_event);
        }

        // SAFETY: Zero initialization is valid for OVERLAPPED before assigning hEvent.
        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = io_event;

        // SAFETY: Handles are valid; buffer and OVERLAPPED live until completion
        // or cancellation; buffer length matches allocated storage.
        let queued = unsafe {
            ReadDirectoryChangesW(
                directory_handle,
                buffer.as_mut_ptr().cast::<c_void>(),
                BUFFER_BYTES as u32,
                1,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                ptr::null_mut(),
                &mut overlapped,
                None,
            )
        };
        if queued == 0 {
            send_failure(
                &sender,
                &context,
                format!(
                    "folder watcher read failed for {}: {}",
                    root.display(),
                    std::io::Error::last_os_error()
                ),
            );
            break;
        }

        // SAFETY: handles contains two valid event handles and remains alive for call.
        let wait_result = unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
        };
        match wait_result {
            WAIT_OBJECT_0_VALUE => {
                cancel_pending_read(directory_handle, &overlapped);
                break;
            }
            value if value == WAIT_OBJECT_0_VALUE + 1 => {
                let mut bytes_returned = 0;
                // SAFETY: OVERLAPPED belongs to completed request on directory_handle.
                let completed = unsafe {
                    GetOverlappedResult(
                        directory_handle,
                        &overlapped,
                        &mut bytes_returned,
                        0,
                    )
                };
                if completed == 0 {
                    const ERROR_NOTIFY_ENUM_DIR_VALUE: i32 = 1022;
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_NOTIFY_ENUM_DIR_VALUE) {
                        if sender.send(FolderWatchEvent::Overflow).is_err() {
                            break;
                        }
                        context.request_repaint();
                        continue;
                    }

                    send_failure(
                        &sender,
                        &context,
                        format!(
                            "folder watcher completion failed for {}: {error}",
                            root.display()
                        ),
                    );
                    break;
                }

                if bytes_returned == 0 {
                    if sender.send(FolderWatchEvent::Overflow).is_err() {
                        break;
                    }
                    context.request_repaint();
                    continue;
                }

                let bytes_returned = bytes_returned as usize;
                if bytes_returned > BUFFER_BYTES {
                    send_failure(
                        &sender,
                        &context,
                        format!(
                            "folder watcher returned oversized data for {}",
                            root.display()
                        ),
                    );
                    break;
                }

                // SAFETY: ReadDirectoryChangesW completed successfully and reported a
                // byte count no larger than the aligned backing buffer.
                let bytes = unsafe {
                    std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes_returned)
                };
                match parse_change_paths(&root, bytes) {
                    Ok(paths) if paths.is_empty() => {}
                    Ok(paths) => {
                        if sender.send(FolderWatchEvent::Changes(paths)).is_err() {
                            break;
                        }
                        context.request_repaint();
                    }
                    Err(error) => {
                        send_failure(
                            &sender,
                            &context,
                            format!("invalid folder watcher data for {}: {error}", root.display()),
                        );
                        break;
                    }
                }
            }
            WAIT_FAILED_VALUE => {
                send_failure(
                    &sender,
                    &context,
                    format!(
                        "folder watcher wait failed for {}: {}",
                        root.display(),
                        std::io::Error::last_os_error()
                    ),
                );
                cancel_pending_read(directory_handle, &overlapped);
                break;
            }
            value => {
                send_failure(
                    &sender,
                    &context,
                    format!(
                        "folder watcher returned unexpected wait result {value} for {}",
                        root.display()
                    ),
                );
                cancel_pending_read(directory_handle, &overlapped);
                break;
            }
        }
    }
}

#[cfg(windows)]
fn cancel_pending_read(
    directory_handle: windows_sys::Win32::Foundation::HANDLE,
    overlapped: &windows_sys::Win32::System::IO::OVERLAPPED,
) {
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult};

    // SAFETY: OVERLAPPED and directory handle belong to same pending request.
    // Waiting for cancellation completion keeps backing buffer alive until I/O ends.
    unsafe {
        CancelIoEx(directory_handle, overlapped);
        let mut ignored_bytes = 0;
        GetOverlappedResult(directory_handle, overlapped, &mut ignored_bytes, 1);
    }
}

#[cfg(windows)]
fn send_failure(
    sender: &mpsc::Sender<FolderWatchEvent>,
    context: &eframe::egui::Context,
    message: String,
) {
    let _ = sender.send(FolderWatchEvent::Failed(message));
    context.request_repaint();
}

#[cfg(windows)]
fn parse_change_paths(
    root: &std::path::Path,
    buffer: &[u8],
) -> Result<Vec<FolderWatchChange>, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::Component};

    const HEADER_BYTES: usize = 12;
    let mut paths = Vec::new();
    let mut offset = 0_usize;

    loop {
        if buffer.len().saturating_sub(offset) < HEADER_BYTES {
            return Err("truncated FILE_NOTIFY_INFORMATION header".to_owned());
        }

        let next_entry_offset = u32::from_ne_bytes(
            buffer[offset..offset + 4]
                .try_into()
                .map_err(|_| "invalid next-entry offset")?,
        ) as usize;
        let action = u32::from_ne_bytes(
            buffer[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| "invalid action")?,
        );
        let file_name_bytes = u32::from_ne_bytes(
            buffer[offset + 8..offset + 12]
                .try_into()
                .map_err(|_| "invalid file-name length")?,
        ) as usize;

        if file_name_bytes == 0 || file_name_bytes % 2 != 0 {
            return Err("invalid UTF-16 file-name length".to_owned());
        }
        let name_end = offset
            .checked_add(HEADER_BYTES)
            .and_then(|value| value.checked_add(file_name_bytes))
            .ok_or_else(|| "file-name length overflow".to_owned())?;
        if name_end > buffer.len() {
            return Err("truncated FILE_NOTIFY_INFORMATION file name".to_owned());
        }

        let file_name = buffer[offset + HEADER_BYTES..name_end]
            .chunks_exact(2)
            .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let relative = PathBuf::from(OsString::from_wide(&file_name));
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err("notification contained unsafe relative path".to_owned());
        }
        const FILE_ACTION_ADDED_VALUE: u32 = 1;
        const FILE_ACTION_REMOVED_VALUE: u32 = 2;
        const FILE_ACTION_MODIFIED_VALUE: u32 = 3;
        const FILE_ACTION_RENAMED_OLD_NAME_VALUE: u32 = 4;
        const FILE_ACTION_RENAMED_NEW_NAME_VALUE: u32 = 5;
        let scan_directory_if_present = match action {
            FILE_ACTION_ADDED_VALUE | FILE_ACTION_RENAMED_NEW_NAME_VALUE => Some(true),
            FILE_ACTION_REMOVED_VALUE | FILE_ACTION_RENAMED_OLD_NAME_VALUE => Some(false),
            FILE_ACTION_MODIFIED_VALUE => None,
            _ => return Err(format!("unknown file notification action: {action}")),
        };
        if let Some(scan_directory_if_present) = scan_directory_if_present {
            paths.push(FolderWatchChange {
                path: root.join(relative),
                scan_directory_if_present,
            });
        }

        if next_entry_offset == 0 {
            break;
        }
        if next_entry_offset < HEADER_BYTES {
            return Err("invalid next-entry offset".to_owned());
        }
        offset = offset
            .checked_add(next_entry_offset)
            .ok_or_else(|| "next-entry offset overflow".to_owned())?;
        if offset >= buffer.len() {
            return Err("next-entry offset exceeds notification buffer".to_owned());
        }
    }

    Ok(paths)
}

#[cfg(windows)]
impl Drop for FolderWatcher {
    fn drop(&mut self) {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::SetEvent};

        // SAFETY: stop_event is owned by FolderWatcher and remains valid until join.
        unsafe {
            SetEvent(self.stop_event as windows_sys::Win32::Foundation::HANDLE);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // SAFETY: Watcher thread has joined; stop event is closed exactly once here.
        unsafe {
            CloseHandle(self.stop_event as windows_sys::Win32::Foundation::HANDLE);
        }
    }
}

#[cfg(not(windows))]
pub(crate) struct FolderWatcher;

#[cfg(not(windows))]
impl FolderWatcher {
    pub(crate) fn start(root: PathBuf, _context: eframe::egui::Context) -> Result<Self, String> {
        Err(format!(
            "automatic folder watching is only available on Windows: {}",
            root.display()
        ))
    }

    pub(crate) fn try_recv(&self) -> Option<FolderWatchEvent> {
        None
    }
}
