use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use eframe::egui::{self, Color32, RichText};

use crate::dataset::{AttributeInfo, Dataset, DimensionInfo, VariableInfo};

const ACCENT: Color32 = Color32::from_rgb(16, 102, 112);

#[derive(Default)]
pub struct NcDiveApp {
    dataset: Option<Dataset>,
    selected: Option<usize>,
    search: String,
    loading: Option<(PathBuf, Receiver<Result<Dataset, String>>)>,
    error: Option<String>,
    map: crate::map::MapPanel,
    metadata: bool,
    exporting: Option<Receiver<Result<PathBuf, String>>>,
    export_message: Option<Result<PathBuf, String>>,
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::native_menu::FileMenu>,
}

impl NcDiveApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_file: Option<PathBuf>,
        #[cfg(target_os = "macos")] native_events: crate::native_menu::NativeEvents,
    ) -> Self {
        let mut visuals = egui::Visuals::light();
        visuals.selection.bg_fill = Color32::from_rgb(209, 235, 237);
        visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
        cc.egui_ctx.set_visuals(visuals);
        crate::typography::configure(&cc.egui_ctx);
        let mut app = Self {
            #[cfg(target_os = "macos")]
            native_menu: Some(crate::native_menu::FileMenu::new(
                &cc.egui_ctx,
                native_events,
            )),
            ..Self::default()
        };
        if let Some(path) = initial_file {
            app.open(path);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        let (sender, receiver) = mpsc::channel();
        let worker_path = path.clone();
        self.error = None;
        match thread::Builder::new()
            .name("netcdf-reader".to_owned())
            .spawn(move || {
                let _ = sender.send(Dataset::load(&worker_path));
            }) {
            Ok(_) => self.loading = Some((path, receiver)),
            Err(error) => self.error = Some(format!("Could not start reading the file: {error}")),
        }
    }

    fn can_export(&self) -> bool {
        self.loading.is_none()
            && self.exporting.is_none()
            && self.selected.is_some()
            && !self.metadata
            && self.map.can_export()
    }

    fn export(&mut self, extension: &'static str) {
        if !self.can_export() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(extension.to_uppercase(), &[extension])
            .set_file_name(format!("ncdive-figure.{extension}"))
            .save_file()
        else {
            return;
        };
        if !path
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(extension))
        {
            self.export_message = Some(Err(format!("Choose a filename ending in .{extension}.")));
            return;
        }
        self.export_message = None;
        match self.map.export(path, extension) {
            Ok(receiver) => self.exporting = Some(receiver),
            Err(error) => self.export_message = Some(Err(error)),
        }
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.exporting else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err("The figure exporter stopped unexpectedly.".into())
            }
        };
        self.exporting = None;
        self.export_message = Some(result);
    }

    fn poll_load(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.loading else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(
                "The file reader stopped unexpectedly. Please try opening the file again."
                    .to_owned(),
            ),
        };
        self.loading = None;
        match result {
            Ok(dataset) => {
                self.dataset = Some(dataset);
                self.selected = None;
                self.search.clear();
                self.map = crate::map::MapPanel::default();
            }
            Err(error) => self.error = Some(error),
        }
    }
}

