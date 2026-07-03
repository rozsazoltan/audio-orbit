use crate::*;

impl AudioOrbitApp {
    pub(crate) fn add_radio_station(&mut self) -> bool {
        let typed_name = self.pending_radio_name.trim().to_owned();
        let url = self.pending_radio_url.trim().to_owned();
        if url.is_empty() {
            self.error_message = Some("Radio stream URL is required.".to_owned());
            return false;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.error_message = Some("Internet radio stream URL must start with http:// or https://.".to_owned());
            return false;
        }

        let metadata = if typed_name.is_empty() {
            self.status_message = "Reading internet radio station name...".to_owned();
            fetch_radio_stream_metadata(&url)
        } else {
            None
        };
        let station_name = if typed_name.is_empty() {
            metadata
                .as_ref()
                .and_then(|metadata| metadata.station_name.clone())
                .unwrap_or_else(|| fallback_radio_station_name(&url))
        } else {
            typed_name
        };

        let mut station = RadioStation::new(station_name, url);
        if let Some(metadata) = metadata {
            station.last_station_name = metadata.station_name;
            station.last_stream_title = metadata.stream_title;
        }
        self.state.radio_stations.push(station);
        self.state.selected_radio_index = None;
        self.radio_selection_was_user_set = false;
        self.pending_radio_name.clear();
        self.pending_radio_url.clear();
        self.status_message = "Added internet radio station.".to_owned();
        self.error_message = None;
        self.save_state_silently();
        true
    }
    pub(crate) fn remove_selected_radio_station(&mut self) {
        let Some(index) = self.state.selected_radio_index else {
            return;
        };
        if index >= self.state.radio_stations.len() {
            return;
        }
        if self.active_radio_index == Some(index) {
            self.stop();
        }
        self.state.radio_stations.remove(index);
        self.state.selected_radio_index = if self.state.radio_stations.is_empty() {
            None
        } else {
            Some(index.min(self.state.radio_stations.len() - 1))
        };
        self.save_state_silently();
    }
    pub(crate) fn play_radio_station(&mut self, index: usize) {
        let Some(station) = self.state.radio_stations.get(index).cloned() else {
            return;
        };
        let settings = self.current_settings();
        let crossfade_seconds = self.configured_manual_crossfade_seconds();
        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };
        self.status_message = if crossfade_seconds > 0.05 {
            format!("Crossfading to internet radio: {}...", station.name)
        } else {
            format!("Opening internet radio: {}...", station.name)
        };
        self.error_message = None;
        match player.play_radio_stream_with_crossfade(&station.url, settings, crossfade_seconds) {
            Ok(()) => {
                self.active_tab = MainContentTab::Radio;
                self.active_radio_index = Some(index);
                self.active_radio_station_name = station.last_station_name.clone();
                self.active_radio_title = station.last_stream_title.clone();
                self.radio_started_at = Some(Instant::now());
                self.last_radio_title_lookup_at = Some(Instant::now());
                self.active_track_index = None;
                self.active_playlist_index = None;
                self.active_track_path = None;
                self.last_playback = None;
                self.pending_track_switch = None;
                self.state.selected_radio_index = Some(index);
                self.radio_selection_was_user_set = true;
                self.status_message = if crossfade_seconds > 0.05 {
                    format!("Crossfading to internet radio: {}.", station.name)
                } else {
                    format!("Playing internet radio: {}.", station.name)
                };
                self.persist_playback_session();
                self.save_state_silently();
                self.start_radio_title_lookup(index, station.url);
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Internet radio playback failed.".to_owned();
            }
        }
    }
    pub(crate) fn start_radio_title_lookup(&mut self, index: usize, url: String) {
        let (sender, receiver) = mpsc::channel();
        self.radio_title_receiver = Some(receiver);
        thread::spawn(move || {
            let metadata = fetch_radio_stream_metadata(&url);
            let _ = sender.send((index, metadata));
        });
    }
    pub(crate) fn process_radio_title_events(&mut self) {
        let Some(receiver) = &self.radio_title_receiver else {
            return;
        };
        let events = receiver.try_iter().collect::<Vec<_>>();
        let mut completed = false;
        for (index, metadata) in events {
            completed = true;
            if let Some(metadata) = metadata {
                if self.active_radio_index == Some(index) {
                    self.active_radio_station_name = metadata.station_name.clone().or_else(|| self.active_radio_station_name.clone());
                    self.active_radio_title = metadata.stream_title.clone().or_else(|| self.active_radio_title.clone());
                }
                if let Some(station) = self.state.radio_stations.get_mut(index) {
                    if metadata.station_name.is_some() {
                        station.last_station_name = metadata.station_name;
                    }
                    if metadata.stream_title.is_some() {
                        station.last_stream_title = metadata.stream_title;
                    }
                }
                self.save_state_silently();
            }
        }
        if completed {
            self.radio_title_receiver = None;
        }
    }
    pub(crate) fn refresh_radio_title_periodically(&mut self) {
        let Some(index) = self.active_radio_index else {
            return;
        };
        if self.radio_title_receiver.is_some() {
            return;
        }
        let Some(last_lookup) = self.last_radio_title_lookup_at else {
            return;
        };
        if last_lookup.elapsed() < Duration::from_secs(RADIO_METADATA_REFRESH_INTERVAL_SECONDS) {
            return;
        }
        let Some(station) = self.state.radio_stations.get(index) else {
            return;
        };
        self.last_radio_title_lookup_at = Some(Instant::now());
        self.start_radio_title_lookup(index, station.url.clone());
    }
    pub(crate) fn current_radio_recording_name(&self) -> String {
        if let Some(index) = self.active_radio_index {
            if let Some(station) = self.state.radio_stations.get(index) {
                return self
                    .active_radio_station_name
                    .clone()
                    .or_else(|| station.last_station_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| station.name.clone());
            }
        }
        "internet-radio".to_owned()
    }
    pub(crate) fn toggle_radio_recording(&mut self) {
        let is_recording = match self.player.as_ref() {
            Some(player) => player.is_radio_recording(),
            None => {
                self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
                return;
            }
        };

        if is_recording {
            let result = self
                .player
                .as_mut()
                .map(|player| player.stop_radio_recording());
            match result {
                Some(Ok(Some(info))) => {
                    self.status_message = format!(
                        "Saved radio recording: {} ({}).",
                        info.path.display(),
                        format_file_size(info.bytes_written)
                    );
                    self.error_message = None;
                }
                Some(Ok(None)) => {
                    self.status_message = "No active radio recording.".to_owned();
                }
                Some(Err(error)) => {
                    self.error_message = Some(error.to_string());
                }
                None => {
                    self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
                }
            }
            return;
        }

        if self.active_radio_index.is_none() {
            self.error_message = Some("Start an internet radio station before recording.".to_owned());
            return;
        }

        let folder = self.state.recording.resolved_output_folder();
        let station_name = self.current_radio_recording_name();
        let stream_title = self.active_radio_title.clone();
        let result = self
            .player
            .as_mut()
            .map(|player| player.start_radio_recording(&folder, &station_name, stream_title.as_deref()));
        match result {
            Some(Ok(path)) => {
                self.status_message = format!("Recording internet radio to {}.", path.display());
                self.error_message = None;
            }
            Some(Err(error)) => {
                self.error_message = Some(error.to_string());
            }
            None => {
                self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            }
        }
    }
    pub(crate) fn choose_recording_folder(&mut self) {
        let initial_dir = self.state.recording.resolved_output_folder();
        let mut dialog = FileDialog::new();
        if initial_dir.exists() {
            dialog = dialog.set_directory(initial_dir);
        }
        if let Some(path) = dialog.pick_folder() {
            self.state.recording.output_folder = Some(path.clone());
            self.status_message = format!("Radio recordings folder set to {}.", path.display());
            self.error_message = None;
            self.save_state_silently();
        }
    }
    pub(crate) fn open_recording_folder(&mut self) {
        let folder = self.state.recording.resolved_output_folder();
        if let Err(error) = fs::create_dir_all(&folder).and_then(|_| reveal_in_file_manager(&folder).map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))) {
            self.error_message = Some(format!("Failed to open recording folder: {error}"));
        } else {
            self.status_message = format!("Opened radio recordings folder: {}.", folder.display());
            self.error_message = None;
        }
    }
}
