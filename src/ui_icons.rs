use eframe::egui;
use lucide_icons::{Icon, LUCIDE_FONT_BYTES};
use std::sync::Arc;

const LUCIDE_FONT_NAME: &str = "lucide-icons";
const WINDOWS_TEXT_FONT_NAME: &str = "windows-ui-text";

pub fn install(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts.font_data.insert(
            WINDOWS_TEXT_FONT_NAME.to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, WINDOWS_TEXT_FONT_NAME.to_owned());
    }

    fonts.font_data.insert(
        LUCIDE_FONT_NAME.to_owned(),
        Arc::new(egui::FontData::from_static(LUCIDE_FONT_BYTES)),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(LUCIDE_FONT_NAME.to_owned());
    }

    context.set_fonts(fonts);
}

pub fn icon(icon: Icon) -> String {
    char::from(icon).to_string()
}

pub fn label(icon: Icon, text: impl AsRef<str>) -> String {
    format!("{} {}", char::from(icon), text.as_ref())
}
