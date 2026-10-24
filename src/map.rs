use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};

use crate::{
    colour::{ColourScale, MISSING, NON_POSITIVE, Palette},
    dataset::{Dataset, VariableInfo},
    grid::{GeoPolygon, contains},
    map_data::{
        Axis, AxisKind, MapSlice, PlotKind, Selector, longitude_parts, number, wrap_longitude,
    },
    projection::{Projection, Projector},
};

const INK: Color32 = Color32::from_rgb(46, 60, 65);

#[derive(Default)]
pub struct MapPanel {
    key: Option<(PathBuf, String)>,
    indices: Vec<usize>,
    loading: Option<Receiver<Result<MapSlice, String>>>,
    view: Option<MapView>,
    error: Option<String>,
    projection: Projection,
    kind: PlotKind,
    palette: Palette,
    logarithmic: bool,
    coastlines: bool,
    inspect_cells: bool,
    missing_text: String,
    missing_values: Vec<f64>,
    missing_error: Option<String>,
    reference: Option<Reference>,
    reference_loading: Option<Receiver<Result<Dataset, String>>>,
    reference_error: Option<String>,
}

struct Reference {
    dataset: Dataset,
    variable: usize,
    indices: Vec<usize>,
}

struct MapView {
    data: MapSlice,
    scale: ColourScale,
    mesh: Option<(Rect, ColourScale, Result<Shape, String>)>,
    coastlines: Option<(Rect, Vec<Shape>)>,
    cell: [usize; 2],
    projector: Result<Projector, String>,
}

impl MapPanel {
    pub fn can_export(&self) -> bool {
        self.loading.is_none()
            && self.reference_loading.is_none()
            && self
                .view
                .as_ref()
                .is_some_and(|view| view.scale.validate().is_ok() && view.projector.is_ok())
    }

    pub fn export(
        &self,
        path: PathBuf,
        extension: &'static str,
    ) -> Result<Receiver<Result<PathBuf, String>>, String> {
        if !self.can_export() {
            return Err("Select a valid plot before exporting a figure.".into());
        }
        let view = self.view.as_ref().unwrap();
        let data = view.data.clone();
        let scale = view.scale;
        let projection = view.projector.as_ref().unwrap().kind;
        let coastlines = self.coastlines;
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("figure-export".into())
            .spawn(move || {
                let result = figure_svg(data, scale, projection, coastlines)
                    .and_then(|svg| crate::export::encode(&svg, extension))
                    .and_then(|bytes| crate::export::save(&path, &bytes).map(|()| path));
                let _ = sender.send(result);
            })
            .map_err(|error| format!("Could not start export: {error}"))?;
        Ok(receiver)
    }

    fn load(&mut self, choose_view: bool) {
        let Some((path, variable)) = self.key.clone() else {
            return;
        };
        let indices = self.indices.clone();
        let missing_values = self.missing_values.clone();
        let kind = self.kind;
        let reference = self.reference.as_ref().map(|r| {
            (
                r.dataset.path.clone(),
                r.dataset.variables[r.variable].path.clone(),
                r.indices.clone(),
            )
        });
        let (sender, receiver) = mpsc::channel();
        self.error = None;
        self.loading = None;
        match thread::Builder::new()
            .name("plot-reader".into())
            .spawn(move || {
                let kind = if choose_view {
                    MapSlice::suggested_kind(&path, &variable, kind)
                } else {
                    Ok(kind)
                };
                let result = kind
                    .and_then(|kind| {
                        MapSlice::load(&path, &variable, &indices, kind, &missing_values)
                    })
                    .and_then(|mut data| {
                        if let Some((path, variable, indices)) = reference {
                            let other = MapSlice::load(
                                &path,
                                &variable,
                                &indices,
                                data.kind,
                                &missing_values,
                            )?;
                            data.subtract(&other)?;
                        }
                        Ok(data)
                    })
                    .map_err(|e| e.to_string());
                let _ = sender.send(result);
            }) {
            Ok(_) => self.loading = Some(receiver),
            Err(error) => self.error = Some(format!("Could not load this plot: {error}")),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, path: &Path, variable: &VariableInfo) {
        if self.key.as_ref().is_none_or(|(old_path, old_variable)| {
            old_path != path || old_variable != &variable.path
        }) {
            let same_file = self
                .key
                .as_ref()
                .is_some_and(|(old_path, _)| old_path == path);
            // Dropping the old receiver prevents an old request replacing the selected variable.
            *self = Self {
                key: Some((path.to_owned(), variable.path.clone())),
                indices: vec![0; variable.dimensions.len()],
                projection: self.projection,
                kind: self.kind,
                palette: self.palette,
                logarithmic: self.logarithmic,
                coastlines: self.coastlines,
                inspect_cells: self.inspect_cells,
                missing_text: if same_file {
                    std::mem::take(&mut self.missing_text)
                } else {
                    String::new()
                },
                missing_values: if same_file {
                    std::mem::take(&mut self.missing_values)
                } else {
                    Vec::new()
                },
                missing_error: if same_file {
                    self.missing_error.take()
                } else {
                    None
                },
                ..Self::default()
            };
            self.load(true);
        }
        let previous_kind = self.kind;
        egui::ComboBox::from_label("View")
            .selected_text(self.kind.name())
            .show_ui(ui, |ui| {
                for kind in PlotKind::ALL {
                    ui.selectable_value(&mut self.kind, kind, kind.name());
                }
            });
        if self.kind != previous_kind {
            self.view = None;
            self.load(false);
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Missing values");
            let input = ui.add(
                egui::TextEdit::singleline(&mut self.missing_text)
                    .hint_text("e.g. -1e35, -9999")
                    .desired_width(210.0),
            ).on_hover_text("Extra values to treat as missing, separated by commas or spaces. Use stored values, before scaling. Add 0 only when zero means missing. Applies to both inputs in a comparison.");
            if ui.button("Apply").clicked()
                || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                match parse_missing_values(&self.missing_text) {
                    Ok(values) => {
                        self.missing_error = None;
                        if values != self.missing_values {
                            self.missing_values = values;
                            self.view = None;
                            self.load(false);
                        }
                    }
                    Err(error) => self.missing_error = Some(error),
                }
            }
        });
        if let Some(error) = &self.missing_error {
            ui.colored_label(Color32::DARK_RED, error);
        }
        if let Some(receiver) = &self.reference_loading {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                    None
                }
                Err(TryRecvError::Disconnected) => {
                    Some(Err("The reference-file reader stopped unexpectedly.".into()))
                }
            };
            if let Some(result) = result {
                self.reference_loading = None;
                match result {
                    Ok(dataset) if !dataset.variables.is_empty() => {
                        let index = dataset
                            .variables
                            .iter()
                            .position(|v| v.path == variable.path)
                            .or_else(|| {
                                dataset
                                    .variables
                                    .iter()
                                    .position(|v| v.dimensions.len() >= 2)
                            })
                            .unwrap_or(0);
                        let indices = vec![0; dataset.variables[index].dimensions.len()];
                        self.reference = Some(Reference {
                            dataset,
                            variable: index,
                            indices,
                        });
                        self.palette = Palette::BlueRed;
                        self.logarithmic = false;
                        self.view = None;
                        self.load(false);
                    }
                    Ok(_) => {
                        self.reference_error = Some("The reference file has no variables.".into())
                    }
                    Err(error) => self.reference_error = Some(error),
                }
            }
        }
        let mut reference_changed = false;
        let mut remove_reference = false;
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.reference_loading.is_none(), egui::Button::new("Reference file…"))
                .on_hover_text("Subtract a variable from another file, or choose this file to compare two slices.")
                .clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("NetCDF datasets", &["nc", "nc4", "cdf"]).pick_file() {
                let (sender, receiver) = mpsc::channel();
                self.reference_error = None;
                match thread::Builder::new().name("reference-reader".into()).spawn(move || { let _ = sender.send(Dataset::load(&path)); }) {
                    Ok(_) => self.reference_loading = Some(receiver),
                    Err(error) => self.reference_error = Some(format!("Could not open reference: {error}")),
                }
            }
            if self.reference_loading.is_some() { ui.spinner(); ui.label("Opening reference…"); }
            if let Some(reference) = &mut self.reference {
                ui.label(reference.dataset.path.file_name().unwrap_or_default().to_string_lossy());
                let old_variable = reference.variable;
                egui::ComboBox::from_id_salt("reference-variable")
                    .selected_text(&reference.dataset.variables[reference.variable].path)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for (index, variable) in reference.dataset.variables.iter().enumerate() {
                            ui.selectable_value(&mut reference.variable, index, &variable.path);
                        }
                    });
                if reference.variable != old_variable {
                    reference.indices = vec![0; reference.dataset.variables[reference.variable].dimensions.len()];
                    reference_changed = true;
                }
                remove_reference = ui.button("Remove comparison").clicked();
            }
        });
        if let Some(error) = &self.reference_error {
            ui.colored_label(Color32::DARK_RED, error);
        }
        if remove_reference {
            self.reference = None;
            self.reference_error = None;
            self.view = None;
            self.load(false);
        } else if reference_changed {
            self.view = None;
            self.load(false);
        }
        if let Some(receiver) = &self.loading {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                    None
                }
                Err(TryRecvError::Disconnected) => Some(Err(
                    "The plot reader stopped unexpectedly. Select another variable and try again."
                        .into(),
                )),
            };
            if let Some(result) = result {
                self.loading = None;
                match result {
                    Ok(data) => {
                        if self.kind != data.kind {
                            self.kind = data.kind;
                            ui.ctx().request_repaint();
                        }
                        let mut scale = ColourScale {
                            limits: [0.0, 1.0],
                            palette: self.palette,
                            logarithmic: self.logarithmic,
                        };
                        if let Some(view) = &self.view {
                            scale.limits = view.scale.limits;
                        } else {
                            scale.auto_range(&data.values, data.reference.is_some());
                        }
                        self.view = Some(MapView::new(data, self.projection, scale));
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.view = None;
                    }
                }
            }
        }
        let mut changed = false;
        if let Some(view) = &self.view {
            ui.add_enabled_ui(self.loading.is_none(), |ui| {
                if self.reference.is_some() {
                    ui.strong("Current slice");
                }
                changed |= selectors(ui, &view.data.selectors, &mut self.indices);
                if let Some(reference) = &mut self.reference {
                    ui.strong("Reference slice");
                    changed |=
                        selectors(ui, &view.data.reference_selectors, &mut reference.indices);
                }
            });
        }
        if changed {
            self.load(false);
        }
        if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading plot…");
            });
        } else if let Some(error) = &self.error {
            ui.add_space(12.0);
            ui.colored_label(Color32::DARK_RED, error);
            ui.label("The Metadata tab is still available.");
        } else if let Some(view) = &mut self.view {
            if self.kind == PlotKind::Map {
                let previous = self.projection;
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_label("Projection").selected_text(self.projection.name()).show_ui(ui, |ui| {
                        for projection in Projection::ALL { ui.selectable_value(&mut self.projection, projection, projection.name()); }
                    });
                    ui.checkbox(&mut self.coastlines, "Modern coastlines")
                        .on_hover_text("Present-day Natural Earth coastlines. Leave off when they do not match the model's geography.");
                });
                ui.small(self.projection.description());
                if self.projection != previous {
                    view.projector = Projector::new(self.projection);
                    view.mesh = None;
                    view.coastlines = None;
                }
            }
            view.show(ui, self.coastlines, &mut self.inspect_cells);
            self.palette = view.scale.palette;
            self.logarithmic = view.scale.logarithmic;
        }
    }
}

