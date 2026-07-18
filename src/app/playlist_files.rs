use crate::*;
use std::{
    fs::OpenOptions,
    io::{self, ErrorKind},
};

impl AudioOrbitApp {
    pub(crate) fn track_file_operations_idle(&self) -> bool {
        self.pending_track_file_operation_receiver.is_none()
            && self.pending_folder_scan_receiver.is_none()
            && self.pending_library_sync_receiver.is_none()
    }

    pub(crate) fn export_current_playlist_to_folder(&mut self) {
        if !self.track_file_operations_idle() {
            self.error_message =
                Some("Wait for current library or track file operation to finish.".to_owned());
            return;
        }

        let Some((playlist_name, paths)) = self.current_playlist().map(|playlist| {
            (
                playlist.name.clone(),
                playlist
                    .tracks
                    .iter()
                    .map(|track| track.path.clone())
                    .collect::<Vec<_>>(),
            )
        }) else {
            return;
        };

        if paths.is_empty() {
            self.error_message = Some("Current playlist has no tracks to export.".to_owned());
            return;
        }

        let Some(destination) = FileDialog::new().pick_folder() else {
            return;
        };

        self.start_copy_track_files(paths, destination, format!("playlist {playlist_name}"));
    }

    pub(crate) fn copy_track_selection_to_folder(&mut self, index: usize) {
        if !self.track_file_operations_idle() {
            self.error_message =
                Some("Wait for current library or track file operation to finish.".to_owned());
            return;
        }

        let paths = self.action_track_paths_for_context(index);
        if paths.is_empty() {
            return;
        }

        let Some(destination) = FileDialog::new().pick_folder() else {
            return;
        };

        self.start_copy_track_files(paths, destination, "selected tracks".to_owned());
    }

    pub(crate) fn request_delete_current_playlist_files(&mut self) {
        if !self.track_file_operations_idle() {
            self.error_message =
                Some("Wait for current library or track file operation to finish.".to_owned());
            return;
        }

        let Some((playlist_name, paths)) = self.current_playlist().map(|playlist| {
            (
                playlist.name.clone(),
                playlist
                    .tracks
                    .iter()
                    .map(|track| track.path.clone())
                    .collect::<Vec<_>>(),
            )
        }) else {
            return;
        };

        if paths.is_empty() {
            self.error_message = Some("Current playlist has no tracks to delete.".to_owned());
            return;
        }

        let paths = deduplicate_paths(paths);
        let count = paths.len();
        self.pending_track_delete_confirmation = Some(PendingTrackDeleteConfirmation {
            paths,
            title: "Delete all playlist files?".to_owned(),
            description: format!(
                "Permanently delete {count} file(s) referenced by {playlist_name}. Deleted files are removed from every playlist. This cannot be undone."
            ),
        });
        self.pending_track_delete_confirmation_text.clear();
    }

    pub(crate) fn request_delete_track_selection(&mut self, index: usize) {
        if !self.track_file_operations_idle() {
            self.error_message =
                Some("Wait for current library or track file operation to finish.".to_owned());
            return;
        }

        let paths = self.action_track_paths_for_context(index);
        if paths.is_empty() {
            return;
        }

        let paths = deduplicate_paths(paths);
        let count = paths.len();
        self.pending_track_delete_confirmation = Some(PendingTrackDeleteConfirmation {
            paths,
            title: "Delete selected files?".to_owned(),
            description: format!(
                "Permanently delete {count} selected file(s). Deleted files are removed from every playlist. This cannot be undone."
            ),
        });
        self.pending_track_delete_confirmation_text.clear();
    }

    pub(crate) fn open_new_playlist_modal(&mut self, paths: Vec<PathBuf>) {
        let paths = deduplicate_paths(paths);
        if paths.is_empty() {
            return;
        }

        self.pending_new_playlist_tracks = paths;
        self.pending_new_playlist_name.clear();
        self.show_new_playlist_modal = true;
        self.error_message = None;
    }

