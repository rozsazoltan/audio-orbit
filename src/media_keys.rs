use std::sync::mpsc::Receiver;

#[cfg(windows)]
use std::sync::mpsc;

#[derive(Clone, Copy, Debug)]
pub enum MediaKeyCommand {
    Previous,
    PlayPause,
    Stop,
    Next,
}

impl MediaKeyCommand {
    pub fn label(self) -> &'static str {
        match self {
            Self::Previous => "Previous",
            Self::PlayPause => "Play/Pause",
            Self::Stop => "Stop",
            Self::Next => "Next",
        }
    }
}

#[derive(Clone, Debug)]
pub enum MediaKeyEvent {
    Ready {
        registered: Vec<MediaKeyCommand>,
        failed: Vec<MediaKeyCommand>,
    },
    Command(MediaKeyCommand),
}

pub struct MediaKeyListener {
    pub receiver: Option<Receiver<MediaKeyEvent>>,
    pub status_message: String,
}

pub fn start_listener() -> MediaKeyListener {
    start_platform_listener()
}

#[cfg(windows)]
fn start_platform_listener() -> MediaKeyListener {
    let (sender, receiver) = mpsc::channel();

    std::thread::Builder::new()
        .name("audio-orbit-media-keys".to_owned())
        .spawn(move || run_windows_media_key_loop(sender))
        .ok();

    MediaKeyListener {
        receiver: Some(receiver),
        status_message: "Media keys: starting".to_owned(),
    }
}

#[cfg(not(windows))]
fn start_platform_listener() -> MediaKeyListener {
    MediaKeyListener {
        receiver: None,
        status_message: "Media keys: Windows only".to_owned(),
    }
}

#[cfg(windows)]
fn run_windows_media_key_loop(sender: mpsc::Sender<MediaKeyEvent>) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, RegisterHotKey, UnregisterHotKey, MSG, MOD_NOREPEAT, WM_HOTKEY,
    };

    const HOTKEY_PREVIOUS: i32 = 0x4101;
    const HOTKEY_PLAY_PAUSE: i32 = 0x4102;
    const HOTKEY_STOP: i32 = 0x4103;
    const HOTKEY_NEXT: i32 = 0x4104;

    let hotkeys = [
        (HOTKEY_PREVIOUS, VK_MEDIA_PREV_TRACK as u32, MediaKeyCommand::Previous),
        (HOTKEY_PLAY_PAUSE, VK_MEDIA_PLAY_PAUSE as u32, MediaKeyCommand::PlayPause),
        (HOTKEY_STOP, VK_MEDIA_STOP as u32, MediaKeyCommand::Stop),
        (HOTKEY_NEXT, VK_MEDIA_NEXT_TRACK as u32, MediaKeyCommand::Next),
    ];

    let mut registered_ids = Vec::new();
    let mut registered_commands = Vec::new();
    let mut failed_commands = Vec::new();

    for (id, key, command) in hotkeys {
        let registered = unsafe { RegisterHotKey(0, id, MOD_NOREPEAT, key) != 0 };
        if registered {
            registered_ids.push(id);
            registered_commands.push(command);
        } else {
            failed_commands.push(command);
        }
    }

    let _ = sender.send(MediaKeyEvent::Ready {
        registered: registered_commands,
        failed: failed_commands,
    });

    if registered_ids.is_empty() {
        return;
    }

    loop {
        let mut message = unsafe { std::mem::zeroed::<MSG>() };
        let result = unsafe { GetMessageW(&mut message, 0, 0, 0) };
        if result <= 0 {
            break;
        }

        if message.message != WM_HOTKEY {
            continue;
        }

        let command = match message.wParam as i32 {
            HOTKEY_PREVIOUS => Some(MediaKeyCommand::Previous),
            HOTKEY_PLAY_PAUSE => Some(MediaKeyCommand::PlayPause),
            HOTKEY_STOP => Some(MediaKeyCommand::Stop),
            HOTKEY_NEXT => Some(MediaKeyCommand::Next),
            _ => None,
        };

        if let Some(command) = command {
            let _ = sender.send(MediaKeyEvent::Command(command));
        }
    }

    for id in registered_ids {
        unsafe {
            UnregisterHotKey(0, id);
        }
    }
}