fn parse_missing_values(input: &str) -> Result<Vec<f64>, String> {
    input
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("Not applied: ‘{part}’ is not a finite number. Separate values with commas or spaces."))
        })
        .collect()
}

fn selectors(ui: &mut egui::Ui, selectors: &[Selector], indices: &mut [usize]) -> bool {
    let mut changed = false;
    for selector in selectors {
        ui.horizontal_wrapped(|ui| {
            if selector.length > 1 {
                changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut indices[selector.dimension],
                            0..=selector.length - 1,
                        )
                        .text(&selector.name),
                    )
                    .changed();
            } else {
                ui.strong(&selector.name);
            }
            ui.label(&selector.label).on_hover_text(
                "Coordinate value as stored in the file; calendar dates are not converted.",
            );
        });
    }
    changed
}

impl MapView {
    fn new(data: MapSlice, projection: Projection, scale: ColourScale) -> Self {
        Self {
            data,
            scale,
            mesh: None,
            coastlines: None,
            cell: [0, 0],
            projector: Projector::new(projection),
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, coastlines: bool, inspect_cells: &mut bool) {
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_label("Palette")
                .selected_text(self.scale.palette.name())
                .show_ui(ui, |ui| {
                    for palette in Palette::ALL {
                        ui.selectable_value(&mut self.scale.palette, palette, palette.name());
                    }
                });
            let old_log = self.scale.logarithmic;
            egui::ComboBox::from_label("Scale")
                .selected_text(if old_log { "Log₁₀" } else { "Linear" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.scale.logarithmic, false, "Linear");
                    ui.selectable_value(&mut self.scale.logarithmic, true, "Log₁₀");
                });
            ui.checkbox(inspect_cells, "Inspect cells").on_hover_text(
                "Show exact values by pointing at a cell or choosing its row and column.",
            );
            if self.scale.logarithmic != old_log {
                self.scale
                    .auto_range(&self.data.values, self.data.reference.is_some());
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.strong("Colour limits");
            let speed = ((self.scale.limits[1] - self.scale.limits[0]).abs() / 100.0).max(1e-12);
            ui.label("Min");
            ui.add(egui::DragValue::new(&mut self.scale.limits[0]).speed(speed).max_decimals(12));
            ui.label("Max");
            ui.add(egui::DragValue::new(&mut self.scale.limits[1]).speed(speed).max_decimals(12));
            if ui.button("Auto range").on_hover_text("Fit this slice. Differences use limits centred on zero; log scales use positive values.").clicked() {
                self.scale.auto_range(&self.data.values, self.data.reference.is_some());
            }
            ui.label(if self.data.units.is_empty() { "Units not specified" } else { &self.data.units });
        });
        if let Err(error) = self.scale.validate() {
            ui.colored_label(Color32::DARK_RED, error);
            return;
        }
        if let Some(reference) = &self.data.reference {
            ui.strong("Difference: current − reference");
            ui.small(format!("Current: {}", self.data.source));
            ui.small(format!("Reference: {reference}"));
        }
        ui.small(format!("{} · no smoothing", self.scale.name()));
        let projection = match &self.projector {
            Ok(p) => p,
            Err(error) => {
                ui.colored_label(Color32::DARK_RED, error);
                return;
            }
        };
        let (outer_size, offset, plot_size) =
            plot_layout(ui.available_width(), self.data.kind, projection);
        let (outer, response) = ui.allocate_exact_size(outer_size, egui::Sense::hover());
        let rect = Rect::from_min_size(outer.min + offset, plot_size);
        if let Err(error) = self.paint(ui.painter(), rect, coastlines) {
            ui.colored_label(Color32::DARK_RED, error);
            return;
        }
        if *inspect_cells {
            let projection = self.projector.as_ref().unwrap();
            if let Some(position) = response.hover_pos()
                && rect.contains(position)
                && ui.input(|i| i.pointer.delta() != Vec2::ZERO)
                && let Some(cell) = cell_at(&self.data, projection, rect, position)
            {
                self.cell = cell;
            }
            let outline = if let Some(cells) = &self.data.cell_polygons {
                cells[self.cell[0] * self.data.shape[1] + self.cell[1]]
                    .iter()
                    .filter_map(|p| projected_polygon(projection, rect, p))
                    .collect()
            } else if self.data.kind == PlotKind::Map {
                longitude_parts(self.data.x.bounds[self.cell[1]])
                    .into_iter()
                    .filter_map(|part| {
                        cell_outline(projection, rect, part, self.data.y.bounds[self.cell[0]])
                    })
                    .collect::<Vec<_>>()
            } else {
                let cell = section_cell_rect(&self.data, rect, self.cell[0], self.cell[1]);
                vec![vec![
                    cell.left_top(),
                    cell.right_top(),
                    cell.right_bottom(),
                    cell.left_bottom(),
                ]]
            };
            for points in outline {
                ui.painter()
                    .add(Shape::closed_line(points, Stroke::new(1.0, Color32::WHITE)));
            }
        }
        let (bar_outer, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 65.0), egui::Sense::hover());
        draw_colorbar(
            ui.painter(),
            Rect::from_min_size(
                bar_outer.min + Vec2::new(offset.x, 0.0),
                Vec2::new(plot_size.x, 14.0),
            ),
            self.scale,
            &self.data.units,
        );
        ui.horizontal_wrapped(|ui| {
            let (swatch, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
            ui.painter().rect_filled(swatch, 0.0, MISSING);
            ui.label(format!(
                "Missing / masked: {} cells",
                self.data.values.len() - self.data.valid_count
            ));
            ui.label(format!("Valid: {}", self.data.valid_count));
            if self.scale.logarithmic {
                let count = self
                    .data
                    .values
                    .iter()
                    .filter(|&&v| v.is_finite() && v <= 0.0)
                    .count();
                let (swatch, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                ui.painter().rect_filled(swatch, 0.0, NON_POSITIVE);
                ui.label(format!(
                    "Zero / negative (outside log scale): {count} cells"
                ));
            }
        });
        for note in self.data_notes() {
            ui.small(note);
        }
        if !*inspect_cells {
            return;
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.strong("Inspect cell");
            ui.label(format!("{} row", self.data.y.name));
            ui.add(egui::DragValue::new(&mut self.cell[0]).range(0..=self.data.shape[0] - 1));
            ui.label(format!("{} column", self.data.x.name));
            ui.add(egui::DragValue::new(&mut self.cell[1]).range(0..=self.data.shape[1] - 1));
        });
        let value = self.data.values[self.cell[0] * self.data.shape[1] + self.cell[1]];
        let [longitude, latitude] = self.data.cell_center(self.cell[0], self.cell[1]);
        ui.label(format!(
            "{}: {} · {}: {} · Value: {} {}",
            self.data.y.label(),
            axis_value(&self.data.y, latitude),
            self.data.x.label(),
            axis_value(&self.data.x, longitude),
            number(value),
            if value.is_finite() {
                &self.data.units
            } else {
                ""
            }
        ));
    }

    fn paint(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        coastlines: bool,
    ) -> Result<(), String> {
        let projection = self.projector.as_ref().map_err(Clone::clone)?;
        // Scrolling can change the last bits of rect.size() through floating-point rounding.
        let size_changed = |old: Rect| (old.size() - rect.size()).length_sq() > 1e-6;
        if self
            .mesh
            .as_ref()
            .is_none_or(|(old_rect, scale, _)| size_changed(*old_rect) || *scale != self.scale)
        {
            let mesh = if self.data.kind == PlotKind::Map {
                map_mesh(&self.data, self.scale, projection, rect)
            } else {
                Some(section_mesh(&self.data, self.scale, rect))
            };
            self.mesh = Some((
                rect,
                self.scale,
                mesh.map(Shape::mesh)
                    .ok_or_else(|| "Could not draw this grid's cell boundaries.".into()),
            ));
        }
        let (origin, _, shape) = self.mesh.as_ref().unwrap();
        let mut shape = shape.as_ref().map_err(Clone::clone)?.clone();
        if origin.min != rect.min {
            shape.translate(rect.min - origin.min);
        }
        painter.add(shape);
        if self.data.kind == PlotKind::Map {
            draw_axes(painter, rect, projection);
            if coastlines {
                if self
                    .coastlines
                    .as_ref()
                    .is_none_or(|(old_rect, _)| size_changed(*old_rect))
                {
                    self.coastlines = Some((rect, coastline_shapes(projection, rect)));
                }
                let (origin, shapes) = self.coastlines.as_ref().unwrap();
                let offset = rect.min - origin.min;
                painter.extend(shapes.iter().cloned().map(|mut shape| {
                    if offset != Vec2::ZERO {
                        shape.translate(offset);
                    }
                    shape
                }));
            }
        } else {
            draw_section_axes(painter, rect, &self.data);
        }
        Ok(())
    }

    fn data_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if !self.data.missing_values.is_empty() {
            notes.push(format!(
                "Additional missing values: {} (before scaling{}).",
                self.data
                    .missing_values
                    .iter()
                    .map(|v| format!("{v:e}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if self.data.reference.is_some() {
                    ", both inputs"
                } else {
                    ""
                },
            ));
        }
        if let Some([lo, hi]) = self.data.range {
            notes.push(format!(
                "Slice range: {} to {} {}",
                number(lo),
                number(hi),
                self.data.units
            ));
        } else {
            notes.push("This slice has no valid values.".into());
        }
        let below = self
            .data
            .values
            .iter()
            .filter(|&&v| {
                v.is_finite() && (!self.scale.logarithmic || v > 0.0) && v < self.scale.limits[0]
            })
            .count();
        let above = self
            .data
            .values
            .iter()
            .filter(|&&v| v.is_finite() && v > self.scale.limits[1])
            .count();
        if below + above > 0 {
            notes.push(format!(
                "Colour limits clip {below} cells below Min and {above} above Max."
            ));
        }
        if self.data.cell_polygons.is_some() && self.data.x.inferred_bounds {
            notes.push("Curved grid: cell corners are approximate, inferred from neighbouring coordinates on the sphere.".into());
        } else if self.data.x.inferred_bounds || self.data.y.inferred_bounds {
            notes.push(
                "Cell edges inferred from coordinate midpoints where bounds are not provided."
                    .into(),
            );
        }
        if let Some(cells) = &self.data.cell_polygons {
            let count = cells.iter().filter(|c| c.is_empty()).count();
            if count > 0 {
                notes.push(format!("Repeated or collapsed grid cells are not drawn ({count}); their stored values remain inspectable."));
            }
        }
        notes
    }
}