    pub(crate) fn render_new_playlist_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "new_playlist_modal_backdrop");
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let mut close = false;
        let mut create = false;
        let track_count = self.pending_new_playlist_tracks.len();

        egui::Area::new(egui::Id::new("new_playlist_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);

                    if Self::render_modal_header(
                        ui,
                        outer_padding.x,
                        Icon::ListPlus,
                        "New playlist",
                        "Create a manual playlist and add the selected track(s).",
                    ) {
                        close = true;
                    }

                    Self::render_modal_section(ui, |ui| {
                        let form_width = ui.available_width().min(640.0);
                        ui.label("Playlist name");
                        let response = ui.add_sized(
                            egui::vec2(form_width, 24.0),
                            egui::TextEdit::singleline(&mut self.pending_new_playlist_name)
                                .hint_text("Playlist name"),
                        );
                        ui.small(format!("{track_count} track(s) will be added."));
                        ui.add_space(10.0);

                        let can_create = !self.pending_new_playlist_name.trim().is_empty();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    can_create,
                                    egui::Button::new(ui_icons::label(
                                        Icon::Plus,
                                        "Create playlist",
                                    )),
                                )
                                .clicked()
                            {
                                create = true;
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });

                        if response.lost_focus()
                            && context.input(|input| input.key_pressed(egui::Key::Enter))
                            && can_create
                        {
                            create = true;
                        }
                    });
                });
            });
        self.render_modal_info_footer_fixed(context, "new_playlist_modal_info_footer", screen_rect);

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        if create && self.create_pending_new_playlist() {
            close = true;
        }

        if close {
            self.show_new_playlist_modal = false;
            self.pending_new_playlist_name.clear();
            self.pending_new_playlist_tracks.clear();
        }
    }

    pub(crate) fn render_track_delete_confirmation_modal(&mut self, context: &egui::Context) {
        let Some(confirmation) = self.pending_track_delete_confirmation.clone() else {
            return;
        };

        self.render_modal_backdrop(context, "track_delete_confirmation_backdrop");
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let mut close = false;
        let mut confirm = false;

        egui::Area::new(egui::Id::new("track_delete_confirmation_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);

                    if Self::render_modal_header(
                        ui,
                        outer_padding.x,
                        Icon::Trash2,
                        &confirmation.title,
                        &confirmation.description,
                    ) {
                        close = true;
                    }

                    Self::render_modal_section(ui, |ui| {
                        let form_width = ui.available_width().min(640.0);
                        ui.label("Type DELETE to confirm permanent file deletion.");
                        ui.add_sized(
                            egui::vec2(form_width, 24.0),
                            egui::TextEdit::singleline(
                                &mut self.pending_track_delete_confirmation_text,
                            )
                            .hint_text("DELETE"),
                        );
                        ui.add_space(10.0);

                        let confirmed =
                            self.pending_track_delete_confirmation_text.trim() == "DELETE";
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    confirmed,
                                    egui::Button::new(ui_icons::label(
                                        Icon::Trash2,
                                        "Delete permanently",
                                    )),
                                )
                                .clicked()
                            {
                                confirm = true;
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
                });
            });
        self.render_modal_info_footer_fixed(
            context,
            "track_delete_confirmation_info_footer",
            screen_rect,
        );

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        if confirm {
            self.pending_track_delete_confirmation = None;
            self.pending_track_delete_confirmation_text.clear();
            self.start_delete_track_files(confirmation.paths);
        } else if close {
            self.pending_track_delete_confirmation = None;
            self.pending_track_delete_confirmation_text.clear();
        }
    }

    pub(crate) fn process_track_file_operation_events(&mut self) {
        let Some(receiver) = &self.pending_track_file_operation_receiver else {
            return;
        };

        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_track_file_operation_receiver = None;
                self.error_message =
                    Some("Track file operation stopped before returning a result.".to_owned());
                return;
            }
        };
        self.pending_track_file_operation_receiver = None;

        match result {
            TrackFileOperationResult::Copy {
                destination,
                requested,
                copied,
                skipped_missing,
                errors,
            } => {
                self.status_message = format!(
                    "Copied {copied} of {requested} track(s) to {}.{}",
                    destination.display(),
                    if skipped_missing > 0 {
                        format!(" Skipped {skipped_missing} missing file(s).")
                    } else {
                        String::new()
                    }
                );
                self.error_message =
                    operation_error_message("Some tracks could not be copied", &errors);
            }
            TrackFileOperationResult::Delete {
                requested,
                deleted,
                already_missing,
                removed_paths,
                errors,
            } => {
                let removed_keys = removed_paths
                    .iter()
                    .map(|path| path_key(path))
                    .collect::<BTreeSet<_>>();

                for playlist in &mut self.state.playlists {
                    playlist
                        .tracks
                        .retain(|track| !removed_keys.contains(&path_key(&track.path)));
                    playlist
                        .repeat_selection
                        .retain(|path| !removed_keys.contains(&path_key(path)));
                    let selected_group = playlist.selected_group.clone();
                    playlist.set_selected_group(selected_group);
                }

                self.clear_multi_track_selection();
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.active_track_index = self
                    .active_playlist_index
                    .zip(self.active_track_path.as_ref())
                    .and_then(|(playlist_index, active_path)| {
                        self.state
                            .playlists
                            .get(playlist_index)
                            .and_then(|playlist| {
                                playlist
                                    .tracks
                                    .iter()
                                    .position(|track| same_path(&track.path, active_path))
                            })
                    });
                self.restore_repeat_selection_for_current_playlist();
                self.status_message = format!(
                    "Deleted {deleted} of {requested} track file(s).{}",
                    if already_missing > 0 {
                        format!(" Removed {already_missing} already-missing reference(s).")
                    } else {
                        String::new()
                    }
                );
                self.error_message =
                    operation_error_message("Some tracks could not be deleted", &errors);
                self.save_state_silently();
            }
        }
    }

    fn create_pending_new_playlist(&mut self) -> bool {
        let name = self.pending_new_playlist_name.trim().to_owned();
        if name.is_empty() {
            self.error_message = Some("Enter a playlist name.".to_owned());
            return false;
        }
        if self
            .state
            .playlists
            .iter()
            .any(|playlist| playlist.name.eq_ignore_ascii_case(&name))
        {
            self.error_message = Some("A playlist with this name already exists.".to_owned());
            return false;
        }

        let mut playlist = Playlist::new(name.clone());
        let requested = self.pending_new_playlist_tracks.len();
        let added = self
            .pending_new_playlist_tracks
            .iter()
            .filter(|path| playlist.add_track_path((*path).clone(), None, 0))
            .count();
        self.state.playlists.push(playlist);
        self.status_message = format!("Created {name} and added {added} of {requested} track(s).");
        self.error_message = None;
        self.save_state_silently();
        true
    }

    fn start_copy_track_files(&mut self, paths: Vec<PathBuf>, destination: PathBuf, label: String) {
        let paths = deduplicate_paths(paths);
        let requested = paths.len();
        let (sender, receiver) = mpsc::channel();
        self.pending_track_file_operation_receiver = Some(receiver);
        self.status_message = format!(
            "Copying {requested} track(s) from {label} to {}...",
            destination.display()
        );
        self.error_message = None;

        thread::spawn(move || {
            let mut copied = 0usize;
            let mut skipped_missing = 0usize;
            let mut errors = Vec::new();

            if let Err(error) = fs::create_dir_all(&destination) {
                errors.push(format!(
                    "Failed to create destination {}: {error}",
                    destination.display()
                ));
            } else {
                for source in paths {
                    if !source.is_file() {
                        skipped_missing += 1;
                        continue;
                    }

                    match copy_file_with_unique_name(&destination, &source) {
                        Ok(_) => copied += 1,
                        Err(error) => errors.push(error),
                    }
                }
            }

            let _ = sender.send(TrackFileOperationResult::Copy {
                destination,
                requested,
                copied,
                skipped_missing,
                errors,
            });
        });
    }

    fn start_delete_track_files(&mut self, paths: Vec<PathBuf>) {
        if !self.track_file_operations_idle() {
            self.error_message =
                Some("Wait for current library or track file operation to finish.".to_owned());
            return;
        }

        let paths = deduplicate_paths(paths);
        if paths.is_empty() {
            return;
        }

        if self
            .active_track_path
            .as_ref()
            .is_some_and(|active_path| paths.iter().any(|path| same_path(path, active_path)))
        {
            self.stop();
        }

        let requested = paths.len();
        let (sender, receiver) = mpsc::channel();
        self.pending_track_file_operation_receiver = Some(receiver);
        self.status_message = format!("Deleting {requested} track file(s)...");
        self.error_message = None;

        thread::spawn(move || {
            let mut deleted = 0usize;
            let mut already_missing = 0usize;
            let mut removed_paths = Vec::new();
            let mut errors = Vec::new();

            for path in paths {
                match fs::remove_file(&path) {
                    Ok(()) => {
                        deleted += 1;
                        removed_paths.push(path);
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        already_missing += 1;
                        removed_paths.push(path);
                    }
                    Err(error) => {
                        errors.push(format!("Failed to delete {}: {error}", path.display()))
                    }
                }
            }

            let _ = sender.send(TrackFileOperationResult::Delete {
                requested,
                deleted,
                already_missing,
                removed_paths,
                errors,
            });
        });
    }
}

