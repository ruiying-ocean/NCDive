use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub const SEMIBOLD: &str = "Inter SemiBold";

pub fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        (
            "Inter",
            include_bytes!("../assets/Inter-Regular.otf").as_slice(),
        ),
        (
            SEMIBOLD,
            include_bytes!("../assets/Inter-SemiBold.otf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(bytes).into());
    }
    let proportional = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
    proportional.insert(0, "Inter".into());
    let mut heading = proportional.clone();
    heading.insert(0, SEMIBOLD.into());
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), heading);
    fonts
}

pub fn heading(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn configure(context: &egui::Context) {
    context.set_fonts(fonts());
    context.all_styles_mut(|style| {
        style.text_styles.extend([
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Heading, heading(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]);
        style.drag_value_text_style = TextStyle::Body;
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size.y = 24.0;
        style.visuals.weak_text_color = Some(egui::Color32::from_gray(105));
    });
}