fn plot_layout(available_width: f32, kind: PlotKind, projection: &Projector) -> (Vec2, Vec2, Vec2) {
    if kind == PlotKind::Map {
        let width = (available_width - 68.0).max(160.0);
        let height = width / projection.aspect_ratio();
        let (padding, top) = if projection.kind == Projection::Mollweide {
            (50.0, 20.0)
        } else {
            (80.0, 12.0)
        };
        (
            Vec2::new(width + 68.0, height + padding),
            Vec2::new(48.0, top),
            Vec2::new(width, height),
        )
    } else {
        let width = (available_width - 96.0).max(160.0);
        let height = (width * 0.6).clamp(180.0, 420.0);
        (
            Vec2::new(width + 96.0, height + 100.0),
            Vec2::new(76.0, 32.0),
            Vec2::new(width, height),
        )
    }
}

fn axis_value(axis: &Axis, value: f64) -> String {
    match axis.kind {
        AxisKind::Longitude => {
            let value = wrap_longitude(value);
            if value.abs() < 1e-8 {
                "0°".into()
            } else {
                format!(
                    "{}°{}",
                    number(value.abs()),
                    if value < 0.0 { "W" } else { "E" }
                )
            }
        }
        AxisKind::Latitude => {
            if value.abs() < 1e-8 {
                "0°".into()
            } else {
                format!(
                    "{}°{}",
                    number(value.abs()),
                    if value < 0.0 { "S" } else { "N" }
                )
            }
        }
        AxisKind::Vertical => number(value),
    }
}

fn section_position(data: &MapSlice, rect: Rect, x: f64, y: f64) -> Pos2 {
    let [xmin, xmax] = data.x.extent();
    let [ymin, ymax] = data.y.extent();
    let fraction = (y - ymin) / (ymax - ymin);
    Pos2::new(
        rect.left() + ((x - xmin) / (xmax - xmin)) as f32 * rect.width(),
        rect.top()
            + (if data.y.positive_down {
                fraction
            } else {
                1.0 - fraction
            }) as f32
                * rect.height(),
    )
}

fn section_cell_rect(data: &MapSlice, rect: Rect, y: usize, x: usize) -> Rect {
    Rect::from_two_pos(
        section_position(data, rect, data.x.bounds[x][0], data.y.bounds[y][0]),
        section_position(data, rect, data.x.bounds[x][1], data.y.bounds[y][1]),
    )
}

fn section_mesh(data: &MapSlice, scale: ColourScale, rect: Rect) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    mesh.add_colored_rect(rect, Color32::from_rgb(248, 249, 250));
    for y in 0..data.y.centers.len() {
        for x in 0..data.x.centers.len() {
            mesh.add_colored_rect(
                section_cell_rect(data, rect, y, x),
                scale.colour(data.values[y * data.x.centers.len() + x]),
            );
        }
    }
    mesh
}

fn draw_section_axes(painter: &egui::Painter, rect: Rect, data: &MapSlice) {
    let font = FontId::proportional(12.0);
    let [xmin, xmax] = data.x.extent();
    let [ymin, ymax] = data.y.extent();
    for i in 0..=4 {
        let fraction = i as f64 / 4.0;
        let x = xmin * (1.0 - fraction) + xmax * fraction;
        let y = ymin * (1.0 - fraction) + ymax * fraction;
        let position = section_position(data, rect, x, y);
        painter.line_segment(
            [
                Pos2::new(position.x, rect.top()),
                Pos2::new(position.x, rect.bottom()),
            ],
            Stroke::new(0.5, Color32::from_black_alpha(40)),
        );
        painter.line_segment(
            [
                Pos2::new(rect.left(), position.y),
                Pos2::new(rect.right(), position.y),
            ],
            Stroke::new(0.5, Color32::from_black_alpha(40)),
        );
        if rect.width() >= 300.0 || i % 2 == 0 {
            painter.text(
                Pos2::new(position.x, rect.bottom() + 7.0),
                Align2::CENTER_TOP,
                axis_value(&data.x, x),
                font.clone(),
                INK,
            );
        }
        painter.text(
            Pos2::new(rect.left() - 8.0, position.y),
            Align2::RIGHT_CENTER,
            number(y),
            font.clone(),
            INK,
        );
    }
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, INK), egui::StrokeKind::Inside);
    painter.text(
        rect.left_top() - Vec2::new(0.0, 24.0),
        Align2::LEFT_TOP,
        data.y.label(),
        font.clone(),
        INK,
    );
    painter.text(
        Pos2::new(rect.center().x, rect.bottom() + 30.0),
        Align2::CENTER_TOP,
        data.x.label(),
        font,
        INK,
    );
}

fn coastline_shapes(projection: &Projector, rect: Rect) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for line in crate::coastlines::lines() {
        let mut points = Vec::new();
        let mut previous: Option<f64> = None;
        for &[longitude, latitude] in line {
            if previous.is_some_and(|old| (longitude - old).abs() > 180.0) {
                if points.len() >= 2 {
                    shapes.push(Shape::line(
                        points.clone(),
                        Stroke::new(2.0, Color32::from_white_alpha(200)),
                    ));
                    shapes.push(Shape::line(
                        std::mem::take(&mut points),
                        Stroke::new(0.8, INK),
                    ));
                } else {
                    points.clear();
                }
            }
            if let Some(position) = map_position(projection, rect, longitude, latitude) {
                points.push(position);
            }
            previous = Some(longitude);
        }
        if points.len() >= 2 {
            shapes.push(Shape::line(
                points.clone(),
                Stroke::new(2.0, Color32::from_white_alpha(200)),
            ));
            shapes.push(Shape::line(points, Stroke::new(0.8, INK)));
        }
    }
    shapes
}

fn paragraph(painter: &egui::Painter, position: Pos2, text: &str, font: FontId, width: f32) -> f32 {
    let galley = painter.layout(text.to_owned(), font, INK, width);
    let height = galley.size().y;
    painter.galley(position, galley, INK);
    height + 5.0
}

