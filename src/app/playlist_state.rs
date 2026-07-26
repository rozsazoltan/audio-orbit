use crate::*;

impl AudioOrbitApp {
    pub(crate) fn current_playlist(&self) -> Option<&Playlist> {
        self.state.playlists.get(self.state.selected_playlist_index)
    }
    pub(crate) fn current_playlist_mut(&mut self) -> Option<&mut Playlist> {
        self.state
            .playlists
            .get_mut(self.state.selected_playlist_index)
    }
    pub(crate) fn select_playlist(&mut self, index: usize) {
        if index >= self.state.playlists.len() {
            return;
        }

        self.persist_repeat_selection_for_current_playlist();
        self.remember_current_playlist_scroll_offset(self.state.ui.playlist_scroll_offset_y);
        self.state.selected_playlist_index = index;
        self.restore_repeat_selection_for_current_playlist();
        self.clear_multi_track_selection();
        self.selected_track_index = self.eligible_track_indexes().first().copied();
        self.collapsed_groups.clear();
        self.scroll_to_track_path_requested = None;
        self.search_cursor = 0;
        self.folder_watcher = None;
        self.folder_watcher_target_key = None;
        self.pending_folder_watch_sync_at = None;
        self.pending_folder_watch_paths.clear();
        self.pending_folder_watch_full_rescan = false;
        self.save_state_silently();
    }
    pub(crate) fn jump_to_track_in_playlist(
        &mut self,
        playlist_index: usize,
        expected_path: PathBuf,
    ) {
        let Some((track_path, playlist_name)) =
            self.state
                .playlists
                .get(playlist_index)
                .and_then(|playlist| {
                    playlist
                        .tracks
                        .iter()
                        .find(|track| same_path(&track.path, &expected_path))
                        .map(|track| (track.path.clone(), playlist.name.clone()))
                })
        else {
            return;
        };

        self.select_playlist(playlist_index);
        self.track_search_query.clear();
        self.search_cursor = 0;
        self.scroll_to_active_track_requested = false;
        self.scroll_to_folder_group_requested = None;
        if let Some(playlist) = self.state.playlists.get_mut(playlist_index) {
            playlist.set_selected_group(None);
        }
        self.selected_track_index = self
            .state
            .playlists
            .get(playlist_index)
            .and_then(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .position(|track| same_path(&track.path, &track_path))
            });
        self.clear_multi_track_selection();
        self.scroll_to_track_path_requested = Some(track_path.clone());
        self.status_message = format!(
            "Found {} in {playlist_name}.",
            display_file_name(&track_path)
        );
        self.save_state_silently();
    }

    pub(crate) fn restore_repeat_selection_for_current_playlist(&mut self) {
        let selected_indexes = self
            .current_playlist()
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .enumerate()
                    .filter_map(|(index, track)| {
                        playlist
                            .repeat_selection
                            .iter()
                            .any(|selected_path| {
                                same_path(selected_path.as_path(), track.path.as_path())
                            })
                            .then_some(index)
                    })
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();

        self.selected_track_indexes = selected_indexes;
    }
    pub(crate) fn persist_repeat_selection_for_current_playlist(&mut self) {
        let selected_paths = self
            .current_playlist()
            .map(|playlist| {
                self.selected_track_indexes
                    .iter()
                    .filter_map(|index| playlist.tracks.get(*index).map(|track| track.path.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some(playlist) = self.current_playlist_mut() {
            playlist.repeat_selection = selected_paths;
        }
    }
    pub(crate) fn current_playlist_scroll_key(&self) -> Option<String> {
        self.state
            .playlists
            .get(self.state.selected_playlist_index)
            .map(|playlist| playlist_scroll_key(self.state.selected_playlist_index, playlist))
    }
    pub(crate) fn current_playlist_scroll_offset_y(&self) -> f32 {
        self.current_playlist_scroll_key()
            .and_then(|key| self.state.ui.playlist_scroll_offsets.get(&key).copied())
            .unwrap_or(self.state.ui.playlist_scroll_offset_y)
            .max(0.0)
    }
    pub(crate) fn remember_current_playlist_scroll_offset(&mut self, offset_y: f32) {
        let offset_y = offset_y.max(0.0);
        self.state.ui.playlist_scroll_offset_y = offset_y;
        if let Some(key) = self.current_playlist_scroll_key() {
            self.state.ui.playlist_scroll_offsets.insert(key, offset_y);
        }
    }
    pub(crate) fn current_settings(&self) -> DspSettings {
        self.state
            .profiles
            .get(self.state.selected_profile_index)
            .map(|profile| profile.settings)
            .unwrap_or_default()
    }
    pub(crate) fn eligible_track_indexes(&self) -> Vec<usize> {
        self.current_playlist()
            .map(|playlist| {
                playlist
                    .filtered_track_indexes()
                    .into_iter()
                    .filter(|index| {
                        playlist
                            .tracks
                            .get(*index)
                            .map(|track| !track.missing)
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub(crate) fn visible_track_indexes(&self) -> Vec<usize> {
        let query = self.track_search_query.trim();
        let Some(playlist) = self.current_playlist() else {
            return Vec::new();
        };

        playlist
            .filtered_track_indexes()
            .into_iter()
            .filter(|index| {
                playlist
                    .tracks
                    .get(*index)
                    .map(|track| track_matches_search_query(track, query))
                    .unwrap_or(false)
            })
            .collect()
    }
    pub(crate) fn playback_sequence_indexes(&self) -> Vec<usize> {
        let restrict_to_search = self.show_track_search
            && self.search_playback_filtered_only
            && !self.track_search_query.trim().is_empty();
        let indexes = if restrict_to_search {
            self.visible_track_indexes()
        } else {
            self.eligible_track_indexes()
        };
        let indexes = indexes
            .into_iter()
            .filter(|index| {
                self.current_playlist()
                    .and_then(|playlist| playlist.tracks.get(*index))
                    .map(|track| !track.missing)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        if self.state.playback.repeat_mode == RepeatMode::Selection
            && !self.selected_track_indexes.is_empty()
        {
            indexes
                .into_iter()
                .filter(|index| self.selected_track_indexes.contains(index))
                .collect()
        } else {
            indexes
        }
    }
    pub(crate) fn selected_track_path(&self) -> Option<PathBuf> {
        let playlist = self.current_playlist()?;
        let index = self.selected_track_index?;
        playlist.tracks.get(index).map(|track| track.path.clone())
    }
    pub(crate) fn clear_multi_track_selection(&mut self) {
        self.multi_selected_track_indexes.clear();
        self.track_selection_anchor_index = None;
    }
    pub(crate) fn select_only_track_for_action(&mut self, index: usize) {
        let is_valid = self
            .current_playlist()
            .map(|playlist| index < playlist.tracks.len())
            .unwrap_or(false);
        if !is_valid {
            return;
        }

        self.multi_selected_track_indexes.clear();
        self.multi_selected_track_indexes.insert(index);
        self.track_selection_anchor_index = Some(index);
        self.selected_track_index = Some(index);
    }
    pub(crate) fn select_track_from_pointer(&mut self, index: usize, modifiers: egui::Modifiers) {
        let is_valid = self
            .current_playlist()
            .map(|playlist| index < playlist.tracks.len())
            .unwrap_or(false);
        if !is_valid {
            return;
        }

        let additive = modifiers.ctrl || modifiers.command;
        if modifiers.shift {
            let visible_indexes = self.visible_track_indexes();
            let anchor = self.track_selection_anchor_index.unwrap_or(index);
            let anchor_position = visible_indexes
                .iter()
                .position(|candidate| *candidate == anchor);
            let target_position = visible_indexes
                .iter()
                .position(|candidate| *candidate == index);

            if let (Some(anchor_position), Some(target_position)) =
                (anchor_position, target_position)
            {
                if !additive {
                    self.multi_selected_track_indexes.clear();
                }
                let start = anchor_position.min(target_position);
                let end = anchor_position.max(target_position);
                self.multi_selected_track_indexes
                    .extend(visible_indexes[start..=end].iter().copied());
            } else {
                self.multi_selected_track_indexes.clear();
                self.multi_selected_track_indexes.insert(index);
                self.track_selection_anchor_index = Some(index);
            }
            self.selected_track_index = Some(index);
            return;
        }

        if additive {
            if !self.multi_selected_track_indexes.insert(index) {
                self.multi_selected_track_indexes.remove(&index);
            }
            self.selected_track_index = self
                .multi_selected_track_indexes
                .iter()
                .next_back()
                .copied();
            self.track_selection_anchor_index = Some(index);
            return;
        }

        self.select_only_track_for_action(index);
    }
    pub(crate) fn ensure_track_selected_for_context(&mut self, index: usize) {
        if !self.multi_selected_track_indexes.contains(&index) {
            self.select_only_track_for_action(index);
        } else {
            self.selected_track_index = Some(index);
        }
    }
    pub(crate) fn action_track_indexes_for_context(&self, index: usize) -> Vec<usize> {
        let Some(playlist) = self.current_playlist() else {
            return Vec::new();
        };

        if self.multi_selected_track_indexes.contains(&index) {
            self.multi_selected_track_indexes
                .iter()
                .copied()
                .filter(|candidate| *candidate < playlist.tracks.len())
                .collect()
        } else if index < playlist.tracks.len() {
            vec![index]
        } else {
            Vec::new()
        }
    }
    pub(crate) fn action_track_paths_for_context(&self, index: usize) -> Vec<PathBuf> {
        let Some(playlist) = self.current_playlist() else {
            return Vec::new();
        };

        self.action_track_indexes_for_context(index)
            .into_iter()
            .filter_map(|track_index| {
                playlist
                    .tracks
                    .get(track_index)
                    .map(|track| track.path.clone())
            })
            .collect()
    }
    pub(crate) fn remember_last_played_track(&mut self, index: Option<usize>, path: &Path) {
        let Some(track_index) = index else {
            return;
        };
        self.state.last_played_track = Some(LastPlayedTrack {
            playlist_index: self
                .active_playlist_index
                .unwrap_or(self.state.selected_playlist_index),
            track_path: path.to_path_buf(),
        });
        self.selected_track_index = Some(track_index);
        self.save_state_silently();
    }
    pub(crate) fn restore_last_played_track_selection(&mut self) {
        let Some(last_played) = self.state.last_played_track.clone() else {
            return;
        };
        let Some(playlist) = self.state.playlists.get(last_played.playlist_index) else {
            return;
        };
        let Some(track_index) = playlist
            .tracks
            .iter()
            .position(|track| same_path(&track.path, &last_played.track_path))
        else {
            return;
        };
        self.state.selected_playlist_index = last_played.playlist_index;
        self.selected_track_index = Some(track_index);
        self.scroll_to_active_track_requested = true;
    }
    pub(crate) fn restore_saved_playback_session(&mut self) {
        let session = self.state.playback_session.clone();
        match session.source.as_str() {
            "radio" => {
                let Some(radio_index) = session.radio_index.or(self.state.selected_radio_index)
                else {
                    return;
                };
                if radio_index >= self.state.radio_stations.len() {
                    return;
                }
                self.active_tab = MainContentTab::Radio;
                self.state.selected_radio_index = Some(radio_index);
                self.radio_selection_was_user_set = true;
                self.scroll_to_active_radio_requested = true;
                if session.was_active && !session.was_paused {
                    self.play_radio_station(radio_index);
                }
            }
            "track" | "music" => {
                let Some((playlist_index, track_index, path)) = self.find_session_track(&session)
                else {
                    return;
                };
                self.active_tab = MainContentTab::Music;
                self.state.selected_playlist_index = playlist_index;
                self.selected_track_index = Some(track_index);
                self.scroll_to_active_track_requested = true;
                if session.was_active && !session.was_paused {
                    let start_seconds = session.position_seconds.max(0.0);
                    self.play_path(path, Some(track_index), start_seconds);
                }
            }
            _ => {}
        }
    }
    pub(crate) fn find_session_track(
        &self,
        session: &PlaybackSession,
    ) -> Option<(usize, usize, PathBuf)> {
        let session_path = session.track_path.as_ref().or_else(|| {
            self.state
                .last_played_track
                .as_ref()
                .map(|track| &track.track_path)
        })?;
        let preferred_playlist = session.playlist_index.or_else(|| {
            self.state
                .last_played_track
                .as_ref()
                .map(|track| track.playlist_index)
        });

        if let Some(playlist_index) = preferred_playlist {
            if let Some(playlist) = self.state.playlists.get(playlist_index) {
                if let Some(track_index) = playlist
                    .tracks
                    .iter()
                    .position(|track| !track.missing && same_path(&track.path, session_path))
                {
                    return Some((
                        playlist_index,
                        track_index,
                        playlist.tracks[track_index].path.clone(),
                    ));
                }
            }
        }

        for (playlist_index, playlist) in self.state.playlists.iter().enumerate() {
            if let Some(track_index) = playlist
                .tracks
                .iter()
                .position(|track| !track.missing && same_path(&track.path, session_path))
            {
                return Some((
                    playlist_index,
                    track_index,
                    playlist.tracks[track_index].path.clone(),
                ));
            }
        }

        None
    }
    pub(crate) fn persist_playback_session(&mut self) {
        let mut session = PlaybackSession::default();
        session.source = match self.active_tab {
            MainContentTab::Radio => "radio".to_owned(),
            MainContentTab::Music => "music".to_owned(),
        };

        let radio_candidate = if self.active_radio_index.is_some()
            || (self.active_track_path.is_none() && self.active_tab == MainContentTab::Radio)
        {
            self.active_radio_index.or(self.state.selected_radio_index)
        } else {
            None
        };

        if let Some(radio_index) = radio_candidate {
            let is_active = self
                .player
                .as_ref()
                .map(|player| player.is_playing() || player.is_paused())
                .unwrap_or(false)
                && self.active_radio_index.is_some();
            session.source = "radio".to_owned();
            session.was_active = is_active;
            session.was_paused = self
                .player
                .as_ref()
                .map(|player| player.is_paused())
                .unwrap_or(false);
            session.radio_index = Some(radio_index);
            self.state.selected_radio_index = Some(radio_index);
            self.state.playback_session = session;
            return;
        }

        let track_path = self
            .active_track_path
            .clone()
            .or_else(|| self.selected_track_path())
            .or_else(|| {
                self.state
                    .last_played_track
                    .as_ref()
                    .map(|track| track.track_path.clone())
            });
        if let Some(path) = track_path {
            let playlist_index = self
                .active_playlist_index
                .or_else(|| {
                    self.state
                        .last_played_track
                        .as_ref()
                        .map(|track| track.playlist_index)
                })
                .unwrap_or(self.state.selected_playlist_index);
            let is_active = self
                .player
                .as_ref()
                .map(|player| player.is_playing() || player.is_paused())
                .unwrap_or(false)
                && self.active_track_path.is_some();
            let player_is_paused = self
                .player
                .as_ref()
                .map(|player| player.is_paused())
                .unwrap_or(false);
            let preserved_paused_session_position = (!is_active
                && self.state.playback_session.was_paused
                && self
                    .state
                    .playback_session
                    .track_path
                    .as_ref()
                    .map(|saved_path| same_path(saved_path, &path))
                    .unwrap_or(false))
            .then_some(self.state.playback_session.position_seconds.max(0.0));
            session.source = "track".to_owned();
            session.was_active = is_active || preserved_paused_session_position.is_some();
            session.was_paused = player_is_paused || preserved_paused_session_position.is_some();
            session.playlist_index = Some(playlist_index);
            session.track_path = Some(path.clone());
            session.position_seconds = if is_active {
                self.displayed_playback_position_seconds().max(0.0)
            } else {
                preserved_paused_session_position.unwrap_or(0.0)
            };
            self.state.last_played_track = Some(LastPlayedTrack {
                playlist_index,
                track_path: path,
            });
        }

        self.state.playback_session = session;
    }
    pub(crate) fn active_track_title(&self) -> String {
        if let Some(radio_index) = self.active_radio_index {
            if let Some(station) = self.state.radio_stations.get(radio_index) {
                let stream_title = self
                    .active_radio_title
                    .clone()
                    .or_else(|| station.last_stream_title.clone())
                    .filter(|title| {
                        !title.trim().is_empty() && !title.eq_ignore_ascii_case(&station.name)
                    });
                let station_name = self
                    .active_radio_station_name
                    .clone()
                    .or_else(|| station.last_station_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| station.name.clone());

                return stream_title.unwrap_or(station_name);
            }
        }

        self.active_track_path
            .as_ref()
            .map(|path| display_file_name(path))
            .unwrap_or_else(|| "No track playing".to_owned())
    }
    pub(crate) fn active_track_detail(&self) -> Option<String> {
        if let Some(radio_index) = self.active_radio_index {
            return self.state.radio_stations.get(radio_index).map(|station| {
                let station_name = self
                    .active_radio_station_name
                    .clone()
                    .or_else(|| station.last_station_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| station.name.clone());
                let elapsed = self
                    .radio_elapsed_seconds()
                    .map(format_duration)
                    .unwrap_or_else(|| "0:00".to_owned());
                format!("{station_name} · live for {elapsed} · {}", station.url)
            });
        }

        self.last_playback.as_ref().map(|playback| {
            if self.player_only_mode {
                format!(
                    "{}k · {} ch · {}",
                    playback.sample_rate / 1000,
                    playback.input_channels,
                    playback
                        .size_bytes
                        .map(format_file_size)
                        .unwrap_or_else(|| "unknown size".to_owned())
                )
            } else {
                format!(
                    "{} · {} Hz · {} ch · {} · duration {}",
                    display_parent(&playback.path),
                    playback.sample_rate,
                    playback.input_channels,
                    playback
                        .size_bytes
                        .map(format_file_size)
                        .unwrap_or_else(|| "unknown size".to_owned()),
                    format_duration(playback.original_duration_seconds)
                )
            }
        })
    }
    pub(crate) fn active_track_time_label(&self) -> String {
        if self.active_radio_index.is_some() {
            return self
                .radio_elapsed_seconds()
                .map(|elapsed| format!("Live stream · {}", format_duration(elapsed)))
                .unwrap_or_else(|| "Live stream · 0:00".to_owned());
        }

        if self.active_track_path.is_some() || self.pending_track_switch.is_some() {
            let position = self
                .waveform_drag_position_seconds
                .unwrap_or_else(|| self.displayed_playback_position_seconds());
            let duration = self.displayed_playback_duration_seconds();
            return format!(
                "{} / {}",
                format_duration(position),
                format_duration(duration)
            );
        }

        String::new()
    }
    pub(crate) fn radio_elapsed_seconds(&self) -> Option<f32> {
        self.radio_started_at
            .map(|started_at| started_at.elapsed().as_secs_f32())
    }
    pub(crate) fn random_sequence_index(
        &self,
        indexes: &[usize],
        current_index: Option<usize>,
    ) -> Option<usize> {
        if indexes.is_empty() {
            return None;
        }

        if indexes.len() == 1 {
            return indexes.first().copied();
        }

        let candidates = indexes
            .iter()
            .copied()
            .filter(|index| Some(*index) != current_index)
            .collect::<Vec<_>>();
        let candidates = if candidates.is_empty() {
            indexes.to_vec()
        } else {
            candidates
        };
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as usize)
            .unwrap_or(0);
        let seed = nanos ^ self.status_updated_at.elapsed().as_nanos() as usize ^ candidates.len();
        candidates.get(seed % candidates.len()).copied()
    }
    pub(crate) fn selected_track_is_visible(&self) -> bool {
        let Some(index) = self.selected_track_index else {
            return false;
        };

        self.current_playlist()
            .and_then(|playlist| {
                playlist
                    .tracks
                    .get(index)
                    .map(|track| playlist.track_matches_selected_group(track))
            })
            .unwrap_or(false)
    }
    pub(crate) fn ensure_selected_track_visible(&mut self) {
        if !self.selected_track_is_visible() {
            self.selected_track_index = self.eligible_track_indexes().first().copied();
        }
    }
    pub(crate) fn favorites_index(&self) -> Option<usize> {
        self.state
            .playlists
            .iter()
            .position(|playlist| playlist.kind == PlaylistKind::Favorites)
    }
    pub(crate) fn is_favorite(&self, path: &Path) -> bool {
        self.favorites_index()
            .and_then(|index| self.state.playlists.get(index))
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .any(|track| same_path(&track.path, path))
            })
            .unwrap_or(false)
    }
    pub(crate) fn toggle_favorite(&mut self, path: PathBuf) {
        let Some(favorites_index) = self.favorites_index() else {
            return;
        };

        let favorites_is_selected = self.state.selected_playlist_index == favorites_index;
        let selected_path = favorites_is_selected
            .then(|| self.selected_track_path())
            .flatten();
        let selected_index = self.selected_track_index.unwrap_or(0);
        let is_favorite = self.is_favorite(&path);
        if let Some(favorites) = self.state.playlists.get_mut(favorites_index) {
            if is_favorite {
                favorites
                    .tracks
                    .retain(|track| !same_path(&track.path, &path));
                self.status_message = "Removed from Favorites.".to_owned();
            } else {
                favorites.add_track_path(path.clone(), None, 0);
                self.status_message = "Added to Favorites.".to_owned();
            }
        }

        if favorites_is_selected {
            self.clear_multi_track_selection();
            let favorites = &self.state.playlists[favorites_index];
            self.selected_track_index = selected_path
                .as_ref()
                .and_then(|selected_path| {
                    favorites
                        .tracks
                        .iter()
                        .position(|track| same_path(&track.path, selected_path))
                })
                .or_else(|| next_valid_track_index(selected_index, favorites.tracks.len()));
            self.ensure_selected_track_visible();
        }
        if self.active_playlist_index == Some(favorites_index) {
            self.active_track_index = self.active_track_path.as_ref().and_then(|active_path| {
                self.state.playlists[favorites_index]
                    .tracks
                    .iter()
                    .position(|track| same_path(&track.path, active_path))
            });
        }

        self.save_state_silently();
    }
    pub(crate) fn add_tracks_to_playlist(&mut self, paths: Vec<PathBuf>, playlist_index: usize) {
        let Some(playlist) = self.state.playlists.get_mut(playlist_index) else {
            return;
        };

        if !playlist.accepts_manual_tracks() {
            self.error_message =
                Some("This playlist is read-only and cannot receive manual tracks.".to_owned());
            return;
        }

        let playlist_name = playlist.name.clone();
        let requested = paths.len();
        let added_count = paths
            .into_iter()
            .filter(|path| playlist.add_track_path(path.clone(), None, 0))
            .count();
        self.status_message = if added_count == 0 {
            format!("Selected track(s) are already in {playlist_name}.")
        } else if requested == 1 {
            format!("Added track to {playlist_name}.")
        } else {
            format!("Added {added_count} of {requested} selected track(s) to {playlist_name}.")
        };

        if added_count > 0 && self.active_playlist_index == Some(playlist_index) {
            self.active_track_index = self.active_track_path.as_ref().and_then(|active_path| {
                self.state.playlists[playlist_index]
                    .tracks
                    .iter()
                    .position(|track| same_path(&track.path, active_path))
            });
        }
        self.save_state_silently();
    }
    pub(crate) fn remove_track_from_current_playlist(&mut self, track_index: usize) {
        let Some(remaining_len) = self.current_playlist_mut().and_then(|playlist| {
            if matches!(
                playlist.kind,
                PlaylistKind::Folder | PlaylistKind::Temporary
            ) {
                None
            } else if track_index < playlist.tracks.len() {
                playlist.tracks.remove(track_index);
                Some(playlist.tracks.len())
            } else {
                None
            }
        }) else {
            self.error_message = Some("This playlist is read-only.".to_owned());
            return;
        };

        self.selected_track_index = next_valid_track_index(track_index, remaining_len);
        self.clear_multi_track_selection();
        self.save_state_silently();
    }
    pub(crate) fn remove_missing_track_from_current_playlist(&mut self, track_index: usize) {
        let Some(path) = self
            .current_playlist()
            .and_then(|playlist| playlist.tracks.get(track_index))
            .filter(|track| track.missing)
            .map(|track| track.path.clone())
        else {
            self.error_message = Some("Track is no longer marked as missing.".to_owned());
            return;
        };

        if self
            .active_track_path
            .as_ref()
            .map(|active| same_path(active, &path))
            .unwrap_or(false)
        {
            self.stop();
        }

        let Some(remaining_len) = self.current_playlist_mut().map(|playlist| {
            playlist.tracks.remove(track_index);
            playlist
                .repeat_selection
                .retain(|selected| !same_path(selected, &path));
            playlist.set_selected_group(playlist.selected_group.clone());
            playlist.tracks.len()
        }) else {
            return;
        };

        self.selected_track_index = next_valid_track_index(track_index, remaining_len);
        self.clear_multi_track_selection();
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
        self.status_message = format!("Removed missing entry {}.", display_file_name(&path));
        self.error_message = None;
        self.save_state_silently();
    }
    pub(crate) fn delete_track_from_disk(&mut self, path: PathBuf) {
        if self
            .active_track_path
            .as_ref()
            .map(|active| same_path(active, &path))
            .unwrap_or(false)
        {
            self.stop();
        }

        match fs::remove_file(&path) {
            Ok(()) => {
                for playlist in &mut self.state.playlists {
                    playlist
                        .tracks
                        .retain(|track| !same_path(&track.path, &path));
                }
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.clear_multi_track_selection();
                self.status_message = format!("Deleted {} from disk.", display_file_name(&path));
                self.error_message = None;
                self.save_state_silently();
            }
            Err(error) => self.error_message = Some(format!("Failed to delete file: {error}")),
        }
    }
    pub(crate) fn reveal_track_in_file_manager(&mut self, path: PathBuf) {
        if let Err(error) = reveal_in_file_manager(&path) {
            self.error_message = Some(error.to_string());
        }
    }
    pub(crate) fn store_playback_metadata(&mut self, info: &PlaybackInfo) {
        for playlist in &mut self.state.playlists {
            for track in &mut playlist.tracks {
                if same_path(&track.path, &info.path) {
                    track.update_playback_metadata(
                        info.original_duration_seconds,
                        info.sample_rate,
                        info.input_channels,
                        info.waveform.clone(),
                        info.waveform_brightness.clone(),
                    );
                    if track.metadata.size_bytes.is_none() {
                        track.metadata.size_bytes = info.size_bytes;
                    }
                }
            }
        }
        self.save_state_silently();
    }
}
