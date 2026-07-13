use crate::*;

#[derive(Clone, Copy, Debug, Default)]
struct AppliedLibrarySyncStats {
    added: usize,
    restored: usize,
    newly_missing: usize,
    missing_total: usize,
}

impl AppliedLibrarySyncStats {
    fn changed(self) -> bool {
        self.added > 0 || self.restored > 0 || self.newly_missing > 0
    }
}

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

        let Some((added_paths, playlist_name, playlist_kind)) = self.current_playlist_mut().map(|playlist| {
            let added_paths = playlist.add_files(files);
            (added_paths, playlist.name.clone(), playlist.kind.clone())
        }) else {
            return;
        };

        let added_count = added_paths.len();
        let selected_path = if playlist_kind == PlaylistKind::Favorites {
            added_paths.last()
        } else {
            added_paths.first()
        };
        self.selected_track_index = selected_path.and_then(|path| {
            self.current_playlist()
                .and_then(|playlist| playlist.tracks.iter().position(|track| same_path(&track.path, path)))
        });
        if self.active_playlist_index == Some(self.state.selected_playlist_index) {
            self.active_track_index = self.active_track_path.as_ref().and_then(|active_path| {
                self.current_playlist()
                    .and_then(|playlist| playlist.tracks.iter().position(|track| same_path(&track.path, active_path)))
            });
        }
        self.ensure_selected_track_visible();
        self.status_message = if added_count == 0 {
            format!("Selected tracks are already in {playlist_name}.")
        } else {
            format!("Added {added_count} track(s) to {playlist_name}.")
        };
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
            self.status_message = format!("Syncing {name} in background...");
        }
    }

    pub(crate) fn start_folder_scan(&mut self, kind: PendingFolderScanKind) -> bool {
        if self.pending_folder_scan_receiver.is_some() || self.pending_library_sync_receiver.is_some() {
            self.error_message = Some("A library scan is already running. Wait for it to finish before starting another scan.".to_owned());
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

        match result.kind {
            PendingFolderScanKind::Import { name, folder, depth } => {
                if result.files.is_empty() {
                    self.error_message = Some(format!(
                        "No supported audio files were found under {}.",
                        folder.display()
                    ));
                    self.status_message = "Folder scan finished without tracks.".to_owned();
                    return;
                }

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
                let selected_path = if self.state.selected_playlist_index == playlist_index {
                    self.selected_track_path()
                } else {
                    None
                };
                let Some(playlist) = self.state.playlists.get_mut(playlist_index) else {
                    self.error_message = Some(format!("{name} no longer exists; sync result was ignored."));
                    return;
                };
                if !playlist
                    .source_folder
                    .as_ref()
                    .map(|source| same_path(source, &folder))
                    .unwrap_or(false)
                {
                    self.error_message = Some(format!("{name} changed source folders; sync result was ignored."));
                    return;
                }

                playlist.folder_depth = depth;
                let stats = playlist.merge_tracks_from_folder_scan(&result.files);
                self.remap_track_indexes_after_library_change(selected_path);
                self.status_message = format!(
                    "Synced {name}: {} available, {} added, {} restored, {} missing.",
                    stats.present_total, stats.added, stats.restored, stats.missing_total
                );
                self.error_message = None;
                self.save_state_silently();
            }
        }
    }

    pub(crate) fn start_library_sync(
        &mut self,
        trigger: LibrarySyncTrigger,
        include_folder_scans: bool,
    ) -> bool {
        if self.pending_folder_scan_receiver.is_some() || self.pending_library_sync_receiver.is_some() {
            if trigger == LibrarySyncTrigger::Manual {
                self.error_message = Some("A library scan is already running.".to_owned());
            }
            return false;
        }

        let mut track_paths = BTreeMap::<String, PathBuf>::new();
        for playlist in &self.state.playlists {
            // A full folder scan already determines availability for folder-owned
            // entries. Check those paths separately only during startup, or when
            // the same file is referenced by a manual playlist or Favorites.
            if include_folder_scans && playlist.kind == PlaylistKind::Folder {
                continue;
            }
            for track in &playlist.tracks {
                track_paths
                    .entry(path_key(&track.path))
                    .or_insert_with(|| track.path.clone());
            }
        }

        let folder_targets = if include_folder_scans {
            self.state
                .playlists
                .iter()
                .enumerate()
                .filter_map(|(playlist_index, playlist)| {
                    if playlist.kind != PlaylistKind::Folder {
                        return None;
                    }
                    playlist.source_folder.clone().map(|source_folder| FolderLibrarySyncTarget {
                        playlist_index,
                        playlist_name: playlist.name.clone(),
                        source_folder,
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        if track_paths.is_empty() && folder_targets.is_empty() {
            self.last_library_sync_at = if trigger == LibrarySyncTrigger::Startup
                && self.state.library.auto_sync_folder_playlists
            {
                Instant::now()
                    .checked_sub(AUTO_LIBRARY_SYNC_INTERVAL)
                    .unwrap_or_else(Instant::now)
            } else {
                Instant::now()
            };
            if trigger == LibrarySyncTrigger::Manual {
                self.status_message = "Library has no tracks or folders to sync.".to_owned();
                self.error_message = None;
            }
            return false;
        }

        let (sender, receiver) = mpsc::channel();
        self.pending_library_sync_receiver = Some(receiver);
        self.last_library_sync_at = Instant::now();
        if trigger == LibrarySyncTrigger::Manual {
            self.status_message = "Syncing library in background...".to_owned();
            self.error_message = None;
        }

        thread::spawn(move || {
            let availability = track_paths
                .into_iter()
                .map(|(key, path)| {
                    let is_present = fs::metadata(&path)
                        .map(|metadata| metadata.is_file())
                        .unwrap_or(false);
                    (key, is_present)
                })
                .collect::<BTreeMap<_, _>>();

            let mut folder_cache = BTreeMap::<String, Result<Arc<Vec<PathBuf>>, String>>::new();
            let folder_results = folder_targets
                .into_iter()
                .map(|target| {
                    let key = path_key(&target.source_folder);
                    let files = folder_cache
                        .entry(key)
                        .or_insert_with(|| {
                            collect_audio_files_from_folder(&target.source_folder)
                                .map(Arc::new)
                                .map_err(|error| error.to_string())
                        })
                        .clone();
                    FolderLibrarySyncResult { target, files }
                })
                .collect();

            let _ = sender.send(PendingLibrarySyncResult {
                trigger,
                availability,
                folder_results,
            });
        });
        true
    }

    pub(crate) fn maybe_start_auto_library_sync(&mut self) {
        if !self.state.library.auto_sync_folder_playlists
            || self.last_library_sync_at.elapsed() < AUTO_LIBRARY_SYNC_INTERVAL
        {
            return;
        }

        self.start_library_sync(LibrarySyncTrigger::Automatic, true);
    }

    pub(crate) fn process_library_sync_events(&mut self) {
        let Some(receiver) = &self.pending_library_sync_receiver else {
            return;
        };

        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_library_sync_receiver = None;
                self.error_message = Some("Library sync stopped before returning a result.".to_owned());
                return;
            }
        };
        self.pending_library_sync_receiver = None;
        self.last_library_sync_at = Instant::now();

        let selected_path = self.selected_track_path();
        let mut stats = AppliedLibrarySyncStats::default();
        let mut synced_folder_indexes = BTreeSet::new();
        let mut errors = Vec::new();

        for folder_result in result.folder_results {
            let Some(playlist_index) = resolve_folder_sync_target(&self.state.playlists, &folder_result.target) else {
                continue;
            };

            match folder_result.files {
                Ok(files) => {
                    let Some(playlist) = self.state.playlists.get_mut(playlist_index) else {
                        continue;
                    };
                    let folder_stats = playlist.merge_tracks_from_folder_scan(files.as_ref());
                    stats.added += folder_stats.added;
                    stats.restored += folder_stats.restored;
                    stats.newly_missing += folder_stats.newly_missing;
                    stats.missing_total += folder_stats.missing_total;
                    synced_folder_indexes.insert(playlist_index);
                }
                Err(error) => {
                    errors.push(format!(
                        "{}: {error}",
                        folder_result.target.playlist_name
                    ));
                }
            }
        }

        for (playlist_index, playlist) in self.state.playlists.iter_mut().enumerate() {
            if synced_folder_indexes.contains(&playlist_index) {
                continue;
            }
            let availability_stats = playlist.apply_track_availability(&result.availability);
            stats.restored += availability_stats.restored;
            stats.newly_missing += availability_stats.newly_missing;
        }
        stats.missing_total = self
            .state
            .playlists
            .iter()
            .flat_map(|playlist| playlist.tracks.iter())
            .filter(|track| track.missing)
            .count();

        if stats.changed() {
            self.remap_track_indexes_after_library_change(selected_path);
            self.save_state_silently();
        }

        match result.trigger {
            LibrarySyncTrigger::Manual => {
                self.status_message = format!(
                    "Library synced: {} added, {} restored, {} newly missing, {} missing total.",
                    stats.added, stats.restored, stats.newly_missing, stats.missing_total
                );
                self.error_message = errors.first().map(|error| {
                    if errors.len() == 1 {
                        format!("One folder could not be synced: {error}")
                    } else {
                        format!("{} folders could not be synced. First error: {error}", errors.len())
                    }
                });
            }
            LibrarySyncTrigger::Automatic => {
                if stats.changed() {
                    self.status_message = format!(
                        "Library updated: {} added, {} restored, {} newly missing.",
                        stats.added, stats.restored, stats.newly_missing
                    );
                }
            }
            LibrarySyncTrigger::Startup => {
                if stats.newly_missing > 0 {
                    self.status_message = format!(
                        "Library check found {} missing track(s).",
                        stats.missing_total
                    );
                }
                if self.state.library.auto_sync_folder_playlists {
                    self.last_library_sync_at = Instant::now()
                        .checked_sub(AUTO_LIBRARY_SYNC_INTERVAL)
                        .unwrap_or_else(Instant::now);
                }
            }
        }
    }

    fn remap_track_indexes_after_library_change(&mut self, selected_path: Option<PathBuf>) {
        if let Some(path) = selected_path {
            self.selected_track_index = self
                .current_playlist()
                .and_then(|playlist| playlist.tracks.iter().position(|track| same_path(&track.path, &path)));
        } else if self
            .selected_track_index
            .map(|index| {
                self.current_playlist()
                    .map(|playlist| index >= playlist.tracks.len())
                    .unwrap_or(true)
            })
            .unwrap_or(false)
        {
            self.selected_track_index = self.eligible_track_indexes().first().copied();
        }

        self.active_track_index = self
            .active_playlist_index
            .zip(self.active_track_path.as_ref())
            .and_then(|(playlist_index, active_path)| {
                self.state.playlists.get(playlist_index).and_then(|playlist| {
                    playlist
                        .tracks
                        .iter()
                        .position(|track| same_path(&track.path, active_path))
                })
            });
        self.restore_repeat_selection_for_current_playlist();
        self.ensure_selected_track_visible();
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
                self.pending_library_sync_receiver = None;
                self.state = state;
                self.show_library_panel = self.state.ui.show_library_panel;
                self.show_profile_panel = self.state.ui.show_profile_panel;
                self.player_only_mode = self.state.ui.player_only_mode;
                self.restore_repeat_selection_for_current_playlist();
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!("Imported app backup from {}.", path.display());
                self.error_message = None;
                self.save_state_silently();
                self.start_library_sync(LibrarySyncTrigger::Startup, false);
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }
}

fn resolve_folder_sync_target(
    playlists: &[Playlist],
    target: &FolderLibrarySyncTarget,
) -> Option<usize> {
    let matches_target = |playlist: &Playlist| {
        playlist.kind == PlaylistKind::Folder
            && playlist.name == target.playlist_name
            && playlist
                .source_folder
                .as_ref()
                .map(|source| same_path(source, &target.source_folder))
                .unwrap_or(false)
    };

    if playlists
        .get(target.playlist_index)
        .map(|playlist| matches_target(playlist))
        .unwrap_or(false)
    {
        return Some(target.playlist_index);
    }

    if let Some(index) = playlists.iter().position(|playlist| matches_target(playlist)) {
        return Some(index);
    }

    let mut source_matches = playlists
        .iter()
        .enumerate()
        .filter(|(_, playlist)| {
            playlist.kind == PlaylistKind::Folder
                && playlist
                    .source_folder
                    .as_ref()
                    .map(|source| same_path(source, &target.source_folder))
                    .unwrap_or(false)
        })
        .map(|(index, _)| index);
    let index = source_matches.next()?;
    source_matches.next().is_none().then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_playlist(name: &str, source: &str) -> Playlist {
        Playlist::from_folder(name, PathBuf::from(source), 1, Vec::new())
    }

    #[test]
    fn folder_sync_target_follows_playlist_reordering_and_renaming() {
        let target = FolderLibrarySyncTarget {
            playlist_index: 1,
            playlist_name: "Old name".to_owned(),
            source_folder: PathBuf::from("C:/Music"),
        };
        let playlists = vec![
            folder_playlist("Renamed", "C:/Music"),
            folder_playlist("Other", "C:/Other"),
        ];

        assert_eq!(resolve_folder_sync_target(&playlists, &target), Some(0));
    }

    #[test]
    fn folder_sync_target_rejects_ambiguous_source_fallback() {
        let target = FolderLibrarySyncTarget {
            playlist_index: 4,
            playlist_name: "Removed".to_owned(),
            source_folder: PathBuf::from("C:/Music"),
        };
        let playlists = vec![
            folder_playlist("First", "C:/Music"),
            folder_playlist("Second", "C:/Music"),
        ];

        assert_eq!(resolve_folder_sync_target(&playlists, &target), None);
    }
}