fn figure_svg(
    data: MapSlice,
    scale: ColourScale,
    projection: Projection,
    coastlines: bool,
) -> Result<String, String> {
    scale.validate().map_err(str::to_owned)?;
    let mut view = MapView::new(data, projection, scale);
    let width = 1200.0;
    let mut height: f32 = 0.0;
    let mut error = None;
    let context = egui::Context::default();
    crate::typography::configure(&context);
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 4096.0))),
            ..Default::default()
        },
        |ui| {
            let painter = ui.painter();
            painter.rect_filled(
                Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 4096.0)),
                0.0,
                Color32::WHITE,
            );
            let mut y = 20.0;
            y += paragraph(
                painter,
                Pos2::new(24.0, y),
                &view.data.title,
                crate::typography::heading(20.0),
                width - 48.0,
            );
            let description = format!(
                "{} · {} · {} · no smoothing",
                view.data.kind.name(),
                scale.name(),
                scale.palette.name()
            );
            y += paragraph(
                painter,
                Pos2::new(24.0, y),
                &description,
                FontId::proportional(12.0),
                width - 48.0,
            );
            let selections = |selectors: &[Selector]| {
                selectors
                    .iter()
                    .map(|s| format!("{}: {}", s.name, s.label))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let current = format!(
                "{}{} · {}",
                if view.data.reference.is_some() {
                    "Current: "
                } else {
                    ""
                },
                view.data.source,
                selections(&view.data.selectors)
            );
            y += paragraph(
                painter,
                Pos2::new(24.0, y),
                &current,
                FontId::proportional(12.0),
                width - 48.0,
            );
            if let Some(reference) = &view.data.reference {
                y += paragraph(
                    painter,
                    Pos2::new(24.0, y),
                    &format!(
                        "Reference: {reference} · {} · plotted as current − reference",
                        selections(&view.data.reference_selectors)
                    ),
                    FontId::proportional(12.0),
                    width - 48.0,
                );
            }
            let projection = match &view.projector {
                Ok(p) => p,
                Err(message) => {
                    error = Some(message.clone());
                    return;
                }
            };
            let (outer, offset, size) = plot_layout(width - 48.0, view.data.kind, projection);
            let rect = Rect::from_min_size(Pos2::new(24.0, y + 8.0) + offset, size);
            if let Err(message) = view.paint(painter, rect, coastlines) {
                error = Some(message);
                return;
            }
            y += outer.y + 8.0;
            draw_colorbar(
                painter,
                Rect::from_min_size(Pos2::new(24.0 + offset.x, y), Vec2::new(size.x, 14.0)),
                scale,
                &view.data.units,
            );
            y += 65.0;
            let mut missing = format!(
                "Gray: missing / masked ({} cells).",
                view.data.values.len() - view.data.valid_count
            );
            if scale.logarithmic {
                let count = view
                    .data
                    .values
                    .iter()
                    .filter(|&&v| v.is_finite() && v <= 0.0)
                    .count();
                missing.push_str(&format!(
                    " Tan: zero / negative, outside Log₁₀ scale ({count} cells)."
                ));
            }
            y += paragraph(
                painter,
                Pos2::new(24.0, y),
                &missing,
                FontId::proportional(12.0),
                width - 48.0,
            );
            for note in view.data_notes() {
                y += paragraph(
                    painter,
                    Pos2::new(24.0, y),
                    &note,
                    FontId::proportional(12.0),
                    width - 48.0,
                );
            }
            if coastlines && view.data.kind == PlotKind::Map {
                y += paragraph(
                    painter,
                    Pos2::new(24.0, y),
                    "Present-day coastlines: Natural Earth, 1:110m.",
                    FontId::proportional(12.0),
                    width - 48.0,
                );
            }
            height = (y + 16.0).ceil();
        },
    );
    output.textures_delta.clear();
    if let Some(error) = error {
        return Err(error);
    }
    crate::export::svg(Vec2::new(width, height), &output.shapes)
}

fn map_position(projection: &Projector, rect: Rect, longitude: f64, latitude: f64) -> Option<Pos2> {
    let [x, y] = projection.forward(longitude, latitude)?;
    Some(Pos2::new(
        rect.left() + ((x + 1.0) / 2.0) as f32 * rect.width(),
        rect.top() + ((1.0 - y) / 2.0) as f32 * rect.height(),
    ))
}

fn cell_at(
    data: &MapSlice,
    projection: &Projector,
    rect: Rect,
    position: Pos2,
) -> Option<[usize; 2]> {
    if data.kind != PlotKind::Map {
        let [xmin, xmax] = data.x.extent();
        let [ymin, ymax] = data.y.extent();
        let xf = f64::from((position.x - rect.left()) / rect.width());
        let yf = f64::from((position.y - rect.top()) / rect.height());
        if !(0.0..=1.0).contains(&xf) || !(0.0..=1.0).contains(&yf) {
            return None;
        }
        let yf = if data.y.positive_down { yf } else { 1.0 - yf };
        let x = xmin * (1.0 - xf) + xmax * xf;
        let y = ymin * (1.0 - yf) + ymax * yf;
        return Some([
            data.y.bounds.iter().position(|b| y >= b[0] && y <= b[1])?,
            data.x.bounds.iter().position(|b| x >= b[0] && x <= b[1])?,
        ]);
    }
    let [longitude, latitude] = projection.inverse([
        f64::from((position.x - rect.left()) / rect.width()) * 2.0 - 1.0,
        1.0 - f64::from((position.y - rect.top()) / rect.height()) * 2.0,
    ])?;
    if let Some(cells) = &data.cell_polygons {
        let index = cells
            .iter()
            .enumerate()
            .filter(|(_, parts)| parts.iter().any(|p| contains(p, [longitude, latitude])))
            .min_by(|(a, _), (b, _)| {
                let distance = |i: usize| {
                    let dx =
                        wrap_longitude(data.x.centers[i] - longitude) * latitude.to_radians().cos();
                    dx * dx + (data.y.centers[i] - latitude).powi(2)
                };
                distance(*a).total_cmp(&distance(*b))
            })?
            .0;
        return Some([index / data.shape[1], index % data.shape[1]]);
    }
    let x = data.x.bounds.iter().position(|&bounds| {
        longitude_parts(bounds)
            .iter()
            .any(|[lo, hi]| longitude >= *lo && longitude <= *hi)
    })?;
    let y = data
        .y
        .bounds
        .iter()
        .position(|[lo, hi]| latitude >= *lo && latitude <= *hi)?;
    Some([y, x])
}

fn cell_rows(
    projection: &Projector,
    rect: Rect,
    longitude: [f64; 2],
    latitude: [f64; 2],
) -> Option<Vec<[Pos2; 2]>> {
    // shortcut: use 1° latitude strips for curved edges; refine by screen error when adding zoom.
    let steps = if projection.kind == Projection::Equirectangular {
        1
    } else {
        (latitude[1] - latitude[0]).ceil().max(1.0) as usize
    };
    (0..=steps)
        .map(|i| {
            let fraction = i as f64 / steps as f64;
            let latitude = latitude[0] * (1.0 - fraction) + latitude[1] * fraction;
            Some([
                map_position(projection, rect, longitude[0], latitude)?,
                map_position(projection, rect, longitude[1], latitude)?,
            ])
        })
        .collect()
}

fn cell_outline(
    projection: &Projector,
    rect: Rect,
    longitude: [f64; 2],
    latitude: [f64; 2],
) -> Option<Vec<Pos2>> {
    let rows = cell_rows(projection, rect, longitude, latitude)?;
    Some(
        rows.iter()
            .map(|row| row[0])
            .chain(rows.iter().rev().map(|row| row[1]))
            .collect(),
    )
}

fn add_cell(
    mesh: &mut egui::Mesh,
    projection: &Projector,
    rect: Rect,
    longitude: [f64; 2],
    latitude: [f64; 2],
    color: Color32,
) -> Option<()> {
    let rows = cell_rows(projection, rect, longitude, latitude)?;
    let start = u32::try_from(mesh.vertices.len()).ok()?;
    for row in &rows {
        for &position in row {
            mesh.colored_vertex(position, color);
        }
    }
    for i in 0..rows.len() - 1 {
        let a = start + i as u32 * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    Some(())
}

fn projected_polygon(
    projection: &Projector,
    rect: Rect,
    polygon: &GeoPolygon,
) -> Option<Vec<Pos2>> {
    let mut points = Vec::new();
    for (&a, &b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        let mut edges = vec![(a, b, 0)];
        while let Some((a, b, depth)) = edges.pop() {
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let pa = map_position(projection, rect, a[0], a[1])?;
            let pb = map_position(projection, rect, b[0], b[1])?;
            let pm = map_position(projection, rect, mid[0], mid[1])?;
            // Near a pole even a sub-degree meridian can bend sharply on screen.
            if depth < 16
                && ((a[0] - b[0]).abs().max((a[1] - b[1]).abs()) > 1.0
                    || pm.distance_sq(pa.lerp(pb, 0.5)) > 0.0001)
            {
                edges.push((mid, b, depth + 1));
                edges.push((a, mid, depth + 1));
            } else {
                points.push(pa);
            }
        }
    }
    // Polar projections collapse many boundary vertices to the same screen point.
    points.dedup();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    Some(points)
}

fn add_polygon(mesh: &mut egui::Mesh, points: &[Pos2], color: Color32) -> Option<()> {
    let start = u32::try_from(mesh.vertices.len()).ok()?;
    for &point in points {
        mesh.colored_vertex(point, color);
    }
    let cross = |a: Pos2, b: Pos2, c: Pos2| {
        f64::from(b.x - a.x) * f64::from(c.y - a.y) - f64::from(b.y - a.y) * f64::from(c.x - a.x)
    };
    let area: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(b.x) * f64::from(a.y))
        .sum();
    let direction = area.signum();
    let mut remaining: Vec<_> = (0..points.len()).collect();
    // Ear clipping also handles concave cells and curved projected edges.
    while remaining.len() > 3 {
        let mut removed = false;
        for i in 0..remaining.len() {
            let a = remaining[(i + remaining.len() - 1) % remaining.len()];
            let b = remaining[i];
            let c = remaining[(i + 1) % remaining.len()];
            let turn = direction * cross(points[a], points[b], points[c]);
            if turn.abs() < 1e-8 {
                remaining.remove(i);
                removed = true;
                break;
            }
            if turn < 0.0
                || remaining.iter().any(|&p| {
                    p != a
                        && p != b
                        && p != c
                        && direction * cross(points[a], points[b], points[p]) >= -1e-8
                        && direction * cross(points[b], points[c], points[p]) >= -1e-8
                        && direction * cross(points[c], points[a], points[p]) >= -1e-8
                })
            {
                continue;
            }
            mesh.add_triangle(start + a as u32, start + b as u32, start + c as u32);
            remaining.remove(i);
            removed = true;
            break;
        }
        if !removed {
            return None;
        }
    }
    if remaining.len() == 3 {
        mesh.add_triangle(
            start + remaining[0] as u32,
            start + remaining[1] as u32,
            start + remaining[2] as u32,
        );
    }
    Some(())
}

