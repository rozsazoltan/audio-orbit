use crate::*;

impl AudioOrbitApp {
    pub(crate) fn process_keyboard_shortcuts(&mut self, context: &egui::Context) {
        let has_blocking_modal = self.show_folder_import_modal
            || self.active_panel_modal.is_some()
            || self.show_radio_add_modal
            || self.show_new_playlist_modal
            || self.pending_track_delete_confirmation.is_some()
            || self.details_modal.is_some();
        let undo_order = context.input(|input| {
            input.key_pressed(egui::Key::Z) && (input.modifiers.ctrl || input.modifiers.command)
        });
        if !has_blocking_modal && undo_order {
            self.undo_last_order_change();
            return;
        }
        if has_blocking_modal || context.wants_keyboard_input() {
            return;
        }

        let (space, enter, stop, next, previous, seek_forward, seek_backward, search, player_only, library, profiles) = context.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::S),
                input.key_pressed(egui::Key::ArrowRight) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::ArrowLeft) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::ArrowRight) && !input.modifiers.ctrl,
                input.key_pressed(egui::Key::ArrowLeft) && !input.modifiers.ctrl,
                input.key_pressed(egui::Key::F) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::M),
                input.key_pressed(egui::Key::L) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::P) && input.modifiers.ctrl,
            )
        });

        if space {
            let is_active = self
                .player
                .as_ref()
                .map(|player| player.is_playing() || player.is_paused())
                .unwrap_or(false);

            if is_active {
                self.pause_or_resume();
            } else {
                self.play_selected_or_first_track();
            }
        }

        if enter {
            self.play_selected_or_first_track();
        }

        if stop {
            self.stop();
        }

        if next {
            self.play_next_track();
        }

        if previous {
            self.play_previous_track();
        }

        if seek_forward {
            self.seek_relative(10.0);
        }

        if seek_backward {
            self.seek_relative(-10.0);
        }

        if search {
            if self.active_tab == MainContentTab::Radio {
                self.show_radio_search = !self.show_radio_search;
                self.focus_radio_search = self.show_radio_search;
                if !self.show_radio_search {
                    self.radio_search_query.clear();
                }
            } else {
                self.show_track_search = !self.show_track_search;
                self.focus_track_search = self.show_track_search;
                self.state.ui.show_track_search = self.show_track_search;
                if !self.show_track_search {
                    self.track_search_query.clear();
                    self.search_cursor = 0;
                }
                self.save_state_silently();
            }
        }

        if player_only {
            self.toggle_player_only_mode(context);
        }

        if library && !self.player_only_mode {
            self.show_library_panel = !self.show_library_panel;
            self.state.ui.show_library_panel = self.show_library_panel;
            self.save_state_silently();
        }

        if profiles && !self.player_only_mode {
            self.show_profile_panel = !self.show_profile_panel;
            self.state.ui.show_profile_panel = self.show_profile_panel;
            self.save_state_silently();
        }
    }
    pub(crate) fn process_media_key_events(&mut self) {
        let events = match &self.media_key_receiver {
            Some(receiver) => receiver.try_iter().collect::<Vec<_>>(),
            None => return,
        };

        for event in events {
            match event {
                media_keys::MediaKeyEvent::Ready { registered, failed } => {
                    self.media_key_status = media_key_status_message(&registered, &failed);
                }
                media_keys::MediaKeyEvent::Command(command) => {
                    self.handle_media_key_command(command);
                }
            }
        }
    }
    pub(crate) fn handle_media_key_command(&mut self, command: media_keys::MediaKeyCommand) {
        match command {
            media_keys::MediaKeyCommand::Previous => self.play_previous_track(),
            media_keys::MediaKeyCommand::PlayPause => {
                let is_active = self
                    .player
                    .as_ref()
                    .map(|player| player.is_playing() || player.is_paused())
                    .unwrap_or(false);

                if is_active {
                    self.pause_or_resume();
                } else if self.active_tab == MainContentTab::Radio {
                    if let Some(index) = self.state.selected_radio_index {
                        self.play_radio_station(index);
                    }
                } else {
                    self.play_selected_or_first_track();
                }
            }
            media_keys::MediaKeyCommand::Stop => self.stop(),
            media_keys::MediaKeyCommand::Next => self.play_next_track(),
        }
    }
}
