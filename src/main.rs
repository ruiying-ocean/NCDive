mod app;
mod coastlines;
mod colour;
mod dataset;
mod export;
mod grid;
mod map;
mod map_data;
#[cfg(target_os = "macos")]
mod native_menu;
mod projection;
mod typography;

fn main() -> eframe::Result {
    #[cfg(target_os = "macos")]
    let native_events = native_menu::NativeEvents::new();
    let initial_file = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([720.0, 480.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "NCDive",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::NcDiveApp::new(
                cc,
                initial_file,
                #[cfg(target_os = "macos")]
                native_events,
            )))
        }),
    )
}
