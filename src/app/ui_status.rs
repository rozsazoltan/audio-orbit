use crate::*;

impl AudioOrbitApp {
    pub(crate) fn playlist_count_label(&self) -> Option<String> {
        if self.active_tab != MainContentTab::Music {
            return None;
        }
        let playlist = self.current_playlist()?;
        let total = playlist.tracks.len();
        if total == 0 {
            return None;
        }
        let visible = self.visible_track_indexes().len();
        if self.show_track_search && !self.track_search_query.trim().is_empty() && visible != total
        {
            Some(format!("{visible}/{total} tracks"))
        } else {
            Some(format!("{total} tracks"))
        }
    }
    pub(crate) fn render_status_panel(&self, ui: &mut egui::Ui) {
        self.render_status_panel_contents(ui, false);
    }
    pub(crate) fn render_status_panel_contents(&self, ui: &mut egui::Ui, include_error: bool) {
        let count_label = self.playlist_count_label();
        let body_font = egui::TextStyle::Body.resolve(ui.style());
        let small_font = egui::TextStyle::Small.resolve(ui.style());
        let text_color = ui.visuals().widgets.inactive.fg_stroke.color;
        let error_color = egui::Color32::from_rgb(255, 112, 112);
        let count_width = count_label
            .as_ref()
            .map(|label| text_width(ui, label, small_font.clone(), text_color) + 18.0)
            .unwrap_or(0.0);
        let available_width = ui.available_width();
        let media_width = if self.media_key_status.is_empty() {
            0.0
        } else {
            (text_width(ui, &self.media_key_status, small_font.clone(), text_color) + 16.0)
                .min(available_width * 0.35)
        };
        let primary_status = self
            .profile_apply_status_text()
            .unwrap_or_else(|| self.status_message.clone());
        let status_message = if include_error {
            self.error_message
                .as_deref()
                .unwrap_or(primary_status.as_str())
        } else {
            primary_status.as_str()
        };
        let status_color = if include_error && self.error_message.is_some() {
            error_color
        } else {
            text_color
        };
        let separator_width = if !status_message.is_empty() && !self.media_key_status.is_empty() {
            12.0
        } else {
            0.0
        };
        let available_status_width =
            (available_width - count_width - media_width - separator_width - 12.0).max(48.0);

        ui.horizontal(|ui| {
            if !status_message.is_empty() {
                render_ellipsized_single_line(
                    ui,
                    status_message,
                    available_status_width,
                    body_font.clone(),
                    status_color,
                );
                if !self.media_key_status.is_empty() {
                    ui.separator();
                }
            }
            if !self.media_key_status.is_empty() {
                let width = media_width.max(48.0);
                render_ellipsized_single_line(
                    ui,
                    &self.media_key_status,
                    width,
                    small_font.clone(),
                    text_color,
                );
            }

            if let Some(count_label) = count_label {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small(count_label);
                });
            }
        });
    }
    pub(crate) fn render_drag_feedback(&self, context: &egui::Context) {
        if self.dragging_track_index.is_some() || self.dragging_radio_index.is_some() {
            context.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
    pub(crate) fn render_error_toast(&mut self, context: &egui::Context) {
        let Some(error_message) = self.error_message.clone() else {
            return;
        };

        let screen_rect = context.screen_rect();
        let estimated_width =
            (error_message.chars().count() as f32 * 7.0 + 34.0).clamp(260.0, 720.0);
        let width = estimated_width.min((screen_rect.width() - 32.0).max(260.0));
        let estimated_lines = (error_message.chars().count() as f32 / 90.0)
            .ceil()
            .max(1.0);
        let max_height = (estimated_lines * 18.0 + 16.0).clamp(32.0, 112.0);
        egui::Area::new(egui::Id::new("error_toast_overlay"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0])
            .show(context, |ui| {
                ui.set_width(width);
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(238))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 54, 62)))
                    .corner_radius(egui::CornerRadius::same(0))
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        egui::ScrollArea::vertical()
                            .max_height(max_height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(error_message.as_str())
                                            .color(egui::Color32::from_rgb(255, 112, 112)),
                                    )
                                    .wrap(),
                                );
                            });
                    });
            });
    }
}
