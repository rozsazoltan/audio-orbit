use crate::*;

impl Drop for AudioOrbitApp {
    fn drop(&mut self) {
        self.persist_playback_session();
        self.persist_repeat_selection_for_current_playlist();
        if let Some(player) = &mut self.player {
            player.stop();
        }
        let _ = save_state(&self.state);
    }
}

impl eframe::App for AudioOrbitApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        context.set_visuals(egui::Visuals::dark());
        let repaint_interval = if self.active_radio_index.is_some() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(33)
        };
        context.request_repaint_after(repaint_interval);
        self.remember_window_geometry(context);

        self.process_media_key_events();
        self.process_update_events();
        self.maybe_start_auto_update_check();
        self.process_escape_navigation(context);
        self.process_keyboard_shortcuts(context);
        self.process_radio_title_events();
        self.refresh_radio_title_periodically();
        self.process_pending_profile_apply();
        self.process_folder_scan_events();
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

        if self.details_modal.is_some() {
            self.render_details_modal(context);
        }

        self.render_drag_feedback(context);
        self.render_error_toast(context);
    }
}
