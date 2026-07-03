use crate::*;

impl AudioOrbitApp {
    pub(crate) fn add_audio_files(&mut self) {
        let Some(files) = FileDialog::new()
            .add_filter("Audio files", &["mp3", "wav", "flac", "ogg", "opus", "m4a", "mp4", "aac", "aiff", "aif", "ape", "wv"])
            .add_filter("All files", &["*"])
            .pick_files()
        else {
            return;
        };

        let Some(playlist) = self.current_playlist() else {
            return;
        };
        if !playlist.accepts_manual_tracks() {
            self.error_message = Some("Folder playlists are scanner-owned. Add files to a manual playlist or Favorites instead.".to_owned());
            return;
        }

        let added_count = files.len();
        let Some((start_index, playlist_name)) = self.current_playlist_mut().map(|playlist| {
            let start_index = playlist.tracks.len();
            playlist.add_files(files);
            (start_index, playlist.name.clone())
        }) else {
            return;
        };

        self.selected_track_index = Some(start_index);
        self.ensure_selected_track_visible();
        self.status_message = format!("Added {added_count} track(s) to {playlist_name}.");
        self.error_message = None;
        self.save_state_silently();
    }
    pub(crate) fn open_folder_import_modal(&mut self) {
        self.show_folder_import_modal = true;
        self.error_message = None;
    }
    pub(crate) fn pick_music_folder(&mut self) {
        let Some(folder) = FileDialog::new().pick_folder() else {
            return;
        };

        if self.pending_playlist_name.trim().is_empty() || self.pending_playlist_name == "Local music" {
            self.pending_playlist_name = folder
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Local music".to_owned());
        }

        self.pending_folder_path = Some(folder);
    }
    pub(crate) fn import_folder_playlist(&mut self) -> bool {
        let Some(folder) = self.pending_folder_path.clone() else {
            self.error_message = Some("Choose a music folder first.".to_owned());
            return false;
        };

        let name = if self.pending_playlist_name.trim().is_empty() {
            folder
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Local music".to_owned())
        } else {
            self.pending_playlist_name.trim().to_owned()
        };

        if !self.start_folder_scan(PendingFolderScanKind::Import {
            name: name.clone(),
            folder: folder.clone(),
            depth: self.pending_folder_depth,
        }) {
            return false;
        }
        self.status_message = format!("Scanning {} for {name}...", folder.display());
        true
    }
    pub(crate) fn rescan_current_folder(&mut self) {
        let playlist_index = self.state.selected_playlist_index;
        let Some((folder, depth, name)) = self.current_playlist().and_then(|playlist| {
            playlist
                .source_folder
                .clone()
                .map(|folder| (folder, playlist.folder_depth, playlist.name.clone()))
        }) else {
            self.error_message = Some("This playlist was not created from a folder.".to_owned());
            return;
        };

        if self.start_folder_scan(PendingFolderScanKind::Rescan {
            playlist_index,
            name: name.clone(),
            folder: folder.clone(),
            depth,
        }) {
            self.status_message = format!("Rescanning {name} in background...");
        }
    }
    pub(crate) fn start_folder_scan(&mut self, kind: PendingFolderScanKind) -> bool {
        if self.pending_folder_scan_receiver.is_some() {
            self.error_message = Some("A folder scan is already running. Wait for it to finish before starting another scan.".to_owned());
            return false;
        }

        let folder = match &kind {
            PendingFolderScanKind::Import { folder, .. } => folder.clone(),
            PendingFolderScanKind::Rescan { folder, .. } => folder.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        self.pending_folder_scan_receiver = Some(receiver);
        self.error_message = None;

        thread::spawn(move || {
            let result = collect_audio_files_from_folder(&folder)
                .map(|files| PendingFolderScanResult { kind, files })
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        true
    }
    pub(crate) fn process_folder_scan_events(&mut self) {
        let Some(receiver) = &self.pending_folder_scan_receiver else {
            return;
        };

        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_folder_scan_receiver = None;
                self.error_message = Some("Folder scan stopped before returning a result.".to_owned());
                return;
            }
        };

        self.pending_folder_scan_receiver = None;
        let result = match message {
            Ok(result) => result,
            Err(error) => {
                self.error_message = Some(error);
                self.status_message = "Folder scan failed.".to_owned();
                return;
            }
        };

        if result.files.is_empty() {
            let folder = match &result.kind {
                PendingFolderScanKind::Import { folder, .. } => folder,
                PendingFolderScanKind::Rescan { folder, .. } => folder,
            };
            self.error_message = Some(format!(
                "No supported audio files were found under {}.",
                folder.display()
            ));
            self.status_message = "Folder scan finished without tracks.".to_owned();
            return;
        }

        match result.kind {
            PendingFolderScanKind::Import { name, folder, depth } => {
                let track_count = result.files.len();
                let playlist = Playlist::from_folder(name.clone(), folder.clone(), depth, result.files);
                self.state.playlists.push(playlist);
                self.state.selected_playlist_index = self.state.playlists.len() - 1;
                self.restore_repeat_selection_for_current_playlist();
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!(
                    "Imported {track_count} track(s) from {} as {name}.",
                    folder.display()
                );
                self.error_message = None;
                self.save_state_silently();
            }
            PendingFolderScanKind::Rescan { playlist_index, name, folder, depth } => {
                let track_count = result.files.len();
                let Some(playlist) = self.state.playlists.get_mut(playlist_index) else {
                    self.error_message = Some(format!("{name} no longer exists; rescan result was ignored."));
                    return;
                };
                if playlist.source_folder.as_ref().map(|source| same_path(source, &folder)).unwrap_or(false) {
                    playlist.folder_depth = depth;
                    playlist.replace_tracks_from_files(result.files);
                    if self.state.selected_playlist_index == playlist_index {
                        self.restore_repeat_selection_for_current_playlist();
                        self.selected_track_index = self.eligible_track_indexes().first().copied();
                    }
                    self.status_message = format!("Rescanned {name}: {track_count} track(s) found.");
                    self.error_message = None;
                    self.save_state_silently();
                } else {
                    self.error_message = Some(format!("{name} changed source folders; rescan result was ignored."));
                }
            }
        }
    }
    pub(crate) fn export_app_backup(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Audio Orbit backup", &["zip"])
            .set_file_name(default_backup_file_name())
            .save_file()
        else {
            return;
        };

        match export_state_zip(&self.state, &path) {
            Ok(()) => {
                self.status_message = format!("Exported app backup to {}.", path.display());
                self.error_message = None;
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }
    pub(crate) fn import_app_backup(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Audio Orbit backup", &["zip"])
            .pick_file()
        else {
            return;
        };

        match import_state_zip(&path) {
            Ok(mut state) => {
                ensure_state_is_valid(&mut state);
                self.stop();
                self.state = state;
                self.show_library_panel = self.state.ui.show_library_panel;
                self.show_profile_panel = self.state.ui.show_profile_panel;
                self.player_only_mode = self.state.ui.player_only_mode;
                self.restore_repeat_selection_for_current_playlist();
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!("Imported app backup from {}.", path.display());
                self.error_message = None;
                self.save_state_silently();
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }
}
