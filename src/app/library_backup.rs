use crate::*;

#[derive(Clone, Copy, Debug, Default)]
struct AppliedLibrarySyncStats {
    added: usize,
    restored: usize,
    newly_missing: usize,
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

    pub(crate) fn start_folder_scan(&mut self, kind: PendingFolderScanKind) -> bool {
        if self.pending_folder_scan_receiver.is_some() || self.pending_library_sync_receiver.is_some() {
            self.error_message = Some("A library scan is already running. Wait for it to finish before starting another scan.".to_owned());
            return false;
        }

        let PendingFolderScanKind::Import { folder, .. } = &kind;
        let folder = folder.clone();
        let (sender, receiver) = mpsc::channel();
        self.pending_folder_scan_receiver = Some(receiver);
        self.error_message = None;

        thread::spawn(move || {
            let result = scan_audio_folder(&folder)
                .map(|scan| PendingFolderScanResult {
                    kind,
                    files: scan.files,
                })
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

        let PendingFolderScanKind::Import { name, folder, depth } = result.kind;
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

    pub(crate) fn start_library_sync(
        &mut self,
        trigger: LibrarySyncTrigger,
        include_folder_scan: bool,
    ) -> bool {
        if self.pending_folder_scan_receiver.is_some() || self.pending_library_sync_receiver.is_some() {
            if trigger == LibrarySyncTrigger::Manual {
                self.error_message = Some("A playlist scan is already running.".to_owned());
            }
            return false;
        }

        let Some(playlist) = self.current_playlist() else {
            return false;
        };
        let playlist_index = self.state.selected_playlist_index;
        let playlist_name = playlist.name.clone();
        let playlist_kind = playlist.kind.clone();
        let source_folder = playlist.source_folder.clone();
        let mut track_paths = BTreeMap::<String, PathBuf>::new();
        let mut folder_targets = Vec::new();

        if include_folder_scan && playlist_kind == PlaylistKind::Folder {
            if let Some(source_folder) = source_folder.clone() {
                folder_targets.push(FolderLibrarySyncTarget {
                    playlist_index,
                    playlist_name: playlist_name.clone(),
                    source_folder,
                });
            }
        } else {
            for track in &playlist.tracks {
                track_paths
                    .entry(path_key(&track.path))
                    .or_insert_with(|| track.path.clone());
            }
        }

        if track_paths.is_empty() && folder_targets.is_empty() {
            if trigger == LibrarySyncTrigger::Manual {
                self.status_message = format!("{playlist_name} has no tracks or folder to sync.");
                self.error_message = None;
            }
            return false;
        }

        let (sender, receiver) = mpsc::channel();
        self.pending_library_sync_receiver = Some(receiver);
        if trigger == LibrarySyncTrigger::Manual {
            self.status_message = format!("Syncing {playlist_name} in background...");
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

            let folder_results = folder_targets
                .into_iter()
                .map(|target| {
                    let outcome = scan_audio_folder(&target.source_folder)
                        .map(|scan| FolderLibrarySyncOutcome::Scanned {
                            files: Arc::new(scan.files),
                        })
                        .map_err(|error| error.to_string());
                    FolderLibrarySyncResult { target, outcome }
                })
                .collect();

            let _ = sender.send(PendingLibrarySyncResult {
                trigger,
                playlist_index,
                playlist_name,
                playlist_kind,
                source_folder,
                availability,
                folder_results,
            });
        });
        true
    }

    fn start_incremental_folder_sync(
        &mut self,
        changed_paths: Vec<PendingFolderWatchChange>,
    ) -> bool {
        if self.pending_folder_scan_receiver.is_some() || self.pending_library_sync_receiver.is_some() {
            return false;
        }

        let Some(playlist) = self.current_playlist() else {
            return false;
        };
        if playlist.kind != PlaylistKind::Folder {
            return false;
        }
        let Some(source_folder) = playlist.source_folder.clone() else {
            return false;
        };
        if changed_paths.is_empty() {
            return false;
        }

        let playlist_index = self.state.selected_playlist_index;
        let playlist_name = playlist.name.clone();
        let playlist_kind = playlist.kind.clone();
        let target = FolderLibrarySyncTarget {
            playlist_index,
            playlist_name: playlist_name.clone(),
            source_folder: source_folder.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        self.pending_library_sync_receiver = Some(receiver);

        thread::spawn(move || {
            let mut present_files = BTreeMap::<String, PathBuf>::new();
            let mut missing_roots = BTreeMap::<String, PathBuf>::new();
            let mut errors = Vec::new();

            for change in changed_paths {
                let path = change.path;
                if !path_is_same_or_descendant(&path, &source_folder) {
                    continue;
                }

                match fs::symlink_metadata(&path) {
                    Ok(metadata) => {
                        let file_type = metadata.file_type();
                        if is_recursive_scan_link(&path, &file_type) {
                            continue;
                        }

                        if file_type.is_dir() && change.scan_directory_if_present {
                            match scan_audio_folder(&path) {
                                Ok(scan) => {
                                    for file in scan.files {
                                        present_files.entry(path_key(&file)).or_insert(file);
                                    }
                                }
                                Err(error) => errors.push(format!("{}: {error}", path.display())),
                            }
                        } else if file_type.is_file() && is_supported_audio_file(&path) {
                            present_files.entry(path_key(&path)).or_insert(path);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        missing_roots.entry(path_key(&path)).or_insert(path);
                    }
                    Err(error) => errors.push(format!("{}: {error}", path.display())),
                }
            }

            let mut compact_missing_roots = Vec::<PathBuf>::new();
            let mut ordered_missing_roots = missing_roots.into_values().collect::<Vec<_>>();
            ordered_missing_roots.sort_by_key(|path| path.components().count());
            for path in ordered_missing_roots {
                if compact_missing_roots
                    .iter()
                    .any(|root| path_is_same_or_descendant(&path, root))
                {
                    continue;
                }
                compact_missing_roots.push(path);
            }

            let folder_result = FolderLibrarySyncResult {
                target,
                outcome: Ok(FolderLibrarySyncOutcome::Incremental {
                    present_files: Arc::new(present_files.into_values().collect()),
                    missing_roots: Arc::new(compact_missing_roots),
                    errors,
                }),
            };
            let _ = sender.send(PendingLibrarySyncResult {
                trigger: LibrarySyncTrigger::Automatic,
                playlist_index,
                playlist_name,
                playlist_kind,
                source_folder: Some(source_folder),
                availability: BTreeMap::new(),
                folder_results: vec![folder_result],
            });
        });
        true
    }

    pub(crate) fn refresh_selected_folder_watcher(&mut self, context: &egui::Context) {
        let desired_root = if self.state.library.auto_sync_selected_playlist {
            self.current_playlist()
                .filter(|playlist| playlist.kind == PlaylistKind::Folder)
                .and_then(|playlist| playlist.source_folder.clone())
        } else {
            None
        };
        let desired_key = desired_root.as_ref().map(|root| path_key(root));

        if self.folder_watcher_target_key == desired_key {
            return;
        }

        self.folder_watcher = None;
        self.pending_folder_watch_sync_at = None;
        self.pending_folder_watch_paths.clear();
        self.pending_folder_watch_full_rescan = false;
        self.folder_watcher_target_key = desired_key;

        let Some(root) = desired_root else {
            return;
        };

        match folder_watcher::FolderWatcher::start(root.clone(), context.clone()) {
            Ok(watcher) => {
                self.folder_watcher = Some(watcher);
            }
            Err(error) => {
                self.error_message = Some(format!(
                    "Automatic folder watching could not start for {}: {error}",
                    root.display()
                ));
            }
        }
    }

    pub(crate) fn process_folder_watch_events(&mut self) {
        let mut paths = Vec::new();
        let mut overflow = false;
        let mut failure = None;
        if let Some(watcher) = &self.folder_watcher {
            while let Some(event) = watcher.try_recv() {
                match event {
                    folder_watcher::FolderWatchEvent::Changes(changed_paths) => {
                        paths.extend(changed_paths);
                    }
                    folder_watcher::FolderWatchEvent::Overflow => overflow = true,
                    folder_watcher::FolderWatchEvent::Failed(error) => failure = Some(error),
                }
            }
        }

        for change in paths {
            let key = path_key(&change.path);
            self.pending_folder_watch_paths
                .entry(key)
                .and_modify(|pending| {
                    pending.scan_directory_if_present |= change.scan_directory_if_present;
                })
                .or_insert(PendingFolderWatchChange {
                    path: change.path,
                    scan_directory_if_present: change.scan_directory_if_present,
                });
        }
        if overflow {
            self.pending_folder_watch_full_rescan = true;
            self.pending_folder_watch_paths.clear();
        }
        if !self.pending_folder_watch_paths.is_empty() || self.pending_folder_watch_full_rescan {
            self.pending_folder_watch_sync_at = Some(Instant::now() + FOLDER_WATCH_DEBOUNCE);
        }

        if let Some(error) = failure {
            self.folder_watcher = None;
            self.error_message = Some(error);
            if let Some(root) = self
                .current_playlist()
                .and_then(|playlist| playlist.source_folder.clone())
            {
                self.pending_folder_watch_paths
                    .entry(path_key(&root))
                    .or_insert(PendingFolderWatchChange {
                        path: root,
                        scan_directory_if_present: false,
                    });
                self.pending_folder_watch_sync_at = Some(Instant::now() + FOLDER_WATCH_DEBOUNCE);
            }
        }
    }

    pub(crate) fn maybe_start_auto_library_sync(&mut self) {
        let Some(run_at) = self.pending_folder_watch_sync_at else {
            return;
        };
        if Instant::now() < run_at
            || !self.state.library.auto_sync_selected_playlist
            || self
                .current_playlist()
                .map(|playlist| playlist.kind != PlaylistKind::Folder)
                .unwrap_or(true)
        {
            return;
        }

        if self.pending_folder_watch_full_rescan {
            if self.start_library_sync(LibrarySyncTrigger::Automatic, true) {
                self.pending_folder_watch_full_rescan = false;
                self.pending_folder_watch_paths.clear();
                self.pending_folder_watch_sync_at = None;
            }
            return;
        }

        let changed_paths = self
            .pending_folder_watch_paths
            .values()
            .cloned()
            .collect::<Vec<_>>();
        if self.start_incremental_folder_sync(changed_paths) {
            self.pending_folder_watch_paths.clear();
            self.pending_folder_watch_sync_at = None;
        }
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
                self.error_message = Some("Playlist sync stopped before returning a result.".to_owned());
                return;
            }
        };
        self.pending_library_sync_receiver = None;

        let PendingLibrarySyncResult {
            trigger,
            playlist_index,
            playlist_name,
            playlist_kind,
            source_folder,
            availability,
            folder_results,
        } = result;
        let selected_path = self.selected_track_path();
        let mut stats = AppliedLibrarySyncStats::default();
        let mut errors = Vec::new();

        for folder_result in folder_results {
            match folder_result.outcome {
                Ok(FolderLibrarySyncOutcome::Scanned { files }) => {
                    let Some(resolved_index) = resolve_folder_sync_target(
                        &self.state.playlists,
                        &folder_result.target,
                    ) else {
                        continue;
                    };
                    let Some(playlist) = self.state.playlists.get_mut(resolved_index) else {
                        continue;
                    };
                    let folder_stats = playlist.merge_tracks_from_folder_scan(files.as_ref());
                    stats.added += folder_stats.added;
                    stats.restored += folder_stats.restored;
                    stats.newly_missing += folder_stats.newly_missing;
                }
                Ok(FolderLibrarySyncOutcome::Incremental {
                    present_files,
                    missing_roots,
                    errors: incremental_errors,
                }) => {
                    let Some(resolved_index) = resolve_folder_sync_target(
                        &self.state.playlists,
                        &folder_result.target,
                    ) else {
                        continue;
                    };
                    let Some(playlist) = self.state.playlists.get_mut(resolved_index) else {
                        continue;
                    };
                    let folder_stats = playlist.merge_tracks_from_folder_changes(
                        present_files.as_ref(),
                        missing_roots.as_ref(),
                    );
                    stats.added += folder_stats.added;
                    stats.restored += folder_stats.restored;
                    stats.newly_missing += folder_stats.newly_missing;
                    errors.extend(incremental_errors);
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", folder_result.target.playlist_name));
                }
            }
        }

        if !availability.is_empty() {
            if let Some(resolved_index) = resolve_playlist_sync_target(
                &self.state.playlists,
                playlist_index,
                &playlist_name,
                &playlist_kind,
                source_folder.as_deref(),
            ) {
                if let Some(playlist) = self.state.playlists.get_mut(resolved_index) {
                    let availability_stats = playlist.apply_track_availability(&availability);
                    stats.restored += availability_stats.restored;
                    stats.newly_missing += availability_stats.newly_missing;
                }
            }
        }

        if stats.changed() {
            self.remap_track_indexes_after_library_change(selected_path);
            self.save_state_silently();
        }

        match trigger {
            LibrarySyncTrigger::Manual => {
                self.status_message = if stats.changed() {
                    format!(
                        "Synced {}: {} added, {} restored, {} newly missing.",
                        playlist_name, stats.added, stats.restored, stats.newly_missing
                    )
                } else {
                    format!("Synced {playlist_name}. No changes.")
                };
                self.error_message = errors.first().map(|error| {
                    if errors.len() == 1 {
                        format!("Folder could not be synced: {error}")
                    } else {
                        format!("{} folders could not be synced. First error: {error}", errors.len())
                    }
                });
            }
            LibrarySyncTrigger::Automatic => {
                if stats.changed() {
                    self.status_message = format!(
                        "{} updated: {} added, {} restored, {} newly missing.",
                        playlist_name, stats.added, stats.restored, stats.newly_missing
                    );
                }
            }
            LibrarySyncTrigger::Startup => {
                if stats.changed() {
                    self.status_message = format!(
                        "{} checked: {} added, {} restored, {} newly missing.",
                        playlist_name, stats.added, stats.restored, stats.newly_missing
                    );
                }
            }
        }

        if trigger != LibrarySyncTrigger::Manual && !errors.is_empty() {
            self.error_message = Some(if errors.len() == 1 {
                format!("Playlist could not be fully synced: {}", errors[0])
            } else {
                format!(
                    "Playlist could not be fully synced. {} errors occurred. First error: {}",
                    errors.len(),
                    errors[0]
                )
            });
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
                self.folder_watcher = None;
                self.folder_watcher_target_key = None;
                self.pending_folder_watch_sync_at = None;
                self.pending_folder_watch_paths.clear();
                self.pending_folder_watch_full_rescan = false;
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

fn resolve_playlist_sync_target(
    playlists: &[Playlist],
    target_index: usize,
    target_name: &str,
    target_kind: &PlaylistKind,
    target_source_folder: Option<&Path>,
) -> Option<usize> {
    let matches_target = |playlist: &Playlist| {
        if &playlist.kind != target_kind || playlist.name != target_name {
            return false;
        }
        match (playlist.source_folder.as_deref(), target_source_folder) {
            (Some(left), Some(right)) => same_path(left, right),
            (None, None) => true,
            _ => false,
        }
    };

    if playlists
        .get(target_index)
        .map(|playlist| matches_target(playlist))
        .unwrap_or(false)
    {
        return Some(target_index);
    }

    let mut matches = playlists
        .iter()
        .enumerate()
        .filter(|(_, playlist)| matches_target(playlist))
        .map(|(index, _)| index);
    let index = matches.next()?;
    matches.next().is_none().then_some(index)
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