fn map_mesh(
    data: &MapSlice,
    scale: ColourScale,
    projection: &Projector,
    rect: Rect,
) -> Option<egui::Mesh> {
    let mut mesh = egui::Mesh::default();
    add_cell(
        &mut mesh,
        projection,
        rect,
        [-180.0, 180.0],
        [-90.0, 90.0],
        Color32::from_rgb(248, 249, 250),
    )?;
    if let Some(cells) = &data.cell_polygons {
        for (parts, &value) in cells.iter().zip(&data.values) {
            for polygon in parts {
                let points = projected_polygon(projection, rect, polygon)?;
                add_polygon(&mut mesh, &points, scale.colour(value))?;
            }
        }
        return Some(mesh);
    }
    let nx = data.x.centers.len();
    for (y, &latitude) in data.y.bounds.iter().enumerate() {
        for (x, &longitude) in data.x.bounds.iter().enumerate() {
            let value = data.values[y * nx + x];
            let color = scale.colour(value);
            for part in longitude_parts(longitude) {
                add_cell(&mut mesh, projection, rect, part, latitude, color)?;
            }
        }
    }
    Some(mesh)
}

fn draw_axes(painter: &egui::Painter, rect: Rect, projection: &Projector) {
    let font = FontId::proportional(12.0);
    let label_width = if projection.kind == Projection::Equirectangular {
        rect.width()
    } else {
        rect.width() * 0.5
    };
    let label_step = if label_width < 120.0 {
        180
    } else if label_width < 300.0 {
        120
    } else {
        60
    };
    for longitude in (-180..=180).step_by(60) {
        let points = (-90..=90)
            .filter_map(|latitude| {
                map_position(projection, rect, longitude as f64, latitude as f64)
            })
            .collect();
        painter.add(Shape::line(
            points,
            Stroke::new(0.5, Color32::from_black_alpha(45)),
        ));
        if projection.kind == Projection::Mollweide || longitude % label_step != 0 {
            continue;
        }
        let label = if longitude < 0 {
            format!("{}°W", -longitude)
        } else if longitude > 0 {
            format!("{longitude}°E")
        } else {
            "0°".into()
        };
        if let Some(position) = map_position(projection, rect, longitude as f64, -90.0) {
            let tick = Pos2::new(position.x, rect.bottom() + 16.0);
            painter.line_segment(
                [position, tick],
                Stroke::new(0.5, Color32::from_black_alpha(90)),
            );
            painter.text(
                tick + Vec2::new(0.0, 4.0),
                Align2::CENTER_TOP,
                label,
                font.clone(),
                INK,
            );
        }
    }
    for latitude in (-90..=90).step_by(30) {
        let (Some(a), Some(b)) = (
            map_position(projection, rect, -180.0, latitude as f64),
            map_position(projection, rect, 180.0, latitude as f64),
        ) else {
            continue;
        };
        painter.line_segment([a, b], Stroke::new(0.5, Color32::from_black_alpha(45)));
        let label = if latitude < 0 {
            format!("{}°S", -latitude)
        } else if latitude > 0 {
            format!("{latitude}°N")
        } else {
            "0°".into()
        };
        let (position, anchor) = if projection.kind == Projection::Mollweide && latitude == 90 {
            (a - Vec2::new(0.0, 6.0), Align2::CENTER_BOTTOM)
        } else if projection.kind == Projection::Mollweide && latitude == -90 {
            (a + Vec2::new(0.0, 6.0), Align2::CENTER_TOP)
        } else {
            (a - Vec2::new(8.0, 0.0), Align2::RIGHT_CENTER)
        };
        painter.text(position, anchor, label, font.clone(), INK);
    }
    if let Some(outline) = cell_outline(projection, rect, [-180.0, 180.0], [-90.0, 90.0]) {
        painter.add(Shape::closed_line(outline, Stroke::new(1.0, INK)));
    }
    if projection.kind != Projection::Mollweide {
        painter.text(
            Pos2::new(rect.center().x, rect.bottom() + 42.0),
            Align2::CENTER_TOP,
            "Longitude",
            font,
            INK,
        );
    }
}

