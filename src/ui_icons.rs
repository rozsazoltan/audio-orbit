use eframe::egui;
use lucide_icons::{Icon, LUCIDE_FONT_BYTES};
use std::sync::Arc;

const LUCIDE_FONT_NAME: &str = "lucide-icons";

pub fn install(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        LUCIDE_FONT_NAME.to_owned(),
        Arc::new(egui::FontData::from_static(LUCIDE_FONT_BYTES)),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, LUCIDE_FONT_NAME.to_owned());
    }

    context.set_fonts(fonts);
}

pub fn icon(icon: Icon) -> String {
    char::from(icon).to_string()
}

pub fn label(icon: Icon, text: impl AsRef<str>) -> String {
    format!("{} {}", char::from(icon), text.as_ref())
}
