use crate::*;

impl AudioOrbitApp {
    pub(crate) fn current_unix_seconds() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default()
    }
    pub(crate) fn maybe_start_auto_update_check(&mut self) {
        if cfg!(debug_assertions) {
            return;
        }
        if self.update_check_receiver.is_some() || self.update_install_receiver.is_some() {
            return;
        }

        let now = Self::current_unix_seconds();
        let last_check = self.state.update_settings.last_auto_check_unix_seconds;
        if last_check != 0 && now.saturating_sub(last_check) < 60 * 60 {
            return;
        }

        self.state.update_settings.last_auto_check_unix_seconds = now;
        self.save_state_silently();
        self.start_update_check(false);
    }
    pub(crate) fn start_update_check(&mut self, manual: bool) {
        if self.update_check_receiver.is_some() {
            self.status_message = "Update check is already running.".to_owned();
            return;
        }

        let include_prereleases = self.state.update_settings.include_prereleases;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = updater::check_for_update(include_prereleases).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.update_check_receiver = Some(receiver);
        self.update_check_started_at = Some(Instant::now());
        if manual {
            self.status_message = if include_prereleases {
                "Checking for prerelease updates...".to_owned()
            } else {
                "Checking for stable updates...".to_owned()
            };
        }
        self.error_message = None;
    }
    pub(crate) fn process_update_events(&mut self) {
        let update_check_result = self
            .update_check_receiver
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("Update check stopped before returning a result.".to_owned())),
            });

        if let Some(result) = update_check_result {
            self.update_check_receiver = None;
            self.update_check_started_at = None;
            match result {
                Ok(check) => {
                    if check.is_update_available {
                        let release_type = if check.prerelease { "Prerelease" } else { "Stable" };
                        self.status_message = format!("{release_type} update available: v{}.", check.latest_version);
                    } else {
                        let release_type = if check.prerelease { "prerelease" } else { "stable" };
                        self.status_message = format!("No newer {release_type} release is available. Current version: v{}.", check.current_version);
                    }
                    self.error_message = None;
                    self.last_update_check = Some(check);
                }
                Err(error) => {
                    self.error_message = Some(error);
                    self.status_message = "Update check failed.".to_owned();
                }
            }
        }

        let install_result = self
            .update_install_receiver
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("Update installer stopped before replacing the executable.".to_owned())),
            });

        if let Some(result) = install_result {
            self.update_install_receiver = None;
            self.update_install_started_at = None;
            match result {
                Ok(()) => {
                    self.status_message = "Update installer finished.".to_owned();
                    self.error_message = None;
                }
                Err(error) => {
                    self.error_message = Some(error);
                    self.status_message = "Update install failed.".to_owned();
                }
            }
        }
    }
    pub(crate) fn start_update_install(&mut self, check: updater::UpdateCheck) {
        if self.update_install_receiver.is_some() {
            self.status_message = "Update install is already running.".to_owned();
            return;
        }
        if !check.is_update_available {
            self.status_message = "No newer update is available.".to_owned();
            return;
        }
        if check.asset_download_url.is_none() {
            self.error_message = Some("The selected release does not contain a Windows executable asset.".to_owned());
            return;
        }

        self.persist_playback_session();
        self.persist_repeat_selection_for_current_playlist();
        let _ = save_state(&self.state);

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = updater::install_update(&check).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.update_install_receiver = Some(receiver);
        self.update_install_started_at = Some(Instant::now());
        self.status_message = "Downloading and installing update...".to_owned();
        self.error_message = None;
    }
    pub(crate) fn start_switch_to_stable_install(&mut self) {
        if self.update_install_receiver.is_some() {
            self.status_message = "Update install is already running.".to_owned();
            return;
        }

        self.state.update_settings.include_prereleases = false;
        self.last_update_check = None;
        self.persist_playback_session();
        self.persist_repeat_selection_for_current_playlist();
        let _ = save_state(&self.state);

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = updater::check_latest_stable()
                .and_then(|check| updater::install_update(&check))
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.update_install_receiver = Some(receiver);
        self.update_install_started_at = Some(Instant::now());
        self.status_message = "Switching back to the latest stable release...".to_owned();
        self.error_message = None;
    }
    pub(crate) fn open_releases_page(&mut self) {
        match updater::open_releases_page() {
            Ok(()) => {
                self.status_message = "Opened Audio Orbit releases.".to_owned();
                self.error_message = None;
            }
            Err(error) => self.error_message = Some(error.to_string()),
        }
    }
}