impl eframe::App for NcDiveApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dropped_files = ui.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        match dropped_files.as_slice() {
            [] => {}
            [_] if self.loading.is_some() => {
                self.error = Some(
                    "Please drop the file again after the current one finishes opening.".into(),
                );
            }
            [file] => self.open(file.path().to_owned()),
            _ => self.error = Some("Please drop one NetCDF file at a time.".into()),
        }
        #[cfg(target_os = "macos")]
        let mut open_requested = false;
        #[cfg(not(target_os = "macos"))]
        let mut open_requested =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::O));

        let mut export_requested = None;
        #[cfg(target_os = "macos")]
        while let Some(action) = self
            .native_menu
            .as_ref()
            .and_then(|menu| menu.take_action())
        {
            match action {
                crate::native_menu::Action::Open => open_requested = true,
                crate::native_menu::Action::ExportPng => export_requested = Some("png"),
                crate::native_menu::Action::ExportPdf => export_requested = Some("pdf"),
                crate::native_menu::Action::OpenDocument(Ok(path)) => self.open(path),
                crate::native_menu::Action::OpenDocument(Err(error)) => self.error = Some(error),
            }
        }
        self.poll_load(ui.ctx());
        self.poll_export(ui.ctx());
        #[cfg(not(target_os = "macos"))]
        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::COMMAND,
                        egui::Key::O,
                    ));
                    if ui
                        .add_enabled(
                            self.loading.is_none(),
                            egui::Button::new("Open NetCDF file…").shortcut_text(shortcut),
                        )
                        .clicked()
                    {
                        open_requested = true;
                        ui.close();
                    }
                    ui.separator();
                    for (label, extension) in [("Export PNG…", "png"), ("Export PDF…", "pdf")] {
                        if ui
                            .add_enabled(self.can_export(), egui::Button::new(label))
                            .on_disabled_hover_text(
                                "Select a plot and wait for it to finish loading.",
                            )
                            .clicked()
                        {
                            export_requested = Some(extension);
                            ui.close();
                        }
                    }
                });
            });
        });

        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(242, 245, 246))
                    .inner_margin(10),
            )
            .show(ui, |ui| {
                if let Some((path, _)) = &self.loading {
                    ui.label(format!("Opening {}…", path.display()));
                } else if self.exporting.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Saving figure…");
                    });
                } else if let Some(result) = &self.export_message {
                    match result {
                        Ok(path) => {
                            ui.label(format!("Saved {}", path.display()));
                        }
                        Err(error) => {
                            ui.colored_label(Color32::DARK_RED, error);
                        }
                    }
                } else {
                    ui.label(
                        RichText::new("NetCDF explorer  ·  Read only")
                            .small()
                            .weak(),
                    );
                }
            });

        if let Some(dataset) = &self.dataset {
            egui::Panel::left("variables")
                .default_size(270.0)
                .size_range(200.0..=440.0)
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgb(247, 249, 249))
                        .inner_margin(16),
                )
                .show(ui, |ui| {
                    ui.label(RichText::new("VARIABLES").small().strong().color(ACCENT));
                    ui.add_space(10.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Find a variable…")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(12.0);
                    if ui
                        .selectable_label(self.selected.is_none(), "Dataset overview")
                        .clicked()
                    {
                        self.selected = None;
                    }
                    ui.separator();
                    let query = self.search.to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_salt("variable_list")
                        .show(ui, |ui| {
                            let mut matches = 0;
                            for (index, variable) in dataset.variables.iter().enumerate() {
                                if !variable.path.to_lowercase().contains(&query)
                                    && !variable
                                        .attribute("long_name")
                                        .unwrap_or("")
                                        .to_lowercase()
                                        .contains(&query)
                                {
                                    continue;
                                }
                                matches += 1;
                                if ui
                                    .add_sized(
                                        [ui.available_width(), 30.0],
                                        egui::Button::new(&variable.path)
                                            .selected(self.selected == Some(index)),
                                    )
                                    .on_hover_text(
                                        variable.attribute("long_name").unwrap_or(&variable.path),
                                    )
                                    .clicked()
                                {
                                    self.selected = Some(index);
                                }
                            }
                            if matches == 0 {
                                ui.label(if dataset.variables.is_empty() {
                                    "This dataset has no variables."
                                } else {
                                    "No matching variables."
                                });
                            }
                        });
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(24))
            .show(ui, |ui| {
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(165, 35, 35), error);
                    ui.add_space(12.0);
                    ui.separator();
                }
                if let Some(dataset) = &self.dataset {
                    egui::ScrollArea::vertical()
                        .id_salt(("details", self.selected))
                        .show(ui, |ui| {
                            if let Some(variable) =
                                self.selected.and_then(|index| dataset.variables.get(index))
                            {
                                variable_header(ui, variable);
                                ui.horizontal(|ui| {
                                    ui.selectable_value(&mut self.metadata, false, "Plot");
                                    ui.selectable_value(&mut self.metadata, true, "Metadata");
                                });
                                ui.separator();
                                if self.metadata {
                                    variable_details(ui, variable);
                                } else {
                                    self.map.show(ui, &dataset.path, variable);
                                }
                            } else {
                                dataset_details(ui, dataset);
                            }
                        });
                } else {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.vertical_centered(|ui| {
                        ui.heading(if ui.input(|input| input.raw.hovered_files.is_empty()) {
                            "Explore a NetCDF dataset"
                        } else {
                            "Drop to open your NetCDF dataset"
                        });
                        ui.add_space(12.0);
                        ui.label("Browse variables, dimensions, units and file metadata.");
                        ui.add_space(24.0);
                        ui.label("Drag a NetCDF file here, or choose File → Open NetCDF file….");
                    });
                }
            });

        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.native_menu {
            menu.set_enabled(self.loading.is_none(), self.can_export());
        }

        if let Some(extension) = export_requested {
            self.export(extension);
        }

        if open_requested
            && self.loading.is_none()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("NetCDF datasets", &["nc", "nc4", "cdf"])
                .add_filter("All files", &["*"])
                .pick_file()
        {
            self.open(path);
        }
    }
}

