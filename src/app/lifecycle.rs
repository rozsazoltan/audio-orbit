use crate::*;

impl Drop for AudioOrbitApp {
    fn drop(&mut self) {
        if let Some(cancel) = &self.dj_mix_cancel_flag {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.persist_playback_session();
        self.persist_repeat_selection_for_current_playlist();
        if let Some(player) = &mut self.player {
            player.stop();
        }
        #[cfg(debug_assertions)]
        if let Some(handle) = &mut self.dev_metrics_window {
            handle.close();
        }
        let _ = save_state(&self.state);
    }
}


impl AudioOrbitApp {
    fn next_repaint_interval(&self, context: &egui::Context) -> Duration {
        let has_live_input = context.input(|input| {
            input.pointer.delta().length_sq() > 0.01
                || input.pointer.any_released()
                || !input.events.is_empty()
                || input.raw_scroll_delta.length_sq() > 0.0
                || input.smooth_scroll_delta.length_sq() > 0.0
        });
        if has_live_input || self.waveform_drag_position_seconds.is_some() {
            return ACTIVE_INPUT_REPAINT_INTERVAL;
        }

        #[cfg(debug_assertions)]
        if self.show_dev_metrics_window {
            return Duration::from_millis(500);
        }

        if self.waveform_loading_animation_is_active() {
            return WAVEFORM_LOADING_REPAINT_INTERVAL;
        }

        if self.active_radio_index.is_some() {
            return RADIO_REPAINT_INTERVAL;
        }

        if self.player.as_ref().map(AudioPlayer::is_playing).unwrap_or(false)
            || self.pending_track_switch.is_some()
        {
            return PLAYBACK_REPAINT_INTERVAL;
        }

        if self.has_background_ui_work() {
            return BACKGROUND_WORK_REPAINT_INTERVAL;
        }

        if self.profile_apply_status_text().is_some()
            || !self.status_message.is_empty()
            || self.error_message.is_some()
        {
            return STATUS_REPAINT_INTERVAL;
        }

        IDLE_REPAINT_INTERVAL
    }

    fn waveform_loading_animation_is_active(&self) -> bool {
        self.active_radio_index.is_none()
            && (self.active_track_path.is_some() || self.pending_track_switch.is_some())
            && self
                .last_playback
                .as_ref()
                .map(|playback| playback.waveform.is_empty())
                .unwrap_or(false)
    }

    fn has_background_ui_work(&self) -> bool {
        self.pending_prepared_track_receiver.is_some()
            || self.pending_fast_seek.is_some()
            || self.pending_seek_prepare.is_some()
            || self.pending_folder_scan_receiver.is_some()
            || self.pending_library_sync_receiver.is_some()
            || self.pending_track_file_operation_receiver.is_some()
            || self.dj_mix_event_receiver.is_some()
            || self.pending_folder_watch_sync_at.is_some()
            || self.update_check_receiver.is_some()
            || self.update_install_receiver.is_some()
            || self.radio_title_receiver.is_some()
            || self.pending_profile_apply_at.is_some()
            || self.profile_apply_applied_until.map(|until| until > Instant::now()).unwrap_or(false)
            || self.detected_output_change.is_some()
            || self.focus_track_search
            || self.focus_radio_search
            || self.scroll_to_active_track_requested
            || self.scroll_to_track_path_requested.is_some()
            || self.scroll_to_active_radio_requested
            || self.scroll_to_folder_group_requested.is_some()
    }
}

impl eframe::App for AudioOrbitApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        context.set_visuals(egui::Visuals::dark());
        let repaint_interval = self.next_repaint_interval(context);
        context.request_repaint_after(repaint_interval);
        #[cfg(debug_assertions)]
        self.update_dev_metrics(context, repaint_interval);
        self.remember_window_geometry(context);

        self.process_external_open_requests(context);
        self.process_media_key_events();
        self.process_update_events();
        self.maybe_start_auto_update_check();
        self.process_escape_navigation(context);
        self.process_keyboard_shortcuts(context);
        self.process_radio_title_events();
        self.refresh_radio_title_periodically();
        self.process_pending_profile_apply();
        self.refresh_selected_folder_watcher(context);
        self.process_folder_watch_events();
        self.process_folder_scan_events();
        self.process_library_sync_events();
        self.process_track_file_operation_events();
        self.process_dj_mix_events();
        self.maybe_start_auto_library_sync();
        self.process_pending_fast_seek();
        self.process_pending_seek_prepare();
        self.process_prepared_track_playback();
        self.process_pending_track_switch();
        self.update_playback_status();
        self.poll_output_device_change();
        self.sync_status_lifetime();
        if let Some(text) = self.pending_clipboard_text.take() {
            context.copy_text(text);
        }

        let now_playing_response = egui::TopBottomPanel::top("now_playing_panel").show(context, |ui| {
            self.render_now_playing_panel(ui);
        });
        self.handle_top_panel_volume_wheel(&now_playing_response.response, context);

        if !self.player_only_mode && self.show_library_panel {
            egui::SidePanel::left("library_panel")
                .resizable(true)
                .default_width(340.0)
                .width_range(260.0..=520.0)
                .show(context, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("library_panel_outer_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.render_library_panel(ui);
                            ui.add_space(16.0);
                        });
                });
        }

        if !self.player_only_mode && self.show_profile_panel {
            egui::SidePanel::right("profile_panel")
                .resizable(true)
                .default_width(340.0)
                .width_range(280.0..=520.0)
                .show(context, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("profile_panel_outer_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.render_profile_panel(ui);
                            ui.add_space(16.0);
                        });
                });
        }

        if self.profile_apply_status_text().is_some()
            || !self.status_message.is_empty()
            || !self.media_key_status.is_empty()
            || self.playlist_count_label().is_some()
        {
            egui::TopBottomPanel::bottom("status_panel").show(context, |ui| {
                self.render_status_panel(ui);
            });
        }

        egui::CentralPanel::default().show(context, |ui| {
            self.render_main_content_panel(ui);
        });

        if self.show_folder_import_modal {
            self.render_folder_import_window(context);
        }

        if let Some(panel) = self.active_panel_modal {
            self.render_panel_modal(context, panel);
        }

        if self.show_radio_add_modal {
            self.render_radio_add_modal(context);
        }

        if self.show_new_playlist_modal {
            self.render_new_playlist_modal(context);
        }

        if self.pending_track_delete_confirmation.is_some() {
            self.render_track_delete_confirmation_modal(context);
        }

        if self.dj_mix_modal.is_some() {
            self.render_dj_mix_modal(context);
        }

        if self.details_modal.is_some() {
            self.render_details_modal(context);
        }

        self.render_drag_feedback(context);
        self.render_error_toast(context);

        #[cfg(debug_assertions)]
        self.render_dev_metrics_window(context);
    }
}
