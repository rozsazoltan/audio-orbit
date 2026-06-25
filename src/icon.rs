use egui::IconData;

pub fn load_window_icon() -> Option<IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/audio-orbit.ico")).ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();

    Some(IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}
