use crate::*;

impl AudioOrbitApp {
    pub(crate) fn new(state: SavedState) -> Self {
        let pending_playlist_name = "Local music".to_owned();
        let current_output_name = current_default_output_device_name();
        let show_library_panel = state.ui.show_library_panel;
        let show_profile_panel = state.ui.show_profile_panel;
        let player_only_mode = state.ui.player_only_mode;
        let show_track_search = state.ui.show_track_search;
        let search_playback_filtered_only = state.ui.search_playback_filtered_only;

        let mut app = match AudioPlayer::new() {
            Ok(player) => {
                let output_name = player.output_device_name().to_owned();
                Self {
                    player: Some(player),
                    state,
                    selected_track_index: None,
                    selected_track_indexes: BTreeSet::new(),
                    multi_selected_track_indexes: BTreeSet::new(),
                    track_selection_anchor_index: None,
                    active_track_index: None,
                    active_playlist_index: None,
                    active_track_path: None,
                    last_playback: None,
                    status_message: format!("Ready. Output device: {output_name}"),
                    status_last_seen: String::new(),
                    status_updated_at: Instant::now(),
                    error_message: None,
                    error_last_seen: None,
                    error_updated_at: Instant::now(),
                    crossfade_started_for_path: None,
                    pending_track_switch: None,
                    pending_prepared_track_receiver: None,
                    pending_seek_prepare: None,
                    pending_fast_seek: None,
                    last_fast_seek_started_at: None,
                    silence_analysis_cache: BTreeMap::new(),
                    pending_folder_scan_receiver: None,
                    pending_library_sync_receiver: None,
                    pending_track_file_operation_receiver: None,
                    folder_watcher: None,
                    folder_watcher_target_key: None,
                    pending_folder_watch_sync_at: None,
                    pending_folder_watch_paths: BTreeMap::new(),
                    pending_folder_watch_full_rescan: false,
                    pending_profile_apply_at: None,
                    profile_apply_applied_until: None,
                    waveform_drag_position_seconds: None,
                    suppress_window_geometry_save_until: None,
                    show_folder_import_modal: false,
                    show_radio_add_modal: false,
                    show_new_playlist_modal: false,
                    pending_new_playlist_name: String::new(),
                    pending_new_playlist_tracks: Vec::new(),
                    pending_track_delete_confirmation: None,
                    pending_track_delete_confirmation_text: String::new(),
                    active_panel_modal: None,
                    panel_modal_history: Vec::new(),
                    details_modal: None,
                    show_library_panel,
                    show_profile_panel,
                    player_only_mode,
                    show_track_search,
                    show_radio_search: false,
                    search_playback_filtered_only,
                    focus_track_search: false,
                    focus_radio_search: false,
                    scroll_to_active_track_requested: false,
                    scroll_to_track_path_requested: None,
                    scroll_to_active_radio_requested: false,
                    scroll_to_folder_group_requested: None,
                    active_tab: MainContentTab::Music,
                    track_search_query: String::new(),
                    search_cursor: 0,
                    pending_radio_name: String::new(),
                    pending_radio_url: String::new(),
                    radio_search_query: String::new(),
                    radio_show_favorites_only: false,
                    active_radio_index: None,
                    radio_selection_was_user_set: false,
                    active_radio_station_name: None,
                    active_radio_title: None,
                    radio_started_at: None,
                    last_radio_title_lookup_at: None,
                    radio_title_receiver: None,
                    dragging_track_index: None,
                    dragging_radio_index: None,
                    track_drop_target_index: None,
                    radio_drop_target_index: None,
                    order_undo_stack: Vec::new(),
                    collapsed_groups: BTreeSet::new(),
                    pending_folder_path: None,
                    pending_playlist_name,
                    pending_folder_depth: 2,
                    last_known_output_name: output_name,
                    detected_output_change: None,
                    last_output_check: Instant::now(),
                    editing_playlist_index: None,
                    editing_profile_index: None,
                    pending_clipboard_text: None,
                    media_key_receiver: None,
                    media_key_status: "Media keys: unavailable".to_owned(),
                    update_check_receiver: None,
                    update_install_receiver: None,
                    last_update_check: None,
                    update_check_started_at: None,
                    update_install_started_at: None,
                    #[cfg(debug_assertions)]
                    dev_metrics: DevMetricsPanelState::default(),
                    #[cfg(debug_assertions)]
                    show_dev_metrics_window: false,
                    #[cfg(debug_assertions)]
                    dev_metrics_window: None,
                }
            }
            Err(error) => Self {
                player: None,
                state,
                selected_track_index: None,
                selected_track_indexes: BTreeSet::new(),
                multi_selected_track_indexes: BTreeSet::new(),
                track_selection_anchor_index: None,
                active_track_index: None,
                active_playlist_index: None,
                active_track_path: None,
                last_playback: None,
                status_message: "No audio output device is available.".to_owned(),
                status_last_seen: String::new(),
                status_updated_at: Instant::now(),
                error_message: Some(error.to_string()),
                error_last_seen: Some(error.to_string()),
                error_updated_at: Instant::now(),
                crossfade_started_for_path: None,
                pending_track_switch: None,
                pending_prepared_track_receiver: None,
                pending_seek_prepare: None,
                pending_fast_seek: None,
                last_fast_seek_started_at: None,
                silence_analysis_cache: BTreeMap::new(),
                pending_folder_scan_receiver: None,
                pending_library_sync_receiver: None,
                pending_track_file_operation_receiver: None,
                folder_watcher: None,
                folder_watcher_target_key: None,
                pending_folder_watch_sync_at: None,
                pending_folder_watch_paths: BTreeMap::new(),
                pending_folder_watch_full_rescan: false,
                pending_profile_apply_at: None,
                profile_apply_applied_until: None,
                waveform_drag_position_seconds: None,
                suppress_window_geometry_save_until: None,
                show_folder_import_modal: false,
                show_radio_add_modal: false,
                show_new_playlist_modal: false,
                pending_new_playlist_name: String::new(),
                pending_new_playlist_tracks: Vec::new(),
                pending_track_delete_confirmation: None,
                pending_track_delete_confirmation_text: String::new(),
                active_panel_modal: None,
                panel_modal_history: Vec::new(),
                details_modal: None,
                show_library_panel,
                show_profile_panel,
                player_only_mode,
                show_track_search,
                show_radio_search: false,
                search_playback_filtered_only,
                focus_track_search: false,
                focus_radio_search: false,
                scroll_to_active_track_requested: false,
                scroll_to_track_path_requested: None,
                scroll_to_active_radio_requested: false,
                scroll_to_folder_group_requested: None,
                active_tab: MainContentTab::Music,
                track_search_query: String::new(),
                search_cursor: 0,
                pending_radio_name: String::new(),
                pending_radio_url: String::new(),
                radio_search_query: String::new(),
                radio_show_favorites_only: false,
                active_radio_index: None,
                radio_selection_was_user_set: false,
                active_radio_station_name: None,
                active_radio_title: None,
                radio_started_at: None,
                last_radio_title_lookup_at: None,
                radio_title_receiver: None,
                dragging_track_index: None,
                dragging_radio_index: None,
                track_drop_target_index: None,
                radio_drop_target_index: None,
                order_undo_stack: Vec::new(),
                collapsed_groups: BTreeSet::new(),
                pending_folder_path: None,
                pending_playlist_name,
                pending_folder_depth: 2,
                last_known_output_name: current_output_name,
                detected_output_change: None,
                last_output_check: Instant::now(),
                editing_playlist_index: None,
                editing_profile_index: None,
                pending_clipboard_text: None,
                media_key_receiver: None,
                media_key_status: "Media keys: unavailable".to_owned(),
                update_check_receiver: None,
                update_install_receiver: None,
                last_update_check: None,
                update_check_started_at: None,
                update_install_started_at: None,
                #[cfg(debug_assertions)]
                dev_metrics: DevMetricsPanelState::default(),
                #[cfg(debug_assertions)]
                show_dev_metrics_window: false,
                #[cfg(debug_assertions)]
                dev_metrics_window: None,
            },
        };

        let initial_volume_percent = app.effective_volume_percent();
        if let Some(player) = &mut app.player {
            player.set_volume_percent(initial_volume_percent);
        }

        let media_keys = media_keys::start_listener();
        app.media_key_receiver = media_keys.receiver;
        app.media_key_status = media_keys.status_message;
        app.restore_last_played_track_selection();
        app.restore_saved_playback_session();
        app.restore_repeat_selection_for_current_playlist();
        app.start_library_sync(LibrarySyncTrigger::Startup, false);
        app
    }
    pub(crate) fn remember_window_geometry(&mut self, context: &egui::Context) {
        if self
            .suppress_window_geometry_save_until
            .map(|blocked_until| blocked_until > Instant::now())
            .unwrap_or(false)
        {
            return;
        }

        let (inner_rect, outer_rect) = context.input(|input| {
            let viewport = input.viewport();
            (viewport.inner_rect, viewport.outer_rect)
        });

        let Some(inner_rect) = inner_rect else {
            return;
        };

        if inner_rect.width() < 320.0 || inner_rect.height() < 180.0 {
            return;
        }

        let position = outer_rect.map(|rect| rect.min).unwrap_or(inner_rect.min);
        let geometry = WindowGeometry {
            x: position.x,
            y: position.y,
            width: inner_rect.width(),
            height: inner_rect.height(),
        };

        if self.player_only_mode {
            self.state.ui.player_only_window_geometry = Some(geometry);
        } else {
            self.state.ui.full_layout_window_geometry = Some(geometry);
        }
        self.state.ui.window_geometry = Some(geometry);
    }
    pub(crate) fn saved_window_size_for_mode(&self, player_only_mode: bool) -> egui::Vec2 {
        let min_size = min_window_size_for_mode(player_only_mode);
        let geometry = if player_only_mode {
            self.state.ui.player_only_window_geometry
        } else {
            self.state.ui.full_layout_window_geometry
        };

        geometry
            .filter(WindowGeometry::is_valid)
            .map(|geometry| egui::vec2(geometry.width.max(min_size.x), geometry.height.max(min_size.y)))
            .unwrap_or_else(|| default_window_size_for_mode(player_only_mode))
    }
    pub(crate) fn apply_window_mode_size(&self, context: &egui::Context, player_only_mode: bool) {
        context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min_window_size_for_mode(player_only_mode)));
        context.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.saved_window_size_for_mode(player_only_mode)));
    }
    pub(crate) fn toggle_player_only_mode(&mut self, context: &egui::Context) {
        self.remember_window_geometry(context);
        self.player_only_mode = !self.player_only_mode;
        self.state.ui.player_only_mode = self.player_only_mode;
        self.apply_window_mode_size(context, self.player_only_mode);
        self.suppress_window_geometry_save_until = Some(Instant::now() + Duration::from_millis(450));
        self.save_state_silently();
    }
    pub(crate) fn open_panel_modal(&mut self, panel: AppPanelModal) {
        if self.active_panel_modal == Some(panel) {
            return;
        }

        if let Some(current) = self.active_panel_modal {
            self.panel_modal_history.push(current);
        }
        self.active_panel_modal = Some(panel);
    }
    pub(crate) fn close_panel_modal(&mut self) {
        self.active_panel_modal = self.panel_modal_history.pop();
    }
    pub(crate) fn close_track_search(&mut self) {
        if self.show_track_search {
            self.show_track_search = false;
            self.state.ui.show_track_search = false;
            self.track_search_query.clear();
            self.search_cursor = 0;
            self.save_state_silently();
        }
    }
    pub(crate) fn close_radio_search(&mut self) {
        if self.show_radio_search {
            self.show_radio_search = false;
            self.radio_search_query.clear();
        }
    }
    pub(crate) fn process_escape_navigation(&mut self, context: &egui::Context) {
        if !context.input(|input| input.key_pressed(egui::Key::Escape)) {
            return;
        }

        if self.active_panel_modal.is_some() {
            self.close_panel_modal();
        } else if self.show_folder_import_modal {
            self.show_folder_import_modal = false;
        } else if self.show_radio_add_modal {
            self.show_radio_add_modal = false;
        } else if self.show_new_playlist_modal {
            self.show_new_playlist_modal = false;
            self.pending_new_playlist_name.clear();
            self.pending_new_playlist_tracks.clear();
        } else if self.pending_track_delete_confirmation.is_some() {
            self.pending_track_delete_confirmation = None;
            self.pending_track_delete_confirmation_text.clear();
        } else if self.details_modal.is_some() {
            self.details_modal = None;
        } else if self.show_track_search {
            self.close_track_search();
        } else if self.show_radio_search {
            self.close_radio_search();
        }
    }
    pub(crate) fn save_state_silently(&mut self) {
        self.persist_repeat_selection_for_current_playlist();
        self.remember_current_playlist_scroll_offset(self.state.ui.playlist_scroll_offset_y);
        if let Err(error) = save_state(&self.state) {
            self.error_message = Some(error.to_string());
        }
    }
    pub(crate) fn sync_status_lifetime(&mut self) {
        if self.status_message != self.status_last_seen {
            self.status_last_seen = self.status_message.clone();
            self.status_updated_at = Instant::now();
        } else if !self.status_message.is_empty() && self.status_updated_at.elapsed() >= Duration::from_secs(10) {
            self.status_message.clear();
            self.status_last_seen.clear();
            self.status_updated_at = Instant::now();
        }

        if self.error_message != self.error_last_seen {
            self.error_last_seen = self.error_message.clone();
            self.error_updated_at = Instant::now();
        } else if self.error_message.is_some() && self.error_updated_at.elapsed() >= Duration::from_secs(10) {
            self.error_message = None;
            self.error_last_seen = None;
            self.error_updated_at = Instant::now();
        }
    }
}