fn deduplicate_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path_key(path)))
        .collect()
}

fn copy_file_with_unique_name(destination: &Path, source: &Path) -> Result<PathBuf, String> {
    let original_file_name = source
        .file_name()
        .ok_or_else(|| format!("Track path has no file name: {}", source.display()))?;
    let stem = source
        .file_stem()
        .map(|value| value.to_string_lossy())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "track".into());
    let extension = source.extension().map(|value| value.to_string_lossy());
    let mut input = fs::File::open(source)
        .map_err(|error| format!("Failed to open {} for copying: {error}", source.display()))?;
    let source_permissions = input.metadata().ok().map(|metadata| metadata.permissions());

    for suffix in 1usize.. {
        let candidate = match (suffix, extension.as_deref()) {
            (1, _) => destination.join(original_file_name),
            (_, Some(extension)) if !extension.is_empty() => {
                destination.join(format!("{stem} ({suffix}).{extension}"))
            }
            _ => destination.join(format!("{stem} ({suffix})")),
        };
        let mut output = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(output) => output,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Failed to create {} while copying {}: {error}",
                    candidate.display(),
                    source.display()
                ));
            }
        };

        if let Err(error) = io::copy(&mut input, &mut output) {
            drop(output);
            let _ = fs::remove_file(&candidate);
            return Err(format!(
                "Failed to copy {} to {}: {error}",
                source.display(),
                candidate.display()
            ));
        }

        drop(output);
        if let Some(permissions) = source_permissions.clone() {
            let _ = fs::set_permissions(&candidate, permissions);
        }
        return Ok(candidate);
    }

    unreachable!()
}