fn dataset_details(ui: &mut egui::Ui, dataset: &Dataset) {
    ui.label(
        RichText::new("DATASET OVERVIEW")
            .small()
            .strong()
            .color(ACCENT),
    );
    ui.add_space(8.0);
    ui.heading(
        dataset
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
    );
    ui.add(
        egui::Label::new(dataset.path.to_string_lossy())
            .wrap()
            .selectable(true),
    );
    ui.add_space(12.0);
    ui.label(format!(
        "{} variables  ·  {} groups (including root)",
        dataset.variables.len(),
        dataset.groups.len()
    ));
    ui.add_space(20.0);
    for group in &dataset.groups {
        ui.push_id(&group.path, |ui| {
            if group.path == "/" {
                dimensions(ui, &group.dimensions);
                ui.add_space(20.0);
                ui.heading("Global attributes");
                attributes(ui, &group.attributes);
            } else {
                ui.add_space(12.0);
                egui::CollapsingHeader::new(format!("Group {}", group.path)).show(ui, |ui| {
                    dimensions(ui, &group.dimensions);
                    ui.add_space(12.0);
                    ui.strong("Group attributes");
                    attributes(ui, &group.attributes);
                });
            }
        });
    }
}

fn variable_header(ui: &mut egui::Ui, variable: &VariableInfo) {
    ui.label(RichText::new("VARIABLE").small().strong().color(ACCENT));
    ui.add_space(8.0);
    ui.heading(&variable.path);
    if let Some(long_name) = variable.attribute("long_name") {
        ui.label(long_name);
    }
    ui.add_space(16.0);
    ui.horizontal_wrapped(|ui| {
        ui.strong("Type");
        ui.label(&variable.data_type);
        ui.separator();
        ui.strong("Shape");
        ui.label(variable.shape());
        ui.separator();
        ui.strong("Units");
        ui.label(variable.attribute("units").unwrap_or("Not specified"));
    });
    ui.add_space(12.0);
}

fn variable_details(ui: &mut egui::Ui, variable: &VariableInfo) {
    ui.add_space(12.0);
    dimensions(ui, &variable.dimensions);
    ui.add_space(24.0);
    ui.heading("Attributes");
    attributes(ui, &variable.attributes);
}

fn dimensions(ui: &mut egui::Ui, dimensions: &[DimensionInfo]) {
    ui.heading("Dimensions");
    ui.add_space(8.0);
    if dimensions.is_empty() {
        ui.label("No dimensions.");
        return;
    }
    egui::Grid::new("dimensions")
        .striped(true)
        .spacing([24.0, 10.0])
        .show(ui, |ui| {
            ui.strong("Name");
            ui.strong("Length");
            ui.strong("Kind");
            ui.end_row();
            for dimension in dimensions {
                ui.monospace(&dimension.name);
                ui.label(dimension.length.to_string());
                ui.label(if dimension.unlimited {
                    "Unlimited"
                } else {
                    "Fixed"
                });
                ui.end_row();
            }
        });
}

