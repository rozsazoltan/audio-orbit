use crate::*;

impl AudioOrbitApp {
    pub(crate) fn add_playlist(&mut self) {
        let number = self.state.playlists.len() + 1;
        self.persist_repeat_selection_for_current_playlist();
        self.state.playlists.push(Playlist::new(format!("Playlist {number}")));
        self.state.selected_playlist_index = self.state.playlists.len() - 1;
        self.restore_repeat_selection_for_current_playlist();
        self.selected_track_index = None;
        self.status_message = "Created a new playlist.".to_owned();
        self.save_state_silently();
    }
    pub(crate) fn remove_current_playlist(&mut self) {
        let removed_index = self.state.selected_playlist_index;
        let Some(playlist) = self.state.playlists.get(removed_index) else {
            return;
        };

        if !playlist.kind.can_delete() {
            self.error_message = Some("Favorites is built-in and cannot be deleted.".to_owned());
            return;
        }

        if self.state.playlists.len() <= 1 {
            self.error_message = Some("At least one playlist is required.".to_owned());
            return;
        }

        let removed_playing_playlist = self.active_playlist_index == Some(removed_index);
        if removed_playing_playlist {
            self.stop();
        }

        self.state.playlists.remove(removed_index);
        if let Some(active_playlist_index) = self.active_playlist_index {
            if active_playlist_index > removed_index {
                self.active_playlist_index = Some(active_playlist_index - 1);
            }
        }
        self.state.selected_playlist_index = removed_index.saturating_sub(1).min(self.state.playlists.len() - 1);
        self.restore_repeat_selection_for_current_playlist();
        self.selected_track_index = self.eligible_track_indexes().first().copied();
        self.status_message = if removed_playing_playlist {
            "Removed playlist and stopped its active playback.".to_owned()
        } else {
            "Removed playlist. Current playback was left running.".to_owned()
        };
        self.save_state_silently();
    }
    pub(crate) fn move_playlist(&mut self, from: usize, delta: isize) {
        if from >= self.state.playlists.len() {
            return;
        }
        let to = if delta < 0 {
            from.saturating_sub(1)
        } else {
            (from + 1).min(self.state.playlists.len() - 1)
        };
        if from == to {
            return;
        }
        self.state.playlists.swap(from, to);
        if self.state.selected_playlist_index == from {
            self.state.selected_playlist_index = to;
        } else if self.state.selected_playlist_index == to {
            self.state.selected_playlist_index = from;
        }
        self.save_state_silently();
    }
    pub(crate) fn push_playlist_order_undo(&mut self) {
        let playlist_index = self.state.selected_playlist_index;
        let Some(playlist) = self.state.playlists.get(playlist_index).cloned() else {
            return;
        };

        self.order_undo_stack.push(OrderUndoSnapshot::Playlist {
            playlist_index,
            playlist,
            selected_track_index: self.selected_track_index,
            selected_track_indexes: self.selected_track_indexes.clone(),
            active_playlist_index: self.active_playlist_index,
            active_track_index: self.active_track_index,
            active_track_path: self.active_track_path.clone(),
        });
        self.trim_order_undo_stack();
    }
    pub(crate) fn push_radio_order_undo(&mut self) {
        self.order_undo_stack.push(OrderUndoSnapshot::Radio {
            stations: self.state.radio_stations.clone(),
            selected_radio_index: self.state.selected_radio_index,
            active_radio_index: self.active_radio_index,
            radio_selection_was_user_set: self.radio_selection_was_user_set,
        });
        self.trim_order_undo_stack();
    }
    pub(crate) fn trim_order_undo_stack(&mut self) {
        const MAX_ORDER_UNDO_STEPS: usize = 50;
        if self.order_undo_stack.len() > MAX_ORDER_UNDO_STEPS {
            let excess = self.order_undo_stack.len() - MAX_ORDER_UNDO_STEPS;
            self.order_undo_stack.drain(0..excess);
        }
    }
    pub(crate) fn undo_last_order_change(&mut self) {
        let Some(snapshot) = self.order_undo_stack.pop() else {
            self.status_message = "Nothing to undo.".to_owned();
            return;
        };

        match snapshot {
            OrderUndoSnapshot::Playlist {
                playlist_index,
                playlist,
                selected_track_index,
                selected_track_indexes,
                active_playlist_index,
                active_track_index,
                active_track_path,
            } => {
                if playlist_index < self.state.playlists.len() {
                    self.state.playlists[playlist_index] = playlist;
                    self.state.selected_playlist_index = playlist_index;
                    self.selected_track_index = selected_track_index
                        .filter(|index| self.current_playlist().map(|playlist| *index < playlist.tracks.len()).unwrap_or(false));
                    self.selected_track_indexes = selected_track_indexes
                        .into_iter()
                        .filter(|index| self.current_playlist().map(|playlist| *index < playlist.tracks.len()).unwrap_or(false))
                        .collect();
                    self.active_playlist_index = active_playlist_index;
                    self.active_track_index = active_track_index;
                    self.active_track_path = active_track_path;
                    self.restore_repeat_selection_for_current_playlist();
                    self.status_message = "Undid playlist order change.".to_owned();
                }
            }
            OrderUndoSnapshot::Radio {
                stations,
                selected_radio_index,
                active_radio_index,
                radio_selection_was_user_set,
            } => {
                self.state.radio_stations = stations;
                self.state.selected_radio_index = selected_radio_index
                    .filter(|index| *index < self.state.radio_stations.len());
                self.active_radio_index = active_radio_index
                    .filter(|index| *index < self.state.radio_stations.len());
                self.radio_selection_was_user_set = radio_selection_was_user_set;
                self.status_message = "Undid radio order change.".to_owned();
            }
        }

        self.dragging_track_index = None;
        self.dragging_radio_index = None;
        self.track_drop_target_index = None;
        self.radio_drop_target_index = None;
        self.save_state_silently();
    }
    pub(crate) fn valid_drop_target(from: usize, to: usize) -> bool {
        from != to && from + 1 != to
    }
    pub(crate) fn restore_track_selection_after_reorder(&mut self, selected_path: Option<PathBuf>) {
        if let Some(selected_path) = selected_path {
            if let Some(index) = self
                .current_playlist()
                .and_then(|playlist| playlist.tracks.iter().position(|track| same_path(&track.path, &selected_path)))
            {
                self.selected_track_index = Some(index);
            }
        }

        if let (Some(active_playlist_index), Some(active_path)) = (self.active_playlist_index, self.active_track_path.clone()) {
            if active_playlist_index == self.state.selected_playlist_index {
                self.active_track_index = self
                    .current_playlist()
                    .and_then(|playlist| playlist.tracks.iter().position(|track| same_path(&track.path, &active_path)));
            }
        }

        self.restore_repeat_selection_for_current_playlist();
    }
    pub(crate) fn sort_current_playlist_by_name(&mut self, ascending: bool) {
        self.persist_repeat_selection_for_current_playlist();
        let selected_path = self.selected_track_path();
        let should_sort = self.current_playlist().map(|playlist| playlist.tracks.len() > 1).unwrap_or(false);
        if should_sort {
            self.push_playlist_order_undo();
        }
        if let Some(playlist) = self.current_playlist_mut() {
            playlist.tracks.sort_by(|left, right| {
                let ordering = naturalish_key(&left.group)
                    .cmp(&naturalish_key(&right.group))
                    .then_with(|| naturalish_key(&left.title).cmp(&naturalish_key(&right.title)))
                    .then_with(|| left.path.cmp(&right.path));
                if ascending { ordering } else { ordering.reverse() }
            });
        }
        self.restore_track_selection_after_reorder(selected_path);
        self.status_message = if ascending {
            "Sorted current playlist A to Z.".to_owned()
        } else {
            "Sorted current playlist Z to A.".to_owned()
        };
        self.save_state_silently();
    }
    pub(crate) fn move_track_in_current_playlist(&mut self, index: usize, delta: isize) {
        self.persist_repeat_selection_for_current_playlist();
        let selected_path = self.selected_track_path();
        let Some(track_count) = self.current_playlist().map(|playlist| playlist.tracks.len()) else {
            return;
        };
        if index >= track_count {
            return;
        }
        let to = if delta < 0 {
            index.saturating_sub(1)
        } else {
            (index + 1).min(track_count - 1)
        };
        if index == to {
            return;
        }
        self.push_playlist_order_undo();
        let Some(playlist) = self.current_playlist_mut() else {
            return;
        };
        playlist.tracks.swap(index, to);
        self.restore_track_selection_after_reorder(selected_path);
        self.status_message = "Moved track in playlist order.".to_owned();
        self.save_state_silently();
    }
    pub(crate) fn move_track_to_index_in_current_playlist(&mut self, from: usize, to: usize) {
        self.persist_repeat_selection_for_current_playlist();
        let selected_path = self.selected_track_path();
        let Some(track_count) = self.current_playlist().map(|playlist| playlist.tracks.len()) else {
            return;
        };
        if from >= track_count || to > track_count {
            return;
        }
        self.push_playlist_order_undo();
        let Some(playlist) = self.current_playlist_mut() else {
            return;
        };
        let track = playlist.tracks.remove(from);
        let insert_at = if from < to { to.saturating_sub(1) } else { to };
        playlist.tracks.insert(insert_at.min(playlist.tracks.len()), track);
        self.restore_track_selection_after_reorder(selected_path);
        self.status_message = "Moved track in playlist order.".to_owned();
        self.save_state_silently();
    }
    pub(crate) fn move_folder_group_in_current_playlist(&mut self, group: &str, delta: isize) {
        self.persist_repeat_selection_for_current_playlist();
        let selected_path = self.selected_track_path();
        let Some(playlist) = self.current_playlist() else {
            return;
        };

        let mut group_order: Vec<String> = Vec::new();
        for track in &playlist.tracks {
            if group_order.last().map(|current| current != &track.group).unwrap_or(true)
                && !group_order.iter().any(|current| current == &track.group)
            {
                group_order.push(track.group.clone());
            }
        }
        let Some(position) = group_order.iter().position(|current| current == group) else {
            return;
        };
        let to = if delta < 0 {
            position.saturating_sub(1)
        } else {
            (position + 1).min(group_order.len().saturating_sub(1))
        };
        if position == to {
            return;
        }

        self.push_playlist_order_undo();
        let Some(playlist) = self.current_playlist_mut() else {
            return;
        };
        group_order.swap(position, to);
        let old_tracks = std::mem::take(&mut playlist.tracks);
        let mut reordered = Vec::with_capacity(old_tracks.len());
        for ordered_group in &group_order {
            reordered.extend(
                old_tracks
                    .iter()
                    .filter(|track| &track.group == ordered_group)
                    .cloned(),
            );
        }
        playlist.tracks = reordered;
        self.restore_track_selection_after_reorder(selected_path);
        self.status_message = format!("Moved folder group: {group}.");
        self.save_state_silently();
    }
    pub(crate) fn sort_radio_stations_by_name(&mut self, ascending: bool) {
        if self.state.radio_stations.len() > 1 {
            self.push_radio_order_undo();
        }
        let active_url = self
            .active_radio_index
            .and_then(|index| self.state.radio_stations.get(index).map(|station| station.url.clone()));
        let selected_url = self
            .state
            .selected_radio_index
            .and_then(|index| self.state.radio_stations.get(index).map(|station| station.url.clone()));

        self.state.radio_stations.sort_by(|left, right| {
            let ordering = naturalish_key(&left.name)
                .cmp(&naturalish_key(&right.name))
                .then_with(|| left.url.cmp(&right.url));
            if ascending { ordering } else { ordering.reverse() }
        });

        self.active_radio_index = active_url.as_ref().and_then(|url| {
            self.state.radio_stations.iter().position(|station| same_text(&station.url, url))
        });
        self.state.selected_radio_index = selected_url.as_ref().and_then(|url| {
            self.state.radio_stations.iter().position(|station| same_text(&station.url, url))
        });
        self.radio_selection_was_user_set = self.state.selected_radio_index.is_some();
        self.status_message = if ascending {
            "Sorted radio stations A to Z.".to_owned()
        } else {
            "Sorted radio stations Z to A.".to_owned()
        };
        self.save_state_silently();
    }
    pub(crate) fn move_radio_station(&mut self, index: usize, delta: isize) {
        if index >= self.state.radio_stations.len() {
            return;
        }
        let active_url = self
            .active_radio_index
            .and_then(|active_index| self.state.radio_stations.get(active_index).map(|station| station.url.clone()));
        let selected_url = self
            .state
            .selected_radio_index
            .and_then(|selected_index| self.state.radio_stations.get(selected_index).map(|station| station.url.clone()));
        let to = if delta < 0 {
            index.saturating_sub(1)
        } else {
            (index + 1).min(self.state.radio_stations.len() - 1)
        };
        if index == to {
            return;
        }
        self.push_radio_order_undo();
        self.state.radio_stations.swap(index, to);
        self.active_radio_index = active_url.as_ref().and_then(|url| {
            self.state.radio_stations.iter().position(|station| same_text(&station.url, url))
        });
        self.state.selected_radio_index = selected_url.as_ref().and_then(|url| {
            self.state.radio_stations.iter().position(|station| same_text(&station.url, url))
        });
        self.radio_selection_was_user_set = self.state.selected_radio_index.is_some();
        self.status_message = "Moved radio station.".to_owned();
        self.save_state_silently();
    }
    pub(crate) fn move_radio_station_to_index(&mut self, from: usize, to: usize) {
        if from >= self.state.radio_stations.len() || to > self.state.radio_stations.len() {
            return;
        }
        self.push_radio_order_undo();
        let active_url = self
            .active_radio_index
            .and_then(|active_index| self.state.radio_stations.get(active_index).map(|station| station.url.clone()));
        let selected_url = self
            .state
            .selected_radio_index
            .and_then(|selected_index| self.state.radio_stations.get(selected_index).map(|station| station.url.clone()));
        let station = self.state.radio_stations.remove(from);
        let insert_at = if from < to { to.saturating_sub(1) } else { to };
        self.state.radio_stations.insert(insert_at.min(self.state.radio_stations.len()), station);
        self.active_radio_index = active_url.as_ref().and_then(|url| {
            self.state.radio_stations.iter().position(|station| same_text(&station.url, url))
        });
        self.state.selected_radio_index = selected_url.as_ref().and_then(|url| {
            self.state.radio_stations.iter().position(|station| same_text(&station.url, url))
        });
        self.radio_selection_was_user_set = self.state.selected_radio_index.is_some();
        self.status_message = "Moved radio station.".to_owned();
        self.save_state_silently();
    }
}