fn draw_colorbar(painter: &egui::Painter, rect: Rect, scale: ColourScale, units: &str) {
    let mut mesh = egui::Mesh::default();
    for i in 0..128 {
        let x = rect.left() + rect.width() * i as f32 / 128.0;
        mesh.add_colored_rect(
            Rect::from_min_max(
                Pos2::new(x, rect.top()),
                Pos2::new(x + rect.width() / 128.0, rect.bottom()),
            ),
            scale.colour(scale.value_at(i as f64 / 127.0)),
        );
    }
    painter.add(Shape::mesh(mesh));
    for i in 0..=4 {
        if scale.limits[0] == scale.limits[1] && i != 2 {
            continue;
        }
        let fraction = i as f64 / 4.0;
        painter.text(
            Pos2::new(
                rect.left() + rect.width() * fraction as f32,
                rect.bottom() + 5.0,
            ),
            Align2::CENTER_TOP,
            number(scale.value_at(fraction)),
            FontId::proportional(12.0),
            INK,
        );
    }
    painter.text(
        Pos2::new(rect.center().x, rect.bottom() + 25.0),
        Align2::CENTER_TOP,
        if units.is_empty() {
            "Units not specified"
        } else {
            units
        },
        FontId::proportional(12.0),
        INK,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_reuses_geometry_and_keeps_cells_and_coastlines_aligned() {
        use std::sync::Arc;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for (file, variable, indices, kind) in [
            (
                "temperature.nc",
                "/temperature",
                vec![0, 0, 0],
                PlotKind::Map,
            ),
            ("curvilinear.nc", "/field", vec![0, 1, 0, 0], PlotKind::Map),
            (
                "packed.nc",
                "/packed",
                vec![0, 1, 0, 0],
                PlotKind::LatitudeDepth,
            ),
        ] {
            let data = MapSlice::load(
                &root.join("samples").join(file),
                variable,
                &indices,
                kind,
                &[],
            )
            .unwrap();
            let scale = ColourScale {
                limits: data.range.unwrap(),
                palette: Palette::Viridis,
                logarithmic: false,
            };
            let mut view = MapView::new(data, Projection::EqualEarth, scale);
            let context = egui::Context::default();
            crate::typography::configure(&context);
            let frame = |view: &mut MapView, rect: Rect| {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(1100.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ui| view.paint(ui.painter(), rect, true).unwrap(),
                );
                output.textures_delta.clear();
                output.shapes
            };
            let rect = Rect::from_min_size(Pos2::new(72.0, 230.3), Vec2::new(720.0, 350.7));
            frame(&mut view, rect);
            let Shape::Mesh(original) = view.mesh.as_ref().unwrap().2.as_ref().unwrap().clone()
            else {
                panic!("expected grid mesh");
            };
            for offset in [
                Vec2::new(0.0, -7.25),
                Vec2::new(0.0, -267.8),
                Vec2::new(3.1, 24.7),
                Vec2::ZERO,
            ] {
                let moved = rect.translate(offset);
                let shapes = frame(&mut view, moved);
                let Shape::Mesh(cached) = view.mesh.as_ref().unwrap().2.as_ref().unwrap() else {
                    panic!("expected cached mesh");
                };
                assert!(
                    Arc::ptr_eq(&original, cached),
                    "{file}: scrolling rebuilt the grid"
                );
                let drawn = shapes
                    .iter()
                    .find_map(|s| {
                        if let Shape::Mesh(m) = &s.shape {
                            Some(m)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                assert_eq!(drawn.indices, original.indices);
                for (actual, stored) in drawn.vertices.iter().zip(&original.vertices) {
                    assert!(actual.pos.distance(stored.pos + offset) < 0.001);
                    assert_eq!(actual.color, stored.color);
                }
                let [x, y] = view.data.cell_center(1, 1);
                let projection = view.projector.as_ref().unwrap();
                let point = if kind == PlotKind::Map {
                    map_position(projection, moved, wrap_longitude(x), y).unwrap()
                } else {
                    section_position(&view.data, moved, x, y)
                };
                assert_eq!(cell_at(&view.data, projection, moved, point), Some([1, 1]));
                if let Some((coast_rect, coastlines)) = &view.coastlines {
                    assert_eq!(*coast_rect, rect);
                    let Shape::Path(first) = &coastlines[0] else {
                        panic!("expected coastline");
                    };
                    assert!(shapes.iter().any(|s| matches!(&s.shape, Shape::Path(p) if
                        p.points.len() == first.points.len()
                        && p.points.iter().zip(&first.points).all(|(a,b)| a.distance(*b + offset) < 0.001)
                    )));
                }
            }
            view.scale.palette = Palette::Plasma;
            frame(&mut view, rect);
            let Shape::Mesh(recoloured) = view.mesh.as_ref().unwrap().2.as_ref().unwrap().clone()
            else {
                panic!("expected recoloured mesh");
            };
            assert!(!Arc::ptr_eq(&original, &recoloured));
            frame(
                &mut view,
                Rect::from_min_size(rect.min, rect.size() + Vec2::splat(20.0)),
            );
            let Shape::Mesh(resized) = view.mesh.as_ref().unwrap().2.as_ref().unwrap() else {
                panic!("expected resized mesh");
            };
            assert!(!Arc::ptr_eq(&recoloured, resized));
        }
    }

    #[test]
    fn missing_value_input_accepts_lists_and_rejects_invalid_entries() {
        assert_eq!(
            parse_missing_values(" -1e35, -9999  0 ").unwrap(),
            [-1e35, -9999.0, 0.0]
        );
        assert!(parse_missing_values(" , ").unwrap().is_empty());
        for input in ["NaN", "inf", "1e999", "-9999, typo"] {
            assert!(parse_missing_values(input).is_err(), "{input}");
        }
    }

    #[test]
    fn applying_missing_values_updates_comparisons_colour_limits_and_exports() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/curvilinear.nc");
        let dataset = Dataset::load(&source).unwrap();
        let variable = dataset
            .variables
            .iter()
            .position(|v| v.path == "/bounded")
            .unwrap();
        let data = MapSlice::load(&source, "/bounded", &[1, 0, 0], PlotKind::Map, &[]).unwrap();
        let mut panel = MapPanel {
            key: Some((source.clone(), "/bounded".into())),
            indices: vec![1, 0, 0],
            missing_text: "0, 7".into(),
            view: Some(MapView::new(
                data,
                Projection::EqualEarth,
                ColourScale {
                    limits: [-1e35, 11.0],
                    palette: Palette::Viridis,
                    logarithmic: false,
                },
            )),
            reference: Some(Reference {
                dataset: Dataset::load(&source).unwrap(),
                variable,
                indices: vec![0, 0, 0],
            }),
            ..MapPanel::default()
        };
        let context = egui::Context::default();
        crate::typography::configure(&context);
        let frame = |panel: &mut MapPanel, events: Vec<egui::Event>| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 1400.0))),
                    events,
                    ..Default::default()
                },
                |ui| panel.show(ui, &source, &dataset.variables[variable]),
            );
            output.textures_delta.clear();
            output
        };
        let output = frame(&mut panel, vec![]);
        let apply = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                Shape::Text(t) if t.galley.job.text == "Apply" => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        for expected_count in [4, 6] {
            for pressed in [true, false] {
                frame(
                    &mut panel,
                    vec![
                        egui::Event::PointerMoved(apply),
                        egui::Event::PointerButton {
                            pos: apply,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            if panel.loading.is_some() {
                assert!(panel.view.is_none());
                assert!(!panel.can_export());
                let data = panel
                    .loading
                    .take()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                let (sender, receiver) = mpsc::channel();
                sender.send(data).unwrap();
                panel.loading = Some(receiver);
                frame(&mut panel, vec![]);
            }
            let view = panel.view.as_ref().unwrap();
            assert_eq!(view.data.valid_count, expected_count);
            assert_eq!(view.data.range, Some([6.0, 6.0]));
            if expected_count == 4 {
                // The first cell is missing in the reference; the second in the current slice.
                assert!(view.data.values[0].is_nan() && view.data.values[1].is_nan());
            }
            assert_eq!(view.scale.limits, [-6.0, 6.0]);
            let svg =
                figure_svg(view.data.clone(), view.scale, Projection::EqualEarth, false).unwrap();
            assert_eq!(
                svg.contains("Additional missing values"),
                expected_count == 4
            );
            if expected_count == 4 {
                assert!(svg.contains("both inputs"));
                assert!(svg.contains("(2 cells)"));
            }
            assert!(panel.can_export());
            panel.missing_text.clear();
        }
    }

    #[test]
    #[ignore = "Set NCDIVE_NEMO_SNAPSHOTS to check undeclared NEMO fill values"]
    fn nemo_missing_values_restore_flux_ranges_and_export() {
        let root = PathBuf::from(
            std::env::var_os("NCDIVE_NEMO_SNAPSHOTS").expect("Set NCDIVE_NEMO_SNAPSHOTS"),
        );
        let source = root.join("ORCA2_1m_20000101_20001231_diad_T.nc");
        let missing = [-1e35, -9999.0];
        let flux = MapSlice::load(&source, "/Cflx", &[0, 0, 0], PlotKind::Map, &missing).unwrap();
        assert_eq!(flux.valid_count, 16638);
        assert_eq!(
            flux.range,
            Some([-1.319323928328231e-6, 4.0621185348754807e-7])
        );
        assert_eq!(flux.values[80 * 182 + 80], -3.266955772573965e-8);
        assert!(flux.values[100 * 182 + 100].is_nan());
        for (name, indices) in [
            ("/vsink", vec![0, 0, 0, 0]),
            ("/Oflx", vec![0, 0, 0]),
            ("/dpCO2", vec![0, 0, 0]),
        ] {
            let data = MapSlice::load(&source, name, &indices, PlotKind::Map, &missing).unwrap();
            assert_eq!(data.valid_count, 16638);
            assert!(data.range.unwrap()[0] > -9999.0);
        }
        let chlorophyll = MapSlice::load(
            &source,
            "/TChl",
            &[0, 0, 0, 0],
            PlotKind::Map,
            &[-1e35, -9999.0, 0.0],
        )
        .unwrap();
        assert_eq!(chlorophyll.valid_count, 16433);
        assert!(chlorophyll.range.unwrap()[0] > 0.0);
        let mut difference = flux.clone();
        difference.subtract(&flux).unwrap();
        assert_eq!(difference.range, Some([0.0, 0.0]));
        assert_eq!(difference.valid_count, flux.valid_count);
        let mut scale = ColourScale {
            limits: [0.0, 1.0],
            palette: Palette::BlueRed,
            logarithmic: false,
        };
        scale.auto_range(&flux.values, false);
        assert_eq!(Some(scale.limits), flux.range);
        let destination = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/export-previews");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(
            destination.join("nemo-masked-flux.values"),
            flux.values
                .iter()
                .map(|v| format!("{v}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let svg = figure_svg(flux, scale, Projection::EqualEarth, true).unwrap();
        assert!(svg.contains("Additional missing values"));
        for extension in ["png", "pdf"] {
            let bytes = crate::export::encode(&svg, extension).unwrap();
            crate::export::save(
                &destination.join(format!("nemo-masked-flux.{extension}")),
                &bytes,
            )
            .unwrap();
        }
    }

    #[test]
    fn inspection_is_opt_in_and_still_reports_the_pointed_cell() {
        assert!(!MapPanel::default().inspect_cells);
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/curvilinear.nc");
        let data = MapSlice::load(&source, "/field", &[0, 1, 0, 0], PlotKind::Map, &[]).unwrap();
        let scale = ColourScale {
            limits: data.range.unwrap(),
            palette: Palette::Viridis,
            logarithmic: false,
        };
        let mut view = MapView::new(data, Projection::EqualEarth, scale);
        let context = egui::Context::default();
        crate::typography::configure(&context);
        let frame = |view: &mut MapView, position: Pos2, mut inspect: bool| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 1400.0))),
                    events: vec![egui::Event::PointerMoved(position)],
                    ..Default::default()
                },
                |ui| view.show(ui, false, &mut inspect),
            );
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, Shape::Text(t) if t.galley.job.text.contains("Value:")))
        };
        assert!(!frame(&mut view, Pos2::ZERO, false));
        let rect = view.mesh.as_ref().unwrap().0;
        let [lon, lat] = view.data.cell_center(1, 2);
        let position = map_position(view.projector.as_ref().unwrap(), rect, lon, lat).unwrap();
        assert!(!frame(&mut view, position, false));
        assert_eq!(view.cell, [0, 0]);
        assert!(frame(&mut view, position + Vec2::splat(0.1), true));
        assert_eq!(view.cell, [1, 2]);
        assert!(!frame(&mut view, Pos2::ZERO, false));
        assert_eq!(view.cell, [1, 2]);
    }

    #[test]
    #[ignore = "Set NCDIVE_NEMO_SNAPSHOTS to measure local NEMO interaction timings"]
    fn nemo_interaction_timings() {
        use std::{hint::black_box, time::Instant};
        let root = PathBuf::from(
            std::env::var_os("NCDIVE_NEMO_SNAPSHOTS").expect("Set NCDIVE_NEMO_SNAPSHOTS"),
        );
        let source = root.join("ORCA2_1m_20000101_20001231_diad_T.nc");
        let start = Instant::now();
        let data = MapSlice::load(&source, "/TChl", &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
        println!(
            "slice load: {:.2} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let projector = Projector::new(Projection::EqualEarth).unwrap();
        let rect = Rect::from_min_size(
            Pos2::new(72.0, 200.0),
            Vec2::new(720.0, 720.0 / projector.aspect_ratio()),
        );
        let scale = ColourScale {
            limits: data.range.unwrap(),
            palette: Palette::Viridis,
            logarithmic: false,
        };
        let start = Instant::now();
        let mesh = map_mesh(&data, scale, &projector, rect).unwrap();
        println!(
            "mesh build: {:.2} ms; {} vertices",
            start.elapsed().as_secs_f64() * 1000.0,
            mesh.vertices.len()
        );
        let positions: Vec<_> = (0..180)
            .map(|i| {
                let lon = -179.0 + i as f64 * 2.0;
                map_position(&projector, rect, lon, 50.0 * (lon.to_radians() * 2.0).sin()).unwrap()
            })
            .collect();
        let start = Instant::now();
        for &position in &positions {
            black_box(cell_at(&data, &projector, rect, position));
        }
        println!(
            "hover lookup: {:.3} ms/move",
            start.elapsed().as_secs_f64() * 1000.0 / positions.len() as f64
        );
        let context = egui::Context::default();
        crate::typography::configure(&context);
        let mut view = MapView::new(data, Projection::EqualEarth, scale);
        let frame = |view: &mut MapView, position: Pos2, mut inspect: bool| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 1400.0))),
                    events: vec![egui::Event::PointerMoved(position)],
                    ..Default::default()
                },
                |ui| view.show(ui, true, &mut inspect),
            );
            output.textures_delta.clear();
            black_box(context.tessellate(output.shapes, output.pixels_per_point));
        };
        for inspect in [false, true] {
            frame(&mut view, positions[0], inspect);
            let start = Instant::now();
            for &position in positions.iter().take(60) {
                frame(&mut view, position, inspect);
            }
            println!(
                "hover UI + tessellation (inspector {inspect}): {:.3} ms/frame",
                start.elapsed().as_secs_f64() * 1000.0 / 60.0
            );
        }
        let scroll_frame = |view: &mut MapView, offset: f32| {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 600.0))),
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("scroll-timing")
                        .vertical_scroll_offset(offset)
                        .show(ui, |ui| view.show(ui, true, &mut false));
                },
            );
            output.textures_delta.clear();
            black_box(context.tessellate(output.shapes, output.pixels_per_point));
        };
        scroll_frame(&mut view, 0.0);
        let start = Instant::now();
        for i in (1..=12).chain((0..12).rev()) {
            scroll_frame(&mut view, i as f32 * 7.25);
        }
        println!(
            "scroll UI + tessellation: {:.3} ms/frame",
            start.elapsed().as_secs_f64() * 1000.0 / 24.0
        );
    }
    #[test]
    fn folded_and_polar_cells_tessellate_at_screen_and_export_sizes() {
        for corners in [
            [
                [-140.20479941185798, 89.51180660730992],
                [-85.5482265351784, 89.50198105048116],
                [-20.413217793729274, 89.76228787054808],
                [152.05363410276797, 89.78384364578595],
            ],
            [
                [40.490509816032244, 22.400883798071423],
                [41.00002463101461, 22.40086889229819],
                [33.961398929087096, 25.858919191154083],
                [40.490535736083984, 24.226118667398914],
            ],
            [
                [173.13373585810467, 79.85694280904752],
                [178.01741196866257, 79.85827499755833],
                [178.26212355105812, 80.83144849312403],
                [172.9108151966406, 80.83629200868715],
            ],
            [[-135.0, 80.0], [-45.0, 80.0], [45.0, 80.0], [135.0, 80.0]],
        ] {
            for kind in Projection::ALL {
                let projection = Projector::new(kind).unwrap();
                for width in [360.0, 720.0, 1084.0] {
                    let rect = Rect::from_min_size(
                        Pos2::new(72.0, 100.0),
                        Vec2::new(width, width / projection.aspect_ratio()),
                    );
                    let mut mesh = egui::Mesh::default();
                    for p in crate::grid::cell_polygons(corners).unwrap() {
                        let points = projected_polygon(&projection, rect, &p).unwrap();
                        assert!(
                            add_polygon(&mut mesh, &points, Color32::RED).is_some(),
                            "{kind:?} {width}: {corners:?}"
                        );
                    }
                    assert!(mesh.is_valid() && !mesh.is_empty(), "{kind:?} {width}");
                }
            }
        }
    }

    #[test]
    fn curved_grids_draw_and_inspect_cells_in_every_projection() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/curvilinear.nc");
        for (name, indices) in [("/field", vec![0, 1, 0, 0]), ("/bounded", vec![0, 0, 0])] {
            let data = MapSlice::load(&path, name, &indices, PlotKind::Map, &[]).unwrap();
            for kind in Projection::ALL {
                let projection = Projector::new(kind).unwrap();
                let rect = Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(720.0, 720.0 / projection.aspect_ratio()),
                );
                let mesh = map_mesh(
                    &data,
                    ColourScale {
                        limits: data.range.unwrap(),
                        palette: Palette::Viridis,
                        logarithmic: false,
                    },
                    &projection,
                    rect,
                )
                .unwrap();
                assert!(mesh.is_valid() && !mesh.is_empty());
                assert!(
                    mesh.vertices
                        .iter()
                        .all(|p| p.pos.is_finite() && rect.expand(0.001).contains(p.pos))
                );
                for row in 0..data.shape[0] {
                    for col in 0..data.shape[1] {
                        let [lon, lat] = data.cell_center(row, col);
                        let position = map_position(&projection, rect, lon, lat).unwrap();
                        assert_eq!(
                            cell_at(&data, &projection, rect, position),
                            Some([row, col]),
                            "{name}, {kind:?}"
                        );
                    }
                }
                assert_eq!(
                    cell_at(
                        &data,
                        &projection,
                        rect,
                        map_position(&projection, rect, 0.0, 0.0).unwrap()
                    ),
                    None
                );
                for triangle in mesh.triangles() {
                    let vertices = triangle.map(|i| mesh.vertices[i as usize]);
                    if vertices[0].color == Color32::from_rgb(248, 249, 250) {
                        continue;
                    }
                    let min = vertices
                        .iter()
                        .map(|v| v.pos.x)
                        .fold(f32::INFINITY, f32::min);
                    let max = vertices
                        .iter()
                        .map(|v| v.pos.x)
                        .fold(f32::NEG_INFINITY, f32::max);
                    assert!(
                        max - min < rect.width() / 4.0,
                        "date-line triangle spans the map"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "Set NCDIVE_NEMO_SNAPSHOTS to the local snapshot directory"]
    fn nemo_snapshots_draw_export_and_preserve_values() {
        let root = PathBuf::from(
            std::env::var_os("NCDIVE_NEMO_SNAPSHOTS").expect("Set NCDIVE_NEMO_SNAPSHOTS"),
        );
        let source = root.join("ORCA2_1m_20000101_20001231_diad_T.nc");
        let data = MapSlice::load(&source, "/TChl", &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(data.shape, [149, 182]);
        assert_eq!(data.valid_count, 27118);
        assert_eq!(data.values[80 * 182 + 80], 8.15558607314415e-8);
        let deep = MapSlice::load(&source, "/TChl", &[11, 20, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(deep.values[80 * 182 + 80], 1.4890209898421602e-10);
        let surface = MapSlice::load(&source, "/PPINT", &[0, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(surface.values[80 * 182 + 80], 2.972570598558377e-7);
        for entry in std::fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.ends_with(".nc") || name.contains("1x1") {
                continue;
            }
            let variable = if name.contains("diad_T") {
                "/TChl"
            } else if name.contains("limphy") {
                "/lim2mmfe_dia"
            } else {
                "/DIC"
            };
            let map = MapSlice::load(&path, variable, &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
            assert_eq!(map.shape, [149, 182]);
            let mut difference = map.clone();
            difference.subtract(&map).unwrap();
            assert_eq!(difference.range, Some([0.0, 0.0]));
        }
        let destination = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/export-previews");
        std::fs::create_dir_all(&destination).unwrap();
        for (name, values) in [
            ("nemo-surface", &data.values),
            ("nemo-deep", &deep.values),
            ("nemo-integrated", &surface.values),
        ] {
            std::fs::write(
                destination.join(format!("{name}.values")),
                values.iter().map(|v| format!("{v}\n")).collect::<String>(),
            )
            .unwrap();
        }
        for row in 1..148 {
            assert!(data.cell_polygons.as_ref().unwrap()[row * 182].is_empty());
            assert!(data.cell_polygons.as_ref().unwrap()[row * 182 + 181].is_empty());
        }
        let scale = ColourScale {
            limits: [1e-8, 6e-6],
            palette: Palette::Viridis,
            logarithmic: true,
        };
        for (i, kind) in Projection::ALL.into_iter().enumerate() {
            let projection = Projector::new(kind).unwrap();
            let rect = Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(720.0, 720.0 / projection.aspect_ratio()),
            );
            let mesh = map_mesh(&data, scale, &projection, rect).unwrap();
            assert!(
                mesh.is_valid()
                    && mesh
                        .vertices
                        .iter()
                        .all(|v| v.pos.is_finite() && rect.expand(0.001).contains(v.pos))
            );
            for cell in [[80, 80], [120, 80], [140, 70], [146, 70], [110, 40]] {
                let [lon, lat] = data.cell_center(cell[0], cell[1]);
                let p = map_position(&projection, rect, wrap_longitude(lon), lat).unwrap();
                let found = cell_at(&data, &projection, rect, p).unwrap();
                assert_eq!(
                    data.cell_center(found[0], found[1]),
                    [lon, lat],
                    "{kind:?} {cell:?}"
                );
            }
            let svg = figure_svg(data.clone(), scale, kind, true).unwrap();
            assert!(svg.contains("approximate"));
            for extension in ["png", "pdf"] {
                let bytes = crate::export::encode(&svg, extension).unwrap();
                crate::export::save(&destination.join(format!("nemo-{i}.{extension}")), &bytes)
                    .unwrap();
            }
        }
    }
    #[test]
    fn map_positions_and_colour_scale_keep_orientation_and_constant_values() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(360.0, 180.0));
        let projection = Projector::new(Projection::Equirectangular).unwrap();
        assert_eq!(
            map_position(&projection, rect, -180.0, 90.0),
            Some(rect.left_top())
        );
        assert_eq!(
            map_position(&projection, rect, 180.0, -90.0),
            Some(rect.right_bottom())
        );
        assert_eq!(
            ColourScale {
                limits: [7.0, 7.0],
                palette: Palette::Viridis,
                logarithmic: false
            }
            .fraction(7.0),
            0.5
        );
        assert_eq!(
            ColourScale {
                limits: [0.0, 10.0],
                palette: Palette::Viridis,
                logarithmic: false
            }
            .fraction(-10.0),
            0.0
        );
        assert_eq!(Palette::Viridis.colour(0.0), Color32::from_rgb(68, 1, 84));
        assert_eq!(
            Palette::Viridis.colour(1.0),
            Color32::from_rgb(253, 231, 37)
        );
    }

    #[test]
    fn longitude_labels_hide_in_mollweide_and_stay_below_other_maps() {
        for kind in Projection::ALL {
            let projection = Projector::new(kind).unwrap();
            for width in [160.0, 360.0, 480.0, 720.0] {
                let rect = Rect::from_min_size(
                    Pos2::new(64.0, 32.0),
                    Vec2::new(width, width / projection.aspect_ratio()),
                );
                let context = egui::Context::default();
                crate::typography::configure(&context);
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1000.0))),
                        ..Default::default()
                    },
                    |ui| draw_axes(ui.painter(), rect, &projection),
                );
                output.textures_delta.clear();
                let labels: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        Shape::Text(text)
                            if text.galley.job.text.ends_with(['E', 'W'])
                                || (text.galley.job.text == "0°"
                                    && text.pos.y > rect.center().y) =>
                        {
                            Some(text.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .collect();
                let has_title = output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, Shape::Text(text) if text.galley.job.text == "Longitude")
                });
                assert_eq!(has_title, kind != Projection::Mollweide);
                if kind == Projection::Mollweide {
                    assert!(labels.is_empty(), "width {width}");
                    assert_eq!(
                        output
                            .shapes
                            .iter()
                            .filter(|shape| {
                                matches!(&shape.shape, Shape::Path(path) if !path.closed)
                            })
                            .count(),
                        7,
                        "Mollweide must keep its longitude grid lines"
                    );
                    assert!(
                        output.shapes.iter().all(|shape| match &shape.shape {
                            Shape::LineSegment { points, .. } =>
                                points.iter().all(|p| p.y <= rect.bottom() + 0.001),
                            _ => true,
                        }),
                        "Mollweide must not draw label guides below the map"
                    );
                    continue;
                }
                assert!(labels.len() >= 3, "{kind:?}, width {width}");
                if width >= 720.0 || (kind == Projection::Equirectangular && width >= 360.0) {
                    assert_eq!(labels.len(), 7, "{kind:?}, width {width}");
                }
                for label in &labels {
                    assert!(label.top() > rect.bottom(), "{kind:?}, width {width}");
                    assert!(label.bottom() < rect.bottom() + 42.0);
                    assert!(label.left() >= rect.left() - 48.0);
                    assert!(label.right() <= rect.right() + 20.0);
                }
                for pair in labels.windows(2) {
                    assert!(
                        pair[0].right() + 4.0 <= pair[1].left(),
                        "{kind:?}, width {width}: labels overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn export_finishes_after_leaving_the_plot_and_reports_save_errors() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/temperature.nc");
        let data = MapSlice::load(&source, "/temperature", &[0, 0, 0], PlotKind::Map, &[]).unwrap();
        let panel = MapPanel {
            view: Some(MapView::new(
                data,
                Projection::EqualEarth,
                ColourScale {
                    limits: [4.0, 27.0],
                    palette: Palette::Viridis,
                    logarithmic: false,
                },
            )),
            ..MapPanel::default()
        };
        let directory =
            std::env::temp_dir().join(format!("ncdive-menu-export-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("figure.png");
        let success = panel.export(path.clone(), "png").unwrap();
        let failure = panel
            .export(directory.join("missing/figure.png"), "png")
            .unwrap();
        drop(panel);
        assert_eq!(
            success
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .unwrap(),
            path
        );
        let image = resvg::tiny_skia::Pixmap::decode_png(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(image.width(), 1200);
        assert!(
            failure
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .unwrap_err()
                .contains("Could not save")
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn every_projection_draws_the_grid_and_inspects_the_original_cells() {
        assert_eq!(MapPanel::default().projection, Projection::EqualEarth);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/packed.nc");
        let data = MapSlice::load(&path, "/packed", &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
        for kind in Projection::ALL {
            let projection = Projector::new(kind).unwrap();
            let rect = Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(720.0, 720.0 / projection.aspect_ratio()),
            );
            let mesh = map_mesh(
                &data,
                ColourScale {
                    limits: data.range.unwrap(),
                    palette: Palette::Viridis,
                    logarithmic: false,
                },
                &projection,
                rect,
            )
            .unwrap();
            assert!(mesh.is_valid() && !mesh.is_empty());
            assert!(
                mesh.vertices
                    .iter()
                    .all(|v| v.pos.is_finite() && rect.expand(0.001).contains(v.pos))
            );
            assert!(mesh.vertices.iter().any(|v| v.color == MISSING));
            for (y, &latitude) in data.y.centers.iter().enumerate() {
                for (x, &longitude) in data.x.centers.iter().enumerate() {
                    let position =
                        map_position(&projection, rect, wrap_longitude(longitude), latitude)
                            .unwrap();
                    assert_eq!(
                        cell_at(&data, &projection, rect, position),
                        Some([y, x]),
                        "{kind:?}"
                    );
                }
            }
            if kind != Projection::Equirectangular {
                assert_eq!(cell_at(&data, &projection, rect, rect.left_top()), None);
            }
        }
    }

    #[test]
    fn sections_keep_physical_orientation_and_inspect_original_cells() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/packed.nc");
        let projection = Projector::new(Projection::EqualEarth).unwrap();
        let rect = Rect::from_min_size(Pos2::new(80.0, 32.0), Vec2::new(640.0, 320.0));
        for kind in [PlotKind::LatitudeDepth, PlotKind::LongitudeDepth] {
            let mut data = MapSlice::load(&path, "/packed", &[0, 1, 0, 0], kind, &[]).unwrap();
            for down in [true, false] {
                data.y.positive_down = down;
                let shallow = section_position(&data, rect, data.x.centers[0], data.y.centers[0]);
                let deep = section_position(&data, rect, data.x.centers[0], data.y.centers[1]);
                assert_eq!(shallow.y < deep.y, down);
                for (y, &vertical) in data.y.centers.iter().enumerate() {
                    for (x, &horizontal) in data.x.centers.iter().enumerate() {
                        assert_eq!(
                            cell_at(
                                &data,
                                &projection,
                                rect,
                                section_position(&data, rect, horizontal, vertical)
                            ),
                            Some([y, x])
                        );
                    }
                }
                let scale = ColourScale {
                    limits: data.range.unwrap(),
                    palette: Palette::Plasma,
                    logarithmic: false,
                };
                let mesh = section_mesh(&data, scale, rect);
                assert!(mesh.is_valid());
                assert!(
                    mesh.vertices
                        .iter()
                        .all(|v| rect.expand(0.001).contains(v.pos))
                );
            }
        }
    }

    #[test]
    fn coastlines_do_not_connect_across_the_date_line() {
        for kind in Projection::ALL {
            let projection = Projector::new(kind).unwrap();
            let rect = Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(720.0, 720.0 / projection.aspect_ratio()),
            );
            let shapes = coastline_shapes(&projection, rect);
            assert!(shapes.len() >= 200);
            for shape in shapes {
                let Shape::Path(path) = shape else {
                    panic!("coastlines must be line paths");
                };
                assert!(
                    path.points
                        .iter()
                        .all(|p| p.is_finite() && rect.expand(0.01).contains(*p))
                );
                assert!(
                    path.points
                        .windows(2)
                        .all(|p| (p[1].x - p[0].x).abs() < rect.width() / 2.0),
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn exports_maps_sections_and_differences_as_png_and_pdf() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let destination = root.join("target/export-previews");
        std::fs::create_dir_all(&destination).unwrap();
        let map = MapSlice::load(
            &root.join("samples/temperature.nc"),
            "/temperature",
            &[0, 0, 0],
            PlotKind::Map,
            &[],
        )
        .unwrap();
        let curved = MapSlice::load(
            &root.join("samples/curvilinear.nc"),
            "/field",
            &[0, 1, 0, 0],
            PlotKind::Map,
            &[],
        )
        .unwrap();
        let section = MapSlice::load(
            &root.join("samples/packed.nc"),
            "/packed",
            &[0, 1, 0, 0],
            PlotKind::LatitudeDepth,
            &[],
        )
        .unwrap();
        let mut difference = MapSlice::load(
            &root.join("samples/temperature.nc"),
            "/temperature",
            &[1, 0, 0],
            PlotKind::Map,
            &[],
        )
        .unwrap();
        difference.subtract(&map).unwrap();
        for (name, data, palette, logarithmic, projection, coasts) in [
            (
                "curved",
                curved,
                Palette::Viridis,
                false,
                Projection::Robinson,
                true,
            ),
            (
                "map",
                map,
                Palette::Cividis,
                false,
                Projection::EqualEarth,
                true,
            ),
            (
                "section",
                section,
                Palette::Plasma,
                true,
                Projection::Equirectangular,
                false,
            ),
            (
                "difference",
                difference,
                Palette::BlueRed,
                false,
                Projection::Mollweide,
                false,
            ),
        ] {
            let mut scale = ColourScale {
                limits: [0.0, 1.0],
                palette,
                logarithmic,
            };
            scale.auto_range(&data.values, data.reference.is_some());
            let svg = figure_svg(data, scale, projection, coasts).unwrap();
            assert!(svg.contains("no smoothing"));
            if name == "curved" {
                assert!(svg.contains("approximate"));
            }
            if name == "difference" {
                assert!(svg.contains("current − reference"));
            }
            if name == "section" {
                assert!(svg.contains("depth (m)"));
                assert!(svg.contains("Log₁₀"));
            }
            for extension in ["png", "pdf"] {
                let bytes = crate::export::encode(&svg, extension).unwrap();
                if extension == "png" {
                    let image = resvg::tiny_skia::Pixmap::decode_png(&bytes).unwrap();
                    assert_eq!(image.width(), 1200);
                    assert!(image.height() > 600 && image.height() < 1200);
                    assert!(image.pixels().iter().all(|p| p.alpha() == 255));
                    assert!(
                        image
                            .pixels()
                            .iter()
                            .filter(|p| p.red().abs_diff(p.blue()) > 30)
                            .count()
                            > 1000
                    );
                } else {
                    assert!(bytes.starts_with(b"%PDF-"));
                }
                crate::export::save(&destination.join(format!("{name}.{extension}")), &bytes)
                    .unwrap();
            }
        }
    }

    #[test]
    fn projected_dateline_cells_stay_on_each_side_of_the_map() {
        for kind in Projection::ALL {
            let projection = Projector::new(kind).unwrap();
            let rect = Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(360.0, 360.0 / projection.aspect_ratio()),
            );
            let mut mesh = egui::Mesh::default();
            for part in longitude_parts([170.0, 190.0]) {
                add_cell(
                    &mut mesh,
                    &projection,
                    rect,
                    part,
                    [-10.0, 10.0],
                    Color32::RED,
                )
                .unwrap();
            }
            for [a, b, c] in mesh.triangles() {
                let points = [a, b, c].map(|i| mesh.vertices[i as usize].pos);
                assert!(
                    points.iter().all(|p| p.x < 30.0) || points.iter().all(|p| p.x > 330.0),
                    "{kind:?}"
                );
            }
        }
    }
}
