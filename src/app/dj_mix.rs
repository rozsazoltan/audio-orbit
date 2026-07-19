use crate::*;
use std::sync::atomic::Ordering;

impl AudioOrbitApp {
    pub(crate) fn dj_mix_is_running(&self) -> bool {
        self.dj_mix_event_receiver.is_some()
    }

    pub(crate) fn open_dj_mix_builder_for_current_playlist(&mut self) {
        if self
            .current_playlist()
            .map(|playlist| playlist.kind == PlaylistKind::Temporary)
            .unwrap_or(false)
        {
            self.error_message = Some("Temporary playback is read-only.".to_owned());
            return;
        }

        let tracks = self
            .current_playlist()
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .filter(|track| !track.missing && track.path.is_file())
                    .map(|track| DjMixTrack::new(track.path.clone(), track.title.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.open_dj_mix_builder(tracks);
    }

    pub(crate) fn open_dj_mix_builder_for_selection(&mut self, context_index: usize) {
        if self
            .current_playlist()
            .map(|playlist| playlist.kind == PlaylistKind::Temporary)
            .unwrap_or(false)
        {
            self.error_message = Some("Temporary playback is read-only.".to_owned());
            return;
        }

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
                    .map(|track| DjMixTrack::new(track.path.clone(), track.title.clone()))
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
            report_path: None,
            diagnostics_summary: None,
            completed: false,
            started_at: None,
            last_progress_at: None,
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
                        modal.last_progress_at = Some(Instant::now());
                    }
                }
                DjMixEvent::Completed {
                    output_path,
                    report_path,
                    track_count,
                    duration_seconds,
                    integrated_lufs,
                    true_peak_dbfs,
                    diagnostics_summary,
                } => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = format!(
                            "Complete — {track_count} tracks, {:.1} min",
                            duration_seconds / 60.0
                        );
                        modal.progress = 1.0;
                        modal.output_path = Some(output_path.clone());
                        modal.report_path = Some(report_path);
                        modal.diagnostics_summary = Some(diagnostics_summary);
                        modal.completed = true;
                        modal.last_progress_at = Some(Instant::now());
                    }
                    self.status_message = format!(
                        "DJ mix saved: {}{}{}",
                        output_path.display(),
                        integrated_lufs
                            .map(|value| format!(" — {value:.1} LUFS"))
                            .unwrap_or_default(),
                        true_peak_dbfs
                            .map(|value| format!(", {value:.1} dBTP"))
                            .unwrap_or_default(),
                    );
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
            modal.report_path = None;
            modal.diagnostics_summary = None;
            modal.completed = false;
            modal.started_at = Some(Instant::now());
            modal.last_progress_at = Some(Instant::now());
        }
        let spawn_result = thread::Builder::new()
            .name("audio-orbit-dj-export".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dj_mix::export_mix(request, sender.clone(), worker_cancel)
                }));
                if result.is_err() {
                    let _ = sender.send(DjMixEvent::Failed(
                        "DJ mix worker crashed unexpectedly.".to_owned(),
                    ));
                }
            });
        if let Err(error) = spawn_result {
            self.dj_mix_event_receiver = None;
            self.dj_mix_cancel_flag = None;
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.stage = "Failed".to_owned();
                modal.started_at = None;
            }
            self.error_message = Some(format!("Failed to start DJ mix worker: {error}"));
        }
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
        let mut play_path = None;

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
                        "Build one deterministic MP3 mix from selected tracks. Keep simple crossfades or use Smart DJ for phrase-aligned section selection, pitch-preserving tempo sync, beat repeats, loop tightening, filter sweeps, echoes, risers, impacts, and adaptive seamless or high-impact transitions.",
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
                                            ui.label("Mix engine:");
                                            ui.add_enabled_ui(!running, |ui| {
                                                ui.selectable_value(
                                                    &mut modal.options.style,
                                                    DjMixStyle::Crossfade,
                                                    "Crossfade",
                                                );
                                                ui.selectable_value(
                                                    &mut modal.options.style,
                                                    DjMixStyle::SmartDj,
                                                    "Smart DJ",
                                                );
                                            });
                                        });
                                        ui.small(match modal.options.style {
                                            DjMixStyle::Crossfade => "Simple equal-power overlap. No tempo change, loop roll, echo, or filter performance.",
                                            DjMixStyle::SmartDj => "Deterministic offline performance-DJ engine: phrase selection, pitch-preserving BPM sync, beat-repeat loops, stutter builds, filter automation, echo throws, synthetic risers and impacts, optional bass swap, and adaptive seamless or high-impact drops.",
                                        });
                                        ui.add_space(6.0);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.add_enabled_ui(!running, |ui| {
                                                ui.checkbox(&mut modal.options.smart_order, "Smart BPM order");
                                                ui.checkbox(&mut modal.options.normalize_loudness, "Loudness leveling");
                                                ui.checkbox(&mut modal.options.bass_swap, "Bass swap");
                                            });
                                        });
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Transition:");
                                            for bars in [8, 16, 32] {
                                                ui.add_enabled_ui(!running, |ui| {
                                                    ui.selectable_value(
                                                        &mut modal.options.transition_bars,
                                                        bars,
                                                        format!("{bars} bars"),
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
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Target length:");
                                            ui.add_enabled(
                                                !running,
                                                egui::DragValue::new(&mut modal.options.target_minutes)
                                                    .range(1.0..=180.0)
                                                    .speed(1.0)
                                                    .suffix(" min"),
                                            );
                                            ui.small("Auto highlight sections are shortened to approach target.");
                                        });
                                        ui.small("Both engines render outside playback callback. Only active section and transition buffers remain in RAM.");
                                    });

                                    ui.add_space(8.0);
                                    Self::render_modal_section(ui, |ui| {
                                        let show_progress = running
                                            || modal.completed
                                            || modal.stage != "Ready";
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(ui.available_width(), 48.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                if show_progress {
                                                    ui.horizontal(|ui| {
                                                        if running {
                                                            ui.spinner();
                                                        }
                                                        ui.label(&modal.stage);
                                                        if let Some(started_at) = modal.started_at {
                                                            let elapsed = started_at.elapsed().as_secs();
                                                            ui.small(format!("{}:{:02}", elapsed / 60, elapsed % 60));
                                                        }
                                                    });
                                                    ui.add(
                                                        egui::ProgressBar::new(modal.progress)
                                                            .animate(running)
                                                            .show_percentage(),
                                                    );
                                                    if running {
                                                        ui.small("Background worker active — playback and library remain usable.");
                                                    }
                                                }
                                            },
                                        );
                                        ui.small("Choose target length and section mode per track, then export MP3. A deterministic .dj-plan.json report is saved beside it.");
                                        if let Some(summary) = &modal.diagnostics_summary {
                                            ui.small(summary);
                                        }
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
                                                    if ui.button(ui_icons::label(Icon::Play, "Play")).clicked() {
                                                        play_path = Some(path.clone());
                                                    }
                                                    if ui.button(ui_icons::label(Icon::FolderOpen, "Show MP3")).clicked() {
                                                        reveal_path = Some(path);
                                                    }
                                                }
                                                if let Some(path) = modal.report_path.clone() {
                                                    if ui.button("Show diagnostics").clicked() {
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
                                                let track_count = modal.tracks.len();
                                                for index in 0..track_count {
                                                    let title = modal.tracks[index].title.clone();
                                                    let path = modal.tracks[index].path.clone();
                                                    let mut section_mode = modal.tracks[index].section_mode;
                                                    let mut favorite_start = modal.tracks[index].favorite_start_seconds;
                                                    let mut favorite_end = modal.tracks[index].favorite_end_seconds;
                                                    ui.horizontal(|ui| {
                                                        ui.label(format!("{}.", index + 1));
                                                        ui.vertical(|ui| {
                                                            ui.label(ellipsize_chars(&title, 72));
                                                            ui.small(ellipsize_chars(&path.display().to_string(), 96));
                                                            ui.horizontal_wrapped(|ui| {
                                                                ui.add_enabled_ui(!running, |ui| {
                                                                    ui.selectable_value(
                                                                        &mut section_mode,
                                                                        DjTrackSectionMode::AutoHighlight,
                                                                        "Auto highlight",
                                                                    );
                                                                    ui.selectable_value(
                                                                        &mut section_mode,
                                                                        DjTrackSectionMode::FullTrack,
                                                                        "Full track",
                                                                    );
                                                                    ui.selectable_value(
                                                                        &mut section_mode,
                                                                        DjTrackSectionMode::FavoriteRange,
                                                                        "Favorite range",
                                                                    );
                                                                });
                                                            });
                                                            if section_mode == DjTrackSectionMode::FavoriteRange {
                                                                ui.horizontal_wrapped(|ui| {
                                                                    ui.label("Start:");
                                                                    ui.add_enabled(
                                                                        !running,
                                                                        egui::DragValue::new(&mut favorite_start)
                                                                            .range(0.0..=86_400.0)
                                                                            .speed(1.0)
                                                                            .suffix(" s"),
                                                                    );
                                                                    ui.label("End:");
                                                                    ui.add_enabled(
                                                                        !running,
                                                                        egui::DragValue::new(&mut favorite_end)
                                                                            .range(0.0..=86_400.0)
                                                                            .speed(1.0)
                                                                            .suffix(" s"),
                                                                    );
                                                                });
                                                            }
                                                        });
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui
                                                                .add_enabled(!running && track_count > 2, egui::Button::new(ui_icons::icon(Icon::X)))
                                                                .on_hover_text("Remove from mix")
                                                                .clicked()
                                                            {
                                                                remove_index = Some(index);
                                                            }
                                                            if ui
                                                                .add_enabled(!running && index + 1 < track_count, egui::Button::new(ui_icons::icon(Icon::ArrowDown)))
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
                                                    modal.tracks[index].section_mode = section_mode;
                                                    modal.tracks[index].favorite_start_seconds = favorite_start.max(0.0);
                                                    modal.tracks[index].favorite_end_seconds = favorite_end.max(0.0);
                                                    if index + 1 < track_count {
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
        if let Some(path) = play_path {
            self.dj_mix_modal = None;
            self.open_audio_files_in_temporary_playlist(vec![path], true);
            return;
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