fn attributes(ui: &mut egui::Ui, attributes: &[AttributeInfo]) {
    ui.add_space(8.0);
    if attributes.is_empty() {
        ui.label("No attributes.");
        return;
    }
    egui::Grid::new("attributes")
        .striped(true)
        .spacing([24.0, 12.0])
        .max_col_width((ui.available_width() - 180.0).max(140.0))
        .show(ui, |ui| {
            for attribute in attributes {
                ui.monospace(&attribute.name);
                match &attribute.value {
                    Ok(value) => {
                        ui.add(egui::Label::new(value).wrap().selectable(true));
                    }
                    Err(error) => {
                        ui.colored_label(
                            Color32::from_rgb(165, 35, 35),
                            format!("Could not read: {error}"),
                        );
                    }
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App as _;
    use std::{path::Path, sync::Arc};

    #[derive(Debug)]
    struct DroppedFile(PathBuf);

    impl egui::DroppedFile for DroppedFile {
        fn path(&self) -> &Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            panic!("Dropped datasets should be opened by path in the background");
        }
    }

    fn sample(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(name)
    }

    fn frame(app: &mut NcDiveApp, ctx: &egui::Context, paths: &[PathBuf]) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                dropped_files: paths
                    .iter()
                    .map(|path| Arc::new(DroppedFile(path.clone())) as egui::DroppedFileHandle)
                    .collect(),
                ..Default::default()
            },
            |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
        );
        output.textures_delta.clear();
    }

    fn finish_load(app: &mut NcDiveApp, ctx: &egui::Context) {
        if let Some((path, receiver)) = app.loading.take() {
            let result = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            let (sender, receiver) = mpsc::channel();
            sender.send(result).unwrap();
            app.loading = Some((path, receiver));
            frame(app, ctx, &[]);
        }
        assert!(app.loading.is_none());
    }

    #[test]
    fn dropped_files_open_and_replace_the_dataset() {
        let ctx = egui::Context::default();
        let mut app = NcDiveApp::default();
        for name in ["temperature.nc", "groups.nc"] {
            let path = sample(name);
            frame(&mut app, &ctx, std::slice::from_ref(&path));
            finish_load(&mut app, &ctx);
            assert_eq!(app.dataset.as_ref().unwrap().path, path);
            assert!(app.error.is_none());
            assert!(app.selected.is_none());
            assert!(app.search.is_empty());
            app.selected = Some(0);
            app.search = "temperature".into();
        }
    }

    #[test]
    fn invalid_drop_keeps_the_current_dataset() {
        let ctx = egui::Context::default();
        let path = sample("temperature.nc");
        let mut app = NcDiveApp {
            dataset: Some(Dataset::load(&path).unwrap()),
            selected: Some(0),
            ..Default::default()
        };
        frame(&mut app, &ctx, &[sample("temperature.cdl")]);
        finish_load(&mut app, &ctx);
        assert_eq!(app.dataset.as_ref().unwrap().path, path);
        assert_eq!(app.selected, Some(0));
        assert!(app.error.as_ref().unwrap().contains("Could not open"));
    }

    #[test]
    fn multiple_dropped_files_show_an_error_without_loading() {
        let ctx = egui::Context::default();
        let mut app = NcDiveApp::default();
        frame(
            &mut app,
            &ctx,
            &[sample("temperature.nc"), sample("groups.nc")],
        );
        assert!(app.loading.is_none());
        assert_eq!(
            app.error.as_deref(),
            Some("Please drop one NetCDF file at a time.")
        );
    }

    #[test]
    fn drop_during_loading_does_not_replace_the_pending_file() {
        let ctx = egui::Context::default();
        let path = sample("temperature.nc");
        let (_sender, receiver) = mpsc::channel();
        let mut app = NcDiveApp {
            loading: Some((path.clone(), receiver)),
            ..Default::default()
        };
        frame(&mut app, &ctx, &[sample("groups.nc")]);
        assert_eq!(app.loading.as_ref().unwrap().0, path);
        assert_eq!(
            app.error.as_deref(),
            Some("Please drop the file again after the current one finishes opening.")
        );
    }
}
