use crate::*;

const OPEN_REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(500);

impl AudioOrbitApp {
    fn temporary_playlist_index(&mut self) -> usize {
        if let Some(index) = self
            .state
            .playlists
            .iter()
            .position(|playlist| playlist.kind == PlaylistKind::Temporary)
        {
            return index;
        }

        self.state.playlists.push(Playlist::temporary());
        self.state.playlists.len() - 1
    }

    pub(crate) fn open_audio_files_in_temporary_playlist(
        &mut self,
        paths: Vec<PathBuf>,
        autoplay: bool,
    ) {
        let mut valid_paths = Vec::new();
        for path in paths {
            if !path.is_file() || !is_supported_audio_file(&path) {
                continue;
            }
            if !valid_paths
                .iter()
                .any(|existing: &PathBuf| same_path(existing, &path))
            {
                valid_paths.push(path);
            }
        }

        let Some(first_path) = valid_paths.first().cloned() else {
            self.error_message = Some("No supported audio files were opened.".to_owned());
            return;
        };

        if autoplay {
            self.stop();
        }

        let playlist_index = self.temporary_playlist_index();
        if let Some(playlist) = self.state.playlists.get_mut(playlist_index) {
            playlist.add_temporary_files(valid_paths.clone());
        }

        let track_index = self
            .state
            .playlists
            .get(playlist_index)
            .and_then(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .position(|track| same_path(&track.path, &first_path))
            });

        self.active_tab = MainContentTab::Music;
        self.select_playlist(playlist_index);
        self.selected_track_index = track_index;
        self.scroll_to_track_path_requested = Some(first_path.clone());
        self.status_message = if valid_paths.len() == 1 {
            format!(
                "Opened {} in temporary playback.",
                display_file_name(&first_path)
            )
        } else {
            format!("Opened {} files in temporary playback.", valid_paths.len())
        };
        self.error_message = None;

        if autoplay {
            self.play_path(first_path, track_index, 0.0);
        }
    }

    pub(crate) fn process_external_open_requests(&mut self, context: &egui::Context) {
        if self.last_external_open_request_poll.elapsed() < OPEN_REQUEST_POLL_INTERVAL {
            return;
        }
        self.last_external_open_request_poll = Instant::now();

        let Some(request_dir) = single_instance::open_request_dir() else {
            return;
        };
        let Ok(entries) = fs::read_dir(&request_dir) else {
            return;
        };

        let mut request_files = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().and_then(|extension| extension.to_str()) == Some("json")
            })
            .collect::<Vec<_>>();
        request_files.sort();

        let mut paths = Vec::new();
        for request_file in request_files {
            let request_paths = fs::read_to_string(&request_file)
                .ok()
                .and_then(|contents| serde_json::from_str::<Vec<PathBuf>>(&contents).ok())
                .unwrap_or_default();
            let _ = fs::remove_file(&request_file);
            paths.extend(request_paths);
        }

        if !paths.is_empty() {
            self.open_audio_files_in_temporary_playlist(paths, true);
            context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }
}
