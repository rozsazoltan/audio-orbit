use std::path::Path;

pub const SUPPORTED_AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "opus", "m4a", "mp4", "aac", "aiff", "aif", "ape", "wv",
];

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::{OsStr, OsString},
        process::Command,
    };

    const PROG_ID: &str = "AudioOrbit.Audio";
    const REGISTERED_APP_NAME: &str = "Audio Orbit";
    const CAPABILITIES_PATH: &str = r"Software\Audio Orbit\Capabilities";

    fn run_reg<I, S>(args: I) -> Result<(), String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("reg.exe")
            .args(args)
            .output()
            .map_err(|error| format!("failed to start reg.exe: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(if message.is_empty() {
                format!("reg.exe exited with {}", output.status)
            } else {
                message
            })
        }
    }

    fn add_value(
        key: &str,
        name: Option<&str>,
        value_type: &str,
        data: &str,
    ) -> Result<(), String> {
        let mut args = vec![OsString::from("add"), OsString::from(key)];
        match name {
            Some(name) => {
                args.push(OsString::from("/v"));
                args.push(OsString::from(name));
            }
            None => args.push(OsString::from("/ve")),
        }
        args.extend([
            OsString::from("/t"),
            OsString::from(value_type),
            OsString::from("/d"),
            OsString::from(data),
            OsString::from("/f"),
        ]);
        run_reg(args)
    }

    fn delete_key(key: &str) {
        let _ = run_reg(["delete", key, "/f"]);
    }

    fn delete_value(key: &str, name: &str) {
        let _ = run_reg(["delete", key, "/v", name, "/f"]);
    }

    pub fn register(executable: &Path) -> Result<(), String> {
        let executable = executable
            .canonicalize()
            .unwrap_or_else(|_| executable.to_path_buf());
        let executable_text = executable.to_string_lossy();
        let quoted_executable = format!("\"{executable_text}\"");
        let open_command = format!("{quoted_executable} \"%1\"");
        let icon_value = format!("{quoted_executable},0");

        add_value(
            &format!(r"HKCU\Software\Classes\{PROG_ID}"),
            None,
            "REG_SZ",
            "Audio Orbit audio file",
        )?;
        add_value(
            &format!(r"HKCU\Software\Classes\{PROG_ID}\DefaultIcon"),
            None,
            "REG_SZ",
            &icon_value,
        )?;
        add_value(
            &format!(r"HKCU\Software\Classes\{PROG_ID}\shell\open\command"),
            None,
            "REG_SZ",
            &open_command,
        )?;

        let application_key = r"HKCU\Software\Classes\Applications\audio-orbit.exe";
        add_value(
            application_key,
            Some("FriendlyAppName"),
            "REG_SZ",
            REGISTERED_APP_NAME,
        )?;
        add_value(
            &format!(r"{application_key}\shell\open\command"),
            None,
            "REG_SZ",
            &open_command,
        )?;

        let capabilities_key = format!(r"HKCU\{CAPABILITIES_PATH}");
        add_value(
            &capabilities_key,
            Some("ApplicationName"),
            "REG_SZ",
            REGISTERED_APP_NAME,
        )?;
        add_value(
            &capabilities_key,
            Some("ApplicationDescription"),
            "REG_SZ",
            "Lightweight local music player and DJ mix exporter",
        )?;

        for extension in SUPPORTED_AUDIO_EXTENSIONS {
            let extension = format!(".{extension}");
            add_value(
                &format!(r"{capabilities_key}\FileAssociations"),
                Some(&extension),
                "REG_SZ",
                PROG_ID,
            )?;
            add_value(
                &format!(r"HKCU\Software\Classes\{extension}\OpenWithProgids"),
                Some(PROG_ID),
                "REG_SZ",
                "",
            )?;
            add_value(
                &format!(r"{application_key}\SupportedTypes"),
                Some(&extension),
                "REG_SZ",
                "",
            )?;
        }

        add_value(
            r"HKCU\Software\RegisteredApplications",
            Some(REGISTERED_APP_NAME),
            "REG_SZ",
            CAPABILITIES_PATH,
        )?;
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        delete_value(r"HKCU\Software\RegisteredApplications", REGISTERED_APP_NAME);
        for extension in SUPPORTED_AUDIO_EXTENSIONS {
            let extension = format!(".{extension}");
            delete_value(
                &format!(r"HKCU\Software\Classes\{extension}\OpenWithProgids"),
                PROG_ID,
            );
        }
        delete_key(r"HKCU\Software\Audio Orbit\Capabilities");
        delete_key(r"HKCU\Software\Classes\Applications\audio-orbit.exe");
        delete_key(&format!(r"HKCU\Software\Classes\{PROG_ID}"));
        Ok(())
    }

    pub fn is_registered() -> bool {
        Command::new("reg.exe")
            .args([
                "query",
                r"HKCU\Software\RegisteredApplications",
                "/v",
                REGISTERED_APP_NAME,
            ])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    pub fn open_default_apps_settings() -> Result<(), String> {
        Command::new("explorer.exe")
            .arg("ms-settings:defaultapps?registeredAppUser=Audio%20Orbit")
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open Windows Default Apps settings: {error}"))
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn register(_executable: &Path) -> Result<(), String> {
        Err("File association registration is available on Windows only.".to_owned())
    }

    pub fn unregister() -> Result<(), String> {
        Err("File association registration is available on Windows only.".to_owned())
    }

    pub fn is_registered() -> bool {
        false
    }

    pub fn open_default_apps_settings() -> Result<(), String> {
        Err("Windows Default Apps settings are available on Windows only.".to_owned())
    }
}

pub use platform::{is_registered, open_default_apps_settings, register, unregister};