fn operation_error_message(prefix: &str, errors: &[String]) -> Option<String> {
    if errors.is_empty() {
        return None;
    }

    let details = errors
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    let remaining = errors.len().saturating_sub(3);
    Some(if remaining == 0 {
        format!("{prefix}: {details}")
    } else {
        format!("{prefix}: {details} | and {remaining} more error(s)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicate_paths_uses_normalized_path_keys() {
        let paths = vec![
            PathBuf::from("music/song.mp3"),
            PathBuf::from("music/song.mp3"),
        ];
        assert_eq!(deduplicate_paths(paths).len(), 1);
    }

    #[test]
    fn copy_file_with_unique_name_adds_suffix_for_existing_file() {
        let root = std::env::temp_dir().join(format!(
            "audio-orbit-copy-target-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source_folder = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source_folder).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let source = source_folder.join("song.mp3");
        fs::write(&source, b"source").unwrap();
        fs::write(destination.join("song.mp3"), b"existing").unwrap();

        let target = copy_file_with_unique_name(&destination, &source).unwrap();
        assert_eq!(
            target.file_name().and_then(|name| name.to_str()),
            Some("song (2).mp3")
        );
        assert_eq!(fs::read(target).unwrap(), b"source");

        fs::remove_dir_all(root).unwrap();
    }
}
