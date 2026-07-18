use crate::*;
use std::sync::atomic::Ordering;

impl AudioOrbitApp {
    pub(crate) fn dj_mix_is_running(&self) -> bool {
        self.dj_mix_event_receiver.is_some()
    }

    pub(crate) fn open_dj_mix_builder_for_current_playlist(&mut self) {
        let tracks = self
            .current_playlist()
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .filter(|track| !track.missing && track.path.is_file())
                    .map(|track| DjMixTrack {
                        path: track.path.clone(),
                        title: track.title.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.open_dj_mix_builder(tracks);
    }

    pub(crate) fn open_dj_mix_builder_for_selection(&mut self, context_index: usize) {
        let selected_paths = self
            .action_track_paths_for_context(context_index)
            .into_iter()
            .map(|path| path_key(&path))
            .collect::<BTreeSet<_>>();
        let tracks = self
            .current_playlist()
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .filter(|track| {
                        selected_paths.contains(&path_key(&track.path))
                            && !track.missing
                            && track.path.is_file()
                    })
                    .map(|track| DjMixTrack {
                        path: track.path.clone(),
                        title: track.title.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.open_dj_mix_builder(tracks);
    }

    fn open_dj_mix_builder(&mut self, tracks: Vec<DjMixTrack>) {
        if self.dj_mix_is_running() {
            self.error_message = Some("DJ mix export already running.".to_owned());
            return;
        }
        if tracks.len() < 2 {
            self.error_message = Some("DJ mix requires at least two available tracks.".to_owned());
            return;
        }
        self.active_panel_modal = None;
        self.panel_modal_history.clear();
        self.dj_mix_modal = Some(DjMixModalState {
            tracks,
            options: DjMixOptions::default(),
            stage: "Ready".to_owned(),
            progress: 0.0,
            output_path: None,
            completed: false,
        });
    }

    pub(crate) fn process_dj_mix_events(&mut self) {
        let (events, disconnected) = match &self.dj_mix_event_receiver {
            Some(receiver) => {
                let mut events = Vec::new();
                let mut disconnected = false;
                loop {
                    match receiver.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                (events, disconnected)
            }
            None => return,
        };

        for event in events {
            match event {
                DjMixEvent::Progress { stage, progress } => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = stage;
                        modal.progress = progress;
                    }
                }
                DjMixEvent::Completed {
                    output_path,
                    track_count,
                } => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = format!("Complete — {track_count} tracks");
                        modal.progress = 1.0;
                        modal.output_path = Some(output_path.clone());
                        modal.completed = true;
                    }
                    self.status_message = format!("DJ mix saved: {}", output_path.display());
                    self.dj_mix_event_receiver = None;
                    self.dj_mix_cancel_flag = None;
                }
                DjMixEvent::Cancelled => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = "Cancelled".to_owned();
                        modal.progress = 0.0;
                    }
                    self.status_message = "DJ mix export cancelled.".to_owned();
                    self.dj_mix_event_receiver = None;
                    self.dj_mix_cancel_flag = None;
                }
                DjMixEvent::Failed(error) => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = "Failed".to_owned();
                        modal.progress = 0.0;
                    }
                    self.error_message = Some(error);
                    self.dj_mix_event_receiver = None;
                    self.dj_mix_cancel_flag = None;
                }
            }
        }

        if disconnected && self.dj_mix_event_receiver.is_some() {
            self.dj_mix_event_receiver = None;
            self.dj_mix_cancel_flag = None;
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.stage = "Failed".to_owned();
                modal.progress = 0.0;
            }
            self.error_message = Some("DJ mix worker stopped before completing export.".to_owned());
        }
    }

    pub(crate) fn cancel_dj_mix_export(&mut self) {
        if let Some(cancel) = &self.dj_mix_cancel_flag {
            cancel.store(true, Ordering::Relaxed);
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.stage = "Cancelling...".to_owned();
            }
        }
    }

    fn start_dj_mix_export(&mut self) {
        if self.dj_mix_is_running() {
            return;
        }
        let Some(modal) = self.dj_mix_modal.as_ref() else {
            return;
        };
        if modal.tracks.len() < 2 {
            self.error_message = Some("DJ mix requires at least two tracks.".to_owned());
            return;
        }

        let default_name = self.default_dj_mix_file_name();
        let Some(output_path) = FileDialog::new()
            .add_filter("MP3 audio", &["mp3"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        let request = dj_mix::ExportRequest {
            tracks: modal.tracks.clone(),
            options: modal.options,
            output_path,
        };
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.dj_mix_event_receiver = Some(receiver);
        self.dj_mix_cancel_flag = Some(cancel);
        if let Some(modal) = self.dj_mix_modal.as_mut() {
            modal.stage = "Starting analysis...".to_owned();
            modal.progress = 0.0;
            modal.output_path = None;
            modal.completed = false;
        }
        thread::spawn(move || dj_mix::export_mix(request, sender, worker_cancel));
    }

    fn default_dj_mix_file_name(&self) -> String {
        let playlist_name = self
            .current_playlist()
            .map(|playlist| playlist.name.as_str())
            .unwrap_or("playlist");
        let safe_name = playlist_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim()
            .to_owned();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        format!(
            "{}-dj-mix-{timestamp}.mp3",
            if safe_name.is_empty() {
                "playlist"
            } else {
                &safe_name
            }
        )
    }

    pub(crate) fn render_dj_mix_modal(&mut self, context: &egui::Context) {
        let Some(mut modal) = self.dj_mix_modal.clone() else {
            return;
        };
        let running = self.dj_mix_is_running();
        self.render_modal_backdrop(context, "dj_mix_modal_backdrop");
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let body_scroll_height = (content_size.y - 96.0).max(120.0);
        let track_scroll_height = (content_size.y * 0.38).clamp(120.0, 360.0);
        let mut close_requested = false;
        let mut start_requested = false;
        let mut cancel_requested = false;
        let mut reveal_path = None;

        egui::Area::new(egui::Id::new("dj_mix_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);
                    if Self::render_modal_header(
                        ui,
                        outer_padding.x,
                        Icon::Music,
                        "DJ Mix Builder",
                        "Build one low-memory MP3 mix from selected tracks. BPM analysis, classic pitch sync, beat-aligned equal-power transitions, bass swap, and loudness leveling run in background.",
                    ) {
                        if running {
                            cancel_requested = true;
                        } else {
                            close_requested = true;
                        }
                    }

                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(outer_padding.x as i8, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .id_salt("dj_mix_modal_body")
                                .max_height(body_scroll_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    Self::render_modal_section(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.add_enabled_ui(!running, |ui| {
                                                ui.checkbox(&mut modal.options.smart_order, "Smart BPM order");
                                                ui.checkbox(&mut modal.options.normalize_loudness, "Loudness leveling");
                                                ui.checkbox(&mut modal.options.bass_swap, "Bass swap");
                                            });
                                        });
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Transition:");
                                            for beats in [8, 16, 32] {
                                                ui.add_enabled_ui(!running, |ui| {
                                                    ui.selectable_value(
                                                        &mut modal.options.transition_beats,
                                                        beats,
                                                        format!("{beats} beats"),
                                                    );
                                                });
                                            }
                                            ui.separator();
                                            ui.label("MP3:");
                                            for bitrate in [192, 256, 320] {
                                                ui.add_enabled_ui(!running, |ui| {
                                                    ui.selectable_value(
                                                        &mut modal.options.bitrate_kbps,
                                                        bitrate,
                                                        format!("{bitrate} kbps"),
                                                    );
                                                });
                                            }
                                        });
                                        ui.small("Classic pitch sync changes speed and pitch by at most ±4%. Smart order keeps BPM gaps small. Full tracks never load into RAM.");
                                    });

                                    ui.add_space(8.0);
                                    Self::render_modal_section(ui, |ui| {
                                        ui.label(&modal.stage);
                                        ui.add(
                                            egui::ProgressBar::new(modal.progress)
                                                .animate(running)
                                                .show_percentage(),
                                        );
                                        ui.small("Choose options, then click Export DJ mix... and select the output MP3 file.");
                                        if modal.tracks.len() < 2 {
                                            ui.small("Choose at least two available tracks.");
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            if running {
                                                if ui.button(ui_icons::label(Icon::X, "Cancel export")).clicked() {
                                                    cancel_requested = true;
                                                }
                                            } else if modal.completed {
                                                if let Some(path) = modal.output_path.clone() {
                                                    if ui.button(ui_icons::label(Icon::FolderOpen, "Show MP3")).clicked() {
                                                        reveal_path = Some(path);
                                                    }
                                                }
                                                if ui
                                                    .add_enabled(
                                                        modal.tracks.len() >= 2,
                                                        egui::Button::new(ui_icons::label(
                                                            Icon::Music,
                                                            "Export another...",
                                                        )),
                                                    )
                                                    .clicked()
                                                {
                                                    start_requested = true;
                                                }
                                                if ui.button("Close").clicked() {
                                                    close_requested = true;
                                                }
                                            } else {
                                                if ui
                                                    .add_enabled(
                                                        modal.tracks.len() >= 2,
                                                        egui::Button::new(ui_icons::label(Icon::Music, "Export DJ mix...")),
                                                    )
                                                    .clicked()
                                                {
                                                    start_requested = true;
                                                }
                                                if ui.button("Close").clicked() {
                                                    close_requested = true;
                                                }
                                            }
                                        });
                                    });

                                    ui.add_space(8.0);
                                    Self::render_modal_section(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.heading(format!("Tracks ({})", modal.tracks.len()));
                                            if modal.options.smart_order {
                                                ui.small("Export order optimized after BPM analysis");
                                            } else {
                                                ui.small("Manual order");
                                            }
                                        });
                                        egui::ScrollArea::vertical()
                                            .id_salt("dj_mix_tracks")
                                            .max_height(track_scroll_height)
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                let mut move_action = None;
                                                let mut remove_index = None;
                                                for index in 0..modal.tracks.len() {
                                                    let title = modal.tracks[index].title.clone();
                                                    let path = modal.tracks[index].path.clone();
                                                    ui.horizontal(|ui| {
                                                        ui.label(format!("{}.", index + 1));
                                                        ui.vertical(|ui| {
                                                            ui.label(ellipsize_chars(&title, 72));
                                                            ui.small(ellipsize_chars(&path.display().to_string(), 96));
                                                        });
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui
                                                                .add_enabled(!running && modal.tracks.len() > 2, egui::Button::new(ui_icons::icon(Icon::X)))
                                                                .on_hover_text("Remove from mix")
                                                                .clicked()
                                                            {
                                                                remove_index = Some(index);
                                                            }
                                                            if ui
                                                                .add_enabled(!running && index + 1 < modal.tracks.len(), egui::Button::new(ui_icons::icon(Icon::ArrowDown)))
                                                                .on_hover_text("Move down")
                                                                .clicked()
                                                            {
                                                                move_action = Some((index, index + 1));
                                                            }
                                                            if ui
                                                                .add_enabled(!running && index > 0, egui::Button::new(ui_icons::icon(Icon::ArrowUp)))
                                                                .on_hover_text("Move up")
                                                                .clicked()
                                                            {
                                                                move_action = Some((index, index - 1));
                                                            }
                                                        });
                                                    });
                                                    if index + 1 < modal.tracks.len() {
                                                        ui.separator();
                                                    }
                                                }
                                                if let Some((from, to)) = move_action {
                                                    modal.tracks.swap(from, to);
                                                    modal.options.smart_order = false;
                                                }
                                                if let Some(index) = remove_index {
                                                    modal.tracks.remove(index);
                                                }
                                            });
                                    });
                                });
                        });
                });
            });
        self.render_modal_info_footer_fixed(context, "dj_mix_modal_info_footer", screen_rect);

        self.dj_mix_modal = Some(modal);
        if cancel_requested {
            self.cancel_dj_mix_export();
        }
        if let Some(path) = reveal_path {
            if let Err(error) = reveal_in_file_manager(&path) {
                self.error_message = Some(error.to_string());
            }
        }
        if start_requested {
            self.start_dj_mix_export();
        }
        if close_requested && !self.dj_mix_is_running() {
            self.dj_mix_modal = None;
        }
    }
}
