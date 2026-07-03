use crate::*;

impl AudioOrbitApp {
    pub(crate) fn render_folder_import_window(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "folder_import_modal_backdrop");
        let mut is_open = self.show_folder_import_modal;
        let mut close_after_import = false;
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let scroll_height = (content_size.y - 78.0).max(120.0);

        egui::Area::new(egui::Id::new("folder_import_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);

                    if Self::render_modal_header(
                        ui,
                        outer_padding.x,
                        Icon::FolderPlus,
                        "Add music folder",
                        "Create a scanner-owned playlist from a folder and group tracks by the first N subfolder levels.",
                    ) {
                        close_after_import = true;
                    }

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            Self::render_modal_section(ui, |ui| {
                                let form_width = ui.available_width().min(720.0);
                                ui.label("Folder");
                                let folder_label = self
                                    .pending_folder_path
                                    .as_ref()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "No folder selected".to_owned());
                                ui.add(egui::Label::new(folder_label).wrap());

                                if ui.button(ui_icons::label(Icon::FolderOpen, "Choose folder...")).clicked() {
                                    self.pick_music_folder();
                                }

                                ui.add_space(6.0);
                                ui.label("Playlist name");
                                ui.add_sized(
                                    egui::vec2(form_width, 24.0),
                                    egui::TextEdit::singleline(&mut self.pending_playlist_name),
                                );

                                ui.add_space(6.0);
                                ui.add(
                                    egui::Slider::new(&mut self.pending_folder_depth, 0usize..=5usize)
                                        .text("Group by folder levels"),
                                );
                                ui.small(r"Example: depth 2 groups D:\mp3\Artist\Album\song.mp3 as Artist / Album.");

                                ui.add_space(10.0);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button(ui_icons::label(Icon::FolderInput, "Import folder")).clicked() {
                                        close_after_import = self.import_folder_playlist();
                                    }
                                });
                            });
                        });
                });
            });
        self.render_modal_info_footer_fixed(context, "folder_import_modal_info_footer", screen_rect);

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            close_after_import = true;
        }

        if close_after_import {
            is_open = false;
            self.pending_folder_path = None;
        }

        self.show_folder_import_modal = is_open;
    }
}
