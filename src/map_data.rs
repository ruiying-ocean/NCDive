use std::path::Path;

use crate::grid::{GeoPolygon, cell_polygons, infer_corners, omit_duplicates};

use netcdf::{
    AttributeValue, Variable,
    types::{FloatType, IntType, NcVariableType},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisKind {
    Longitude,
    Latitude,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlotKind {
    #[default]
    Map,
    LatitudeDepth,
    LongitudeDepth,
}

impl PlotKind {
    pub const ALL: [Self; 3] = [Self::Map, Self::LatitudeDepth, Self::LongitudeDepth];

    pub fn name(self) -> &'static str {
        match self {
            Self::Map => "Map",
            Self::LatitudeDepth => "Latitude–depth section",
            Self::LongitudeDepth => "Longitude–depth section",
        }
    }

    fn axes(self) -> [AxisKind; 2] {
        match self {
            Self::Map => [AxisKind::Longitude, AxisKind::Latitude],
            Self::LatitudeDepth => [AxisKind::Latitude, AxisKind::Vertical],
            Self::LongitudeDepth => [AxisKind::Longitude, AxisKind::Vertical],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Axis {
    pub name: String,
    pub units: String,
    pub kind: AxisKind,
    pub positive_down: bool,
    /// Flattened in row-major order for two-dimensional map coordinates.
    pub centers: Vec<f64>,
    pub bounds: Vec<[f64; 2]>,
    pub inferred_bounds: bool,
}

#[derive(Clone, Debug)]
pub struct Selector {
    pub dimension: usize,
    pub name: String,
    pub length: usize,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct MapSlice {
    pub x: Axis,
    pub y: Axis,
    pub kind: PlotKind,
    pub shape: [usize; 2],
    pub cell_polygons: Option<Vec<Vec<GeoPolygon>>>,
    pub title: String,
    pub units: String,
    pub source: String,
    pub reference: Option<String>,
    pub reference_selectors: Vec<Selector>,
    /// Y rows, X columns, in the coordinate variables' order.
    pub values: Vec<f64>,
    pub selectors: Vec<Selector>,
    pub range: Option<[f64; 2]>,
    pub valid_count: usize,
    pub missing_values: Vec<f64>,
}

impl MapSlice {
    pub fn suggested_kind(path: &Path, name: &str, preferred: PlotKind) -> Result<PlotKind> {
        let file = netcdf::open(path)?;
        let variable = file
            .variable(name.trim_start_matches('/'))
            .ok_or("This variable is no longer in the file.")?;
        for kind in
            std::iter::once(preferred).chain(PlotKind::ALL.into_iter().filter(|&k| k != preferred))
        {
            if let [Some(x), Some(y)] = find_axes(&file, name, &variable, kind)?
                && x.0 != y.0
            {
                return Ok(kind);
            }
        }
        Ok(preferred)
    }

    pub fn load(
        path: &Path,
        name: &str,
        indices: &[usize],
        kind: PlotKind,
        missing_values: &[f64],
    ) -> Result<Self> {
        let file = netcdf::open(path)?;
        let variable = file
            .variable(name.trim_start_matches('/'))
            .ok_or("This variable is no longer in the file.")?;
        let dimensions = variable.dimensions();
        if dimensions.len() != indices.len() || dimensions.iter().any(|d| d.len() == 0) {
            return Err("This variable has no data to plot.".into());
        }
        let wanted = kind.axes();
        let axes = find_axes(&file, name, &variable, kind)?;
        let [x_coordinate, y_coordinate] = axes;
        let (x_dimension, x_path) = x_coordinate
            .ok_or("This view needs longitude and latitude coordinates. Sections require one-dimensional coordinates.")?;
        let (y_dimension, y_path) = y_coordinate.ok_or(if kind == PlotKind::Map {
            "Map view needs matching one- or two-dimensional longitude and latitude coordinates."
        } else {
            "This variable has no one-dimensional depth, height or pressure coordinate for a vertical section."
        })?;
        if x_dimension == y_dimension {
            return Err(
                "These coordinates describe points rather than a two-dimensional grid.".into(),
            );
        }
        let shape = [dimensions[y_dimension].len(), dimensions[x_dimension].len()];
        let mut x = read_axis(&file, &x_path, wanted[0])?;
        let mut y = read_axis(&file, &y_path, wanted[1])?;
        let cell_polygons = if file
            .variable(x_path.trim_start_matches('/'))
            .unwrap()
            .dimensions()
            .len()
            == 2
        {
            let (polygons, inferred) =
                read_cell_polygons(&file, [&x_path, &y_path], [&x, &y], shape)?;
            x.inferred_bounds = inferred;
            y.inferred_bounds = inferred;
            Some(polygons)
        } else {
            None
        };
        let mut start = indices.to_vec();
        let mut count = vec![1; dimensions.len()];
        let mut selectors = Vec::new();
        for (index, dimension) in dimensions.iter().enumerate() {
            if index == y_dimension || index == x_dimension {
                start[index] = 0;
                count[index] = dimension.len();
            } else {
                if start[index] >= dimension.len() {
                    return Err("The selected slice is outside this variable.".into());
                }
                let mut label = format!("Index {} of {}", start[index] + 1, dimension.len());
                if let Some((_, coordinate)) = relative_variable(&file, name, &dimension.name())
                    && coordinate.dimensions().len() == 1
                    && coordinate.len() == dimension.len()
                {
                    let values = decoded(&coordinate, &[start[index]], &[1], &[])?;
                    let units = text(&coordinate, "units")?.unwrap_or_default();
                    label = format!("{} {}", number(values[0]), units);
                    if let Some(calendar) = text(&coordinate, "calendar")? {
                        label.push_str(&format!(" · {calendar} calendar"));
                    }
                }
                selectors.push(Selector {
                    dimension: index,
                    name: dimension.name(),
                    length: dimension.len(),
                    label,
                });
            }
        }
        let raw = decoded(&variable, &start, &count, missing_values)?;
        let [ny, nx] = shape;
        let values = if y_dimension < x_dimension {
            raw
        } else {
            (0..ny)
                .flat_map(|y| (0..nx).map(move |x| (x, y)))
                .map(|(x, y)| raw[x * ny + y])
                .collect()
        };
        let mut slice = Self {
            x,
            y,
            kind,
            shape,
            cell_polygons,
            title: text(&variable, "long_name")?.unwrap_or_else(|| name.to_owned()),
            units: text(&variable, "units")?.unwrap_or_default(),
            source: format!("{} · {name}", path.display()),
            reference: None,
            reference_selectors: Vec::new(),
            values,
            selectors,
            range: None,
            valid_count: 0,
            missing_values: missing_values.to_vec(),
        };
        slice.update_range();
        Ok(slice)
    }

    pub fn subtract(&mut self, reference: &Self) -> Result<()> {
        if self.units.trim().is_empty() || self.units.trim() != reference.units.trim() {
            return Err("Difference plots need matching, specified units. Unit conversion is not supported.".into());
        }
        // shortcut: compare grids in their stored order; add explicit regridding before accepting different grids.
        if self.kind != reference.kind
            || !self.x.matches(&reference.x)
            || !self.y.matches(&reference.y)
            || self.shape != reference.shape
            || !polygons_match(&self.cell_polygons, &reference.cell_polygons)
        {
            return Err("The coordinate values or cell boundaries differ. Difference plots require matching grids in the same order.".into());
        }
        for (value, other) in self.values.iter_mut().zip(&reference.values) {
            *value = if value.is_finite() && other.is_finite() {
                *value - other
            } else {
                f64::NAN
            };
            if !value.is_finite() {
                *value = f64::NAN;
            }
        }
        self.title = format!("{} − {}", self.title, reference.title);
        self.reference = Some(reference.source.clone());
        self.reference_selectors = reference.selectors.clone();
        self.update_range();
        Ok(())
    }

    pub fn cell_center(&self, row: usize, column: usize) -> [f64; 2] {
        if self.cell_polygons.is_some() {
            let index = row * self.shape[1] + column;
            [self.x.centers[index], self.y.centers[index]]
        } else {
            [self.x.centers[column], self.y.centers[row]]
        }
    }

    fn update_range(&mut self) {
        self.range = None;
        self.valid_count = 0;
        for &value in &self.values {
            if value.is_finite() {
                self.valid_count += 1;
                self.range = Some(
                    self.range
                        .map_or([value, value], |[lo, hi]| [lo.min(value), hi.max(value)]),
                );
            }
        }
    }
}

impl Axis {
    pub fn extent(&self) -> [f64; 2] {
        self.bounds
            .iter()
            .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], b| {
                [lo.min(b[0]), hi.max(b[1])]
            })
    }

    pub fn label(&self) -> String {
        match self.kind {
            AxisKind::Longitude => "Longitude".into(),
            AxisKind::Latitude => "Latitude".into(),
            AxisKind::Vertical => format!("{} ({})", self.name, self.units),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0);
        let coordinate_matches = |a: f64, b: f64| {
            if self.kind == AxisKind::Longitude {
                close(wrap_longitude(a - b), 0.0)
            } else {
                close(a, b)
            }
        };
        self.kind == other.kind
            && self.positive_down == other.positive_down
            && (self.kind != AxisKind::Vertical || self.units.trim() == other.units.trim())
            && self.centers.len() == other.centers.len()
            && self.bounds.len() == other.bounds.len()
            && self
                .centers
                .iter()
                .zip(&other.centers)
                .all(|(&a, &b)| coordinate_matches(a, b))
            && self.bounds.iter().zip(&other.bounds).all(|(a, b)| {
                close(a[1] - a[0], b[1] - b[0])
                    && coordinate_matches(a[0], b[0])
                    && coordinate_matches(a[1], b[1])
            })
    }
}

fn find_axes(
    file: &netcdf::File,
    name: &str,
    variable: &Variable<'_>,
    kind: PlotKind,
) -> Result<[Option<(usize, String)>; 2]> {
    let wanted = kind.axes();
    let mut axes = [None, None];
    let auxiliary = text(variable, "coordinates")?.unwrap_or_default();
    if kind == PlotKind::Map {
        let names: Vec<_> = auxiliary.split_whitespace().collect();
        let candidates: Vec<_> = if names.is_empty() {
            vec!["nav_lon", "nav_lat", "lon", "lat", "longitude", "latitude"]
        } else {
            names
        };
        let mut curved: [Option<(String, Vec<usize>)>; 2] = [None, None];
        for candidate in candidates {
            let Some((path, coordinate)) = relative_variable(file, name, candidate) else {
                continue;
            };
            if coordinate.dimensions().len() != 2 {
                continue;
            }
            let Some(axis) =
                coordinate_kind(&coordinate)?.and_then(|k| wanted.iter().position(|&w| w == k))
            else {
                continue;
            };
            let positions: Option<Vec<_>> = coordinate
                .dimensions()
                .iter()
                .map(|d| {
                    variable
                        .dimensions()
                        .iter()
                        .position(|v| v.name() == d.name() && v.len() == d.len())
                })
                .collect();
            if let Some(positions) = positions {
                if curved[axis].is_some() {
                    return Err(
                        "More than one two-dimensional coordinate describes this grid.".into(),
                    );
                }
                curved[axis] = Some((path, positions));
            }
        }
        if curved.iter().any(Option::is_some) {
            let [Some((x, xd)), Some((y, yd))] = curved else {
                return Err(
                    "Curved grids need both two-dimensional longitude and latitude coordinates."
                        .into(),
                );
            };
            if xd != yd || xd[0] == xd[1] {
                return Err("Two-dimensional longitude and latitude must use the same grid dimensions in the same order.".into());
            }
            return Ok([Some((xd[1], x)), Some((xd[0], y))]);
        }
    }
    for (index, dimension) in variable.dimensions().iter().enumerate() {
        let names = std::iter::once(dimension.name())
            .chain(auxiliary.split_whitespace().map(str::to_owned));
        for coordinate_name in names {
            let Some((coordinate_path, coordinate)) =
                relative_variable(file, name, &coordinate_name)
            else {
                continue;
            };
            if coordinate.dimensions().len() != 1
                || coordinate.dimensions()[0].name() != dimension.name()
                || coordinate.len() != dimension.len()
            {
                continue;
            }
            if let Some(axis_kind) = coordinate_kind(&coordinate)? {
                for (axis, wanted) in axes.iter_mut().zip(wanted) {
                    if axis.is_none() && axis_kind == wanted {
                        *axis = Some((index, coordinate_path.clone()));
                    }
                }
            }
        }
    }
    Ok(axes)
}

fn coordinate_kind(variable: &Variable<'_>) -> Result<Option<AxisKind>> {
    if let Some(longitude) = geographic_axis(variable)? {
        return Ok(Some(if longitude {
            AxisKind::Longitude
        } else {
            AxisKind::Latitude
        }));
    }
    let name = variable.name().to_lowercase();
    let standard = text(variable, "standard_name")?
        .unwrap_or_default()
        .to_lowercase();
    let axis = text(variable, "axis")?.unwrap_or_default();
    if axis.eq_ignore_ascii_case("Z")
        || text(variable, "positive")?.is_some()
        || matches!(
            standard.as_str(),
            "depth" | "height" | "altitude" | "air_pressure" | "sea_water_pressure"
        )
        || matches!(
            name.as_str(),
            "depth" | "zt" | "z" | "lev" | "level" | "pressure"
        )
    {
        return Ok(Some(AxisKind::Vertical));
    }
    Ok(None)
}

fn geographic_axis(variable: &Variable<'_>) -> Result<Option<bool>> {
    let standard = text(variable, "standard_name")?
        .unwrap_or_default()
        .to_lowercase();
    let name = variable.name().to_lowercase();
    let units = text(variable, "units")?.unwrap_or_default().to_lowercase();
    if units.contains("radian") || standard.starts_with("grid_") {
        return Ok(None);
    }
    if matches!(
        units.as_str(),
        "degrees_east" | "degree_east" | "degrees_e" | "degree_e"
    ) {
        return Ok(Some(true));
    }
    if matches!(
        units.as_str(),
        "degrees_north" | "degree_north" | "degrees_n" | "degree_n"
    ) {
        return Ok(Some(false));
    }
    if !units.is_empty() && units != "degrees" && units != "degree" {
        return Ok(None);
    }
    if standard == "longitude" || name == "lon" || name == "longitude" {
        Ok(Some(true))
    } else if standard == "latitude" || name == "lat" || name == "latitude" {
        Ok(Some(false))
    } else {
        Ok(None)
    }
}

fn relative_variable<'a>(
    file: &'a netcdf::File,
    scope: &str,
    name: &str,
) -> Option<(String, Variable<'a>)> {
    if name.starts_with('/') {
        return file
            .variable(name.trim_start_matches('/'))
            .map(|v| (name.to_owned(), v));
    }
    let mut parent = scope.rsplit_once('/').map_or("", |(parent, _)| parent);
    loop {
        let path = format!("{parent}/{name}");
        if let Some(variable) = file.variable(path.trim_start_matches('/')) {
            return Some((path, variable));
        }
        if parent.is_empty() {
            return None;
        }
        parent = parent.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

fn read_axis(file: &netcdf::File, path: &str, kind: AxisKind) -> Result<Axis> {
    let variable = file
        .variable(path.trim_start_matches('/'))
        .ok_or("Missing coordinate variable.")?;
    let longitude = kind == AxisKind::Longitude;
    let units = text(&variable, "units")?.unwrap_or_default();
    if kind == AxisKind::Vertical
        && (text(&variable, "formula_terms")?.is_some()
            || matches!(units.trim(), "" | "1" | "sigma_level" | "level"))
    {
        return Err("Vertical sections need physical depth, height or pressure coordinates. Dimensionless and terrain-following coordinates are not supported.".into());
    }
    if variable.dimensions().len() == 2 && kind != AxisKind::Vertical {
        let shape: Vec<_> = variable.dimensions().iter().map(|d| d.len()).collect();
        let centers = decoded(&variable, &[0, 0], &shape, &[])?;
        if centers
            .iter()
            .any(|v| !v.is_finite() || (kind == AxisKind::Latitude && v.abs() > 90.0))
        {
            return Err(format!("{path} contains missing or invalid coordinates.").into());
        }
        return Ok(Axis {
            name: variable.name(),
            units,
            kind,
            positive_down: false,
            centers,
            bounds: Vec::new(),
            inferred_bounds: false,
        });
    }
    let mut centers = decoded(&variable, &[0], &[variable.len()], &[])?;
    if centers
        .iter()
        .any(|v| !v.is_finite() || (kind == AxisKind::Latitude && v.abs() > 90.0))
    {
        return Err(format!("{path} contains missing or invalid coordinates.").into());
    }
    if longitude {
        for i in 1..centers.len() {
            centers[i] = centers[i - 1] + wrap_longitude(centers[i] - centers[i - 1]);
        }
        if centers.len() > 1 && (centers[centers.len() - 1] - centers[0]).abs() >= 360.0 - 1e-8 {
            return Err("The longitude axis repeats or spans more than one world.".into());
        }
    }
    if centers.len() > 1 {
        let direction = (centers[1] - centers[0]).signum();
        if direction == 0.0 || centers.windows(2).any(|v| (v[1] - v[0]) * direction <= 0.0) {
            return Err(format!("{path} must have increasing or decreasing coordinates.").into());
        }
    }
    let mut inferred_bounds = false;
    let mut cgenie_edges = false;
    let mut bounds = if let Some(reference) = text(&variable, "bounds")? {
        let (_, bound_variable) = relative_variable(file, path, &reference)
            .ok_or("The coordinate bounds variable is missing.")?;
        let shape: Vec<_> = bound_variable
            .dimensions()
            .iter()
            .map(|d| d.len())
            .collect();
        if shape != [centers.len(), 2] {
            return Err("Coordinate bounds must have shape (coordinate, 2).".into());
        }
        decoded(&bound_variable, &[0, 0], &shape, &[])?
            .as_chunks::<2>()
            .0
            .to_vec()
    } else if let Some(reference) = text(&variable, "edges")? {
        cgenie_edges = true;
        let (_, edges) = relative_variable(file, path, &reference)
            .ok_or("The coordinate edges variable is missing.")?;
        if edges.dimensions().len() != 1 || edges.len() != centers.len() + 1 {
            return Err(
                "Coordinate edges must contain one more value than the coordinate axis.".into(),
            );
        }
        decoded(&edges, &[0], &[edges.len()], &[])?
            .windows(2)
            .map(|v| [v[0], v[1]])
            .collect()
    } else {
        inferred_bounds = true;
        if centers.len() < 2 {
            return Err("A single-cell axis needs explicit coordinate bounds.".into());
        }
        let mut edges = vec![centers[0] - (centers[1] - centers[0]) / 2.0];
        edges.extend(centers.windows(2).map(|v| (v[0] + v[1]) / 2.0));
        let last = centers.len() - 1;
        edges.push(centers[last] + (centers[last] - centers[last - 1]) / 2.0);
        edges.windows(2).map(|v| [v[0], v[1]]).collect()
    };
    for (index, bounds) in bounds.iter_mut().enumerate() {
        if bounds.iter().any(|v| !v.is_finite()) {
            return Err("Coordinate bounds contain missing values.".into());
        }
        if longitude && (bounds[1] - bounds[0]).abs() < 360.0 - 1e-8 {
            for edge in bounds.iter_mut() {
                *edge = centers[index] + wrap_longitude(*edge - centers[index]);
            }
        }
        bounds.sort_by(f64::total_cmp);
        if kind == AxisKind::Latitude {
            // cGENIE extrapolates outer edges past samples located exactly at the poles.
            if inferred_bounds || (cgenie_edges && centers[index].abs() == 90.0) {
                for edge in bounds.iter_mut() {
                    *edge = edge.clamp(-90.0, 90.0);
                }
            }
            if bounds[0] < -90.0 || bounds[1] > 90.0 {
                return Err("Latitude bounds lie outside −90° to 90°.".into());
            }
        }
        if bounds[1] <= bounds[0]
            || centers[index] < bounds[0] - 1e-8
            || centers[index] > bounds[1] + 1e-8
            || (longitude && bounds[1] - bounds[0] > 360.0 + 1e-8)
        {
            return Err("Coordinate bounds do not enclose their grid cells.".into());
        }
    }
    let positive_down = if kind == AxisKind::Vertical {
        match text(&variable, "positive")?.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("down") => true,
            Some(value) if value.eq_ignore_ascii_case("up") => false,
            Some(_) => return Err("The vertical positive attribute must be up or down.".into()),
            None => {
                matches!(
                    units.to_lowercase().as_str(),
                    "pa" | "hpa" | "kpa" | "bar" | "dbar" | "mbar" | "millibar" | "millibars"
                ) || ((matches!(variable.name().to_lowercase().as_str(), "depth" | "zt")
                    || text(&variable, "standard_name")?.as_deref() == Some("depth"))
                    && centers.iter().all(|&v| v >= 0.0))
            }
        }
    } else {
        false
    };
    Ok(Axis {
        name: text(&variable, "long_name")?.unwrap_or_else(|| variable.name()),
        units,
        kind,
        positive_down,
        centers,
        bounds,
        inferred_bounds,
    })
}

fn read_cell_polygons(
    file: &netcdf::File,
    paths: [&str; 2],
    axes: [&Axis; 2],
    shape: [usize; 2],
) -> Result<(Vec<Vec<GeoPolygon>>, bool)> {
    let mut bounds = [None, None];
    for (axis, path) in paths.iter().enumerate() {
        let coordinate = file.variable(path.trim_start_matches('/')).unwrap();
        if let Some(reference) = text(&coordinate, "bounds")? {
            let (_, variable) = relative_variable(file, path, &reference)
                .ok_or("The coordinate bounds variable is missing.")?;
            let dims = variable.dimensions();
            if dims.len() != 3
                || dims[2].len() != 4
                || !dims[..2]
                    .iter()
                    .zip(coordinate.dimensions())
                    .all(|(a, b)| a.name() == b.name() && a.len() == b.len())
            {
                return Err("Curved-grid bounds must have shape (row, column, 4) on the coordinate's dimensions.".into());
            }
            let values = decoded(&variable, &[0, 0, 0], &[shape[0], shape[1], 4], &[])?;
            if values
                .iter()
                .any(|v| !v.is_finite() || (axis == 1 && v.abs() > 90.0))
            {
                return Err("Curved-grid bounds contain missing or invalid coordinates.".into());
            }
            bounds[axis] = Some(values);
        }
    }
    let inferred = bounds.iter().all(Option::is_none);
    let corners = match bounds {
        [Some(lon), Some(lat)] => lon.as_chunks::<4>().0.iter().zip(lat.as_chunks::<4>().0)
            .map(|(x, y)| std::array::from_fn(|i| [x[i], y[i]])).collect(),
        [None, None] => infer_corners(&axes[0].centers, &axes[1].centers, shape)
            .ok_or("Inferring curved-grid corners needs at least two rows and columns and distinct neighbouring coordinates.")?,
        _ => return Err("Curved grids must declare bounds for both longitude and latitude, or neither.".into()),
    };
    let mut polygons = corners
        .into_iter()
        .map(cell_polygons)
        .collect::<Option<Vec<_>>>()
        .ok_or("Could not form geographic cells from these grid boundaries.")?;
    if inferred {
        omit_duplicates(&mut polygons, &axes[0].centers, &axes[1].centers, shape);
    } else if polygons.iter().enumerate().any(|(i, parts)| {
        !parts.iter().any(|p| {
            crate::grid::contains(p, [wrap_longitude(axes[0].centers[i]), axes[1].centers[i]])
        })
    }) {
        return Err("Curved-grid bounds do not enclose their coordinate centres.".into());
    }
    Ok((polygons, inferred))
}

fn polygons_match(a: &Option<Vec<Vec<GeoPolygon>>>, b: &Option<Vec<Vec<GeoPolygon>>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter().zip(b).all(|(a, b)| {
                    a.len() == b.len()
                        && a.iter().zip(b).all(|(a, b)| {
                            a.len() == b.len()
                                && a.iter().zip(b).all(|(a, b)| {
                                    (a[0] - b[0]).abs() <= 1e-8 && (a[1] - b[1]).abs() <= 1e-8
                                })
                        })
                })
        }
        _ => false,
    }
}

fn decoded(
    variable: &Variable<'_>,
    start: &[usize],
    count: &[usize],
    extra_missing: &[f64],
) -> Result<Vec<f64>> {
    if text(variable, "_Unsigned")?.is_some_and(|s| s.eq_ignore_ascii_case("true")) {
        return Err("Variables using _Unsigned packing are not supported yet.".into());
    }
    let default_fill = match variable.vartype() {
        // Read declared fills as attributes: a mismatched NaN type can make the C fill query return zero.
        NcVariableType::Float(_)
        | NcVariableType::Int(
            IntType::I8 | IntType::I16 | IntType::I32 | IntType::U8 | IntType::U16 | IntType::U32,
        ) if variable.attribute("_FillValue").is_some() => None,
        NcVariableType::Float(FloatType::F32) => variable.fill_value::<f32>()?.map(f64::from),
        NcVariableType::Float(FloatType::F64) => variable.fill_value::<f64>()?,
        NcVariableType::Int(IntType::I8) => variable.fill_value::<i8>()?.map(f64::from),
        NcVariableType::Int(IntType::I16) => variable.fill_value::<i16>()?.map(f64::from),
        NcVariableType::Int(IntType::I32) => variable.fill_value::<i32>()?.map(f64::from),
        NcVariableType::Int(IntType::U8) => variable.fill_value::<u8>()?.map(f64::from),
        NcVariableType::Int(IntType::U16) => variable.fill_value::<u16>()?.map(f64::from),
        NcVariableType::Int(IntType::U32) => variable.fill_value::<u32>()?.map(f64::from),
        // shortcut: plot integers up to 32 bits; add typed masking when supporting 64-bit fields.
        _ => return Err("Map view supports floating point and up to 32-bit integer data.".into()),
    };
    let mut missing = numbers(variable, "missing_value")?;
    missing.extend(numbers(variable, "_FillValue")?);
    missing.extend(default_fill);
    if extra_missing.iter().any(|v| !v.is_finite()) {
        return Err("Additional missing values must be finite numbers.".into());
    }
    // Match the stored float precision: -1e35 is not exactly representable as f32.
    missing.extend(extra_missing.iter().map(|&value| {
        if variable.vartype() == NcVariableType::Float(FloatType::F32) {
            let stored = f64::from(value as f32);
            // A nonzero marker that underflows must not mask genuine zeros.
            if stored == 0.0 && value != 0.0 {
                value
            } else {
                stored
            }
        } else {
            value
        }
    }));
    let valid = numbers(variable, "valid_range")?;
    let (minimum, maximum) = match valid.as_slice() {
        [] => (
            scalar(variable, "valid_min", f64::NEG_INFINITY)?,
            scalar(variable, "valid_max", f64::INFINITY)?,
        ),
        [lo, hi] => (*lo, *hi),
        _ => return Err("valid_range must contain exactly two values.".into()),
    };
    let scale = scalar(variable, "scale_factor", 1.0)?;
    let offset = scalar(variable, "add_offset", 0.0)?;
    if !scale.is_finite()
        || !offset.is_finite()
        || minimum.is_nan()
        || maximum.is_nan()
        || minimum > maximum
    {
        return Err("This variable has invalid scaling or range metadata.".into());
    }
    let mut values = variable.get_values::<f64, _>((start, count))?;
    for value in &mut values {
        // CF missing and validity markers refer to packed values, before scaling.
        if !value.is_finite() || missing.contains(value) || *value < minimum || *value > maximum {
            *value = f64::NAN;
        } else {
            *value = *value * scale + offset;
            if !value.is_finite() {
                *value = f64::NAN;
            }
        }
    }
    Ok(values)
}

fn text(variable: &Variable<'_>, name: &str) -> Result<Option<String>> {
    variable
        .attribute_value(name)
        .map(|value| String::try_from(value?).map_err(Into::into))
        .transpose()
}

fn scalar(variable: &Variable<'_>, name: &str, default: f64) -> Result<f64> {
    match numbers(variable, name)?.as_slice() {
        [] => Ok(default),
        [value] => Ok(*value),
        _ => Err(format!("{name} must be a single number.").into()),
    }
}

fn numbers(variable: &Variable<'_>, name: &str) -> Result<Vec<f64>> {
    let Some(value) = variable.attribute_value(name) else {
        return Ok(Vec::new());
    };
    Ok(match value? {
        AttributeValue::Doubles(v) => v,
        AttributeValue::Floats(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Ints(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Uints(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Shorts(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Ushorts(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Schars(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Uchars(v) => v.into_iter().map(f64::from).collect(),
        AttributeValue::Longlongs(v) => v.into_iter().map(|v| v as f64).collect(),
        AttributeValue::Ulonglongs(v) => v.into_iter().map(|v| v as f64).collect(),
        value => vec![f64::try_from(value)?],
    })
}

pub fn wrap_longitude(value: f64) -> f64 {
    (value + 180.0).rem_euclid(360.0) - 180.0
}

pub fn longitude_parts([lo, hi]: [f64; 2]) -> Vec<[f64; 2]> {
    let width = hi - lo;
    if width >= 360.0 - 1e-8 {
        return vec![[-180.0, 180.0]];
    }
    let start = wrap_longitude(lo);
    let end = start + width;
    if end <= 180.0 {
        vec![[start, end]]
    } else {
        vec![[start, 180.0], [-180.0, end - 360.0]]
    }
}

pub fn number(value: f64) -> String {
    if !value.is_finite() {
        "Missing".to_owned()
    } else if value != 0.0 && !(0.001..1_000_000.0).contains(&value.abs()) {
        format!("{value:.4e}")
    } else {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(name)
    }

    #[test]
    fn curved_grids_keep_packing_slices_and_coordinate_order() {
        let path = sample("curvilinear.nc");
        let map = MapSlice::load(&path, "/field", &[0, 1, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(map.shape, [2, 3]);
        assert_eq!(&map.values[..3], &[12.0, 16.0, 20.0]);
        assert!(map.values[3].is_nan());
        assert_eq!(&map.values[4..], &[16.5, 20.5]);
        assert_eq!(map.valid_count, 5);
        assert_eq!(map.cell_center(1, 2), [-168.0, 14.0]);
        assert_eq!(
            map.selectors
                .iter()
                .map(|s| s.dimension)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(map.selectors[0].label.contains("30 days"));
        assert!(map.selectors[1].label.contains("5 m"));
        assert!(map.x.inferred_bounds);
        assert_eq!(
            MapSlice::suggested_kind(&path, "/field", PlotKind::LatitudeDepth).unwrap(),
            PlotKind::Map
        );
        assert!(
            MapSlice::load(&path, "/field", &[0, 0, 0, 0], PlotKind::LatitudeDepth, &[]).is_err()
        );
        for name in ["/fallback", "/ocean/field"] {
            let map = MapSlice::load(&path, name, &[0, 0], PlotKind::Map, &[]).unwrap();
            assert_eq!(map.shape, [2, 3]);
            assert!(map.cell_polygons.is_some());
        }
        let mut deep = MapSlice::load(&path, "/field", &[0, 1, 1, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(deep.values, [13.0, 17.0, 21.0, 13.5, 17.5, 21.5]);
        deep.subtract(&map).unwrap();
        assert_eq!(deep.range, Some([1.0, 1.0]));
        assert!(deep.values[3].is_nan());
    }

    #[test]
    fn curved_bounds_and_differences_require_the_same_geography() {
        let path = sample("curvilinear.nc");
        let map = MapSlice::load(&path, "/bounded", &[0, 0, 0], PlotKind::Map, &[]).unwrap();
        assert!(!map.x.inferred_bounds);
        let polygons = map.cell_polygons.as_ref().unwrap();
        assert!(polygons[1].len() == 2);
        assert!(polygons[0][0].contains(&[169.0, -21.0]));
        let mut other = map.clone();
        for lon in &mut other.x.centers {
            *lon += 360.0;
        }
        assert!(map.clone().subtract(&other).is_ok());
        other.y.centers[3] += 0.1;
        assert!(map.clone().subtract(&other).is_err());
        other = map.clone();
        other.cell_polygons.as_mut().unwrap()[0][0][0][0] += 0.1;
        assert!(map.clone().subtract(&other).is_err());
        other = map.clone();
        other.cell_polygons = None;
        assert!(map.clone().subtract(&other).is_err());
    }

    #[test]
    fn malformed_curved_coordinates_and_bounds_are_reported() {
        let path =
            std::env::temp_dir().join(format!("ncdive-curved-invalid-{}.nc", std::process::id()));
        for (variable, attribute, value, field, indices) in [
            (
                "nav_lon",
                "bounds",
                "lon_bounds",
                "/field",
                vec![0, 0, 0, 0],
            ),
            (
                "bounded_lon",
                "bounds",
                "nav_lon",
                "/bounded",
                vec![0, 0, 0],
            ),
            (
                "field",
                "coordinates",
                "nav_lon",
                "/field",
                vec![0, 0, 0, 0],
            ),
        ] {
            std::fs::copy(sample("curvilinear.nc"), &path).unwrap();
            let mut file = netcdf::append(&path).unwrap();
            file.variable_mut(variable)
                .unwrap()
                .put_attribute(attribute, value)
                .unwrap();
            file.close().unwrap();
            assert!(MapSlice::load(&path, field, &indices, PlotKind::Map, &[]).is_err());
        }
        std::fs::copy(sample("curvilinear.nc"), &path).unwrap();
        let mut file = netcdf::append(&path).unwrap();
        file.variable_mut("nav_lat")
            .unwrap()
            .put_value(91.0, [0, 0])
            .unwrap();
        file.close().unwrap();
        assert!(MapSlice::load(&path, "/field", &[0, 0, 0, 0], PlotKind::Map, &[]).is_err());
        std::fs::copy(sample("curvilinear.nc"), &path).unwrap();
        let mut file = netcdf::append(&path).unwrap();
        file.variable_mut("lat_bounds")
            .unwrap()
            .put_values(&[80.0; 24], ..)
            .unwrap();
        file.close().unwrap();
        assert!(
            MapSlice::load(&path, "/bounded", &[0, 0, 0], PlotKind::Map, &[])
                .unwrap_err()
                .to_string()
                .contains("enclose")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn extra_missing_values_match_storage_precision_and_keep_real_zeros_and_negatives() {
        let path =
            std::env::temp_dir().join(format!("ncdive-extra-missing-{}.nc", std::process::id()));
        std::fs::copy(sample("curvilinear.nc"), &path).unwrap();
        let mut file = netcdf::append(&path).unwrap();
        file.redef().unwrap();
        file.add_variable::<f32>("float_missing", &["y", "x"])
            .unwrap()
            .put_attribute("_FillValue", 1e20_f32)
            .unwrap();
        file.add_variable::<f64>("double_missing", &["y", "x"])
            .unwrap();
        file.enddef().unwrap();
        let values = [-1e35, -9999.0, 0.0, -2.5, 4.0, 1e20];
        file.variable_mut("float_missing")
            .unwrap()
            .put_values(&values, ..)
            .unwrap();
        file.variable_mut("double_missing")
            .unwrap()
            .put_values(&values, ..)
            .unwrap();
        file.close().unwrap();

        let original =
            MapSlice::load(&path, "/float_missing", &[0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(original.valid_count, 5);
        assert_eq!(original.range.unwrap()[0], f64::from(-1e35_f32));
        for name in ["/float_missing", "/double_missing"] {
            let missing = [-1e35, -9999.0, 1e20, 1e-100];
            let data = MapSlice::load(&path, name, &[0, 0], PlotKind::Map, &missing).unwrap();
            assert_eq!(data.valid_count, 3);
            assert_eq!(data.range, Some([-2.5, 4.0]));
            assert!(data.values[0].is_nan() && data.values[1].is_nan() && data.values[5].is_nan());
            assert_eq!(&data.values[2..5], &[0.0, -2.5, 4.0]);
            assert_eq!(data.missing_values, missing);
            let zeros = MapSlice::load(
                &path,
                name,
                &[0, 0],
                PlotKind::Map,
                &[-1e35, -9999.0, 1e20, 0.0],
            )
            .unwrap();
            assert_eq!(zeros.valid_count, 2);
            assert!(zeros.values[2].is_nan());
        }
        let restored =
            MapSlice::load(&path, "/float_missing", &[0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(restored.range, original.range);
        assert_eq!(restored.valid_count, original.valid_count);
        assert!(
            MapSlice::load(
                &path,
                "/float_missing",
                &[0, 0],
                PlotKind::Map,
                &[f64::INFINITY]
            )
            .is_err()
        );
        std::fs::remove_file(path).unwrap();

        // Zero is a packed field marker here, not a missing coordinate or decoded value.
        let data = MapSlice::load(
            &sample("curvilinear.nc"),
            "/field",
            &[0, 0, 0, 0],
            PlotKind::Map,
            &[0.0],
        )
        .unwrap();
        assert!(data.values[0].is_nan());
        assert_eq!(data.values[1], 14.0);
        assert_eq!(data.valid_count, 5);
        assert!(data.selectors.iter().any(|s| s.label.starts_with("0 days")));
        let data = MapSlice::load(
            &sample("temperature.nc"),
            "/temperature",
            &[0, 0, 0],
            PlotKind::Map,
            &[0.0],
        )
        .unwrap();
        assert_eq!(data.x.centers[0], 0.0);
        assert_eq!(data.y.centers[1], 0.0);
        assert_eq!(data.valid_count, 11);
    }

    #[test]
    fn selects_one_time_slice_and_masks_missing_cells() {
        let map = MapSlice::load(
            &sample("temperature.nc"),
            "/temperature",
            &[1, 0, 0],
            PlotKind::Map,
            &[],
        )
        .unwrap();
        assert_eq!(map.values.len(), 12);
        assert_eq!(map.values[0], 5.0);
        assert!(map.values[6].is_nan());
        assert_eq!(map.valid_count, 11);
        assert_eq!(map.range, Some([5.0, 28.0]));
        assert_eq!(map.y.bounds[0], [-90.0, -30.0]);
        assert!(map.selectors[0].label.contains("30 days since 2000-01-01"));
        assert!(map.selectors[0].label.contains("360_day"));
    }

    #[test]
    fn handles_packing_transposed_dimensions_depth_and_descending_latitude() {
        let path = sample("packed.nc");
        let map = MapSlice::load(&path, "/packed", &[0, 1, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(map.values, [12.0, 16.0, 20.0, 12.5, 16.5, 20.5]);
        assert_eq!(map.y.centers, [30.0, -30.0]);
        assert_eq!(map.y.bounds, [[0.0, 90.0], [-90.0, 0.0]]);
        assert!(!map.y.inferred_bounds);
        let map = MapSlice::load(&path, "/packed", &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(map.valid_count, 3);
        assert_eq!(map.range, Some([10.0, 18.5]));
        assert!(map.values[2].is_nan() && map.values[3].is_nan() && map.values[4].is_nan());
        let deep = MapSlice::load(&path, "/packed", &[0, 1, 1, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(deep.values[0], 13.0);
    }

    #[test]
    fn nan_fill_keeps_zero_coordinates_and_pressure_direction() {
        let path = std::env::temp_dir().join(format!("ncdive-nan-fill-{}.nc", std::process::id()));
        std::fs::copy(sample("reanalysis.nc"), &path).unwrap();
        let mut file = netcdf::append(&path).unwrap();
        file.redef().unwrap();
        // The public ERA-Interim sample stores double NaN fill attributes on floats and shorts.
        for name in ["latitude", "longitude", "wind"] {
            file.variable_mut(name)
                .unwrap()
                .put_attribute("_FillValue", f64::NAN)
                .unwrap();
        }
        file.close().unwrap();
        let map = MapSlice::load(&path, "/wind", &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(map.x.centers, [-180.0, -90.0, 0.0, 90.0]);
        assert_eq!(map.y.centers, [90.0, 0.0, -90.0]);
        assert_eq!(map.valid_count, 12);
        assert_eq!(map.values[0], 2.0);
        assert_eq!(map.range, Some([-0.75, 2.0]));
        let section =
            MapSlice::load(&path, "/wind", &[0, 0, 0, 0], PlotKind::LatitudeDepth, &[]).unwrap();
        assert_eq!(section.y.units, "millibars");
        assert!(section.y.positive_down);
        assert_eq!(
            section.values,
            [2.0, 1.0, 0.0, -1.0, -2.0, -3.0, -4.0, -5.0, -6.0]
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn handles_constant_missing_and_grouped_fields() {
        let path = sample("packed.nc");
        let constant = MapSlice::load(&path, "/constant", &[0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(constant.range, Some([7.0, 7.0]));
        let empty = MapSlice::load(&path, "/empty", &[0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(empty.range, None);
        assert_eq!(empty.valid_count, 0);
        let grouped = MapSlice::load(&path, "/ocean/field", &[0, 0], PlotKind::Map, &[]).unwrap();
        assert_eq!(grouped.range, Some([1.0, 6.0]));
    }

    #[test]
    fn chooses_a_compatible_view_when_switching_variables() {
        let path = sample("overturning.nc");
        for (variable, preferred, expected) in [
            ("/phys_opsia", PlotKind::Map, PlotKind::LatitudeDepth),
            ("/surface", PlotKind::LatitudeDepth, PlotKind::Map),
            ("/zonal_section", PlotKind::Map, PlotKind::LongitudeDepth),
        ] {
            assert_eq!(
                MapSlice::suggested_kind(&path, variable, preferred).unwrap(),
                expected
            );
        }
        let packed = sample("packed.nc");
        for kind in PlotKind::ALL {
            assert_eq!(
                MapSlice::suggested_kind(&packed, "/packed", kind).unwrap(),
                kind
            );
        }
        assert!(MapSlice::load(&path, "/phys_opsia", &[0, 0, 0], PlotKind::Map, &[]).is_err());
    }

    #[test]
    fn cgenie_overturning_sections_keep_pole_samples_and_physical_depth() {
        let path = sample("overturning.nc");
        let section = MapSlice::load(
            &path,
            "/phys_opsia",
            &[1, 0, 0],
            PlotKind::LatitudeDepth,
            &[],
        )
        .unwrap();
        assert_eq!(section.x.centers, [-90.0, -45.0, 0.0, 45.0, 90.0]);
        assert_eq!(section.x.bounds[0], [-90.0, -67.5]);
        assert_eq!(section.x.bounds[4], [67.5, 90.0]);
        assert_eq!(section.y.extent(), [0.0, 5000.0]);
        assert!(section.y.positive_down);
        assert_eq!(section.units, "Sv");
        assert_eq!(section.valid_count, 14);
        assert_eq!(section.values[8], 16.0);
        assert!(section.values[9].is_nan());
        assert!(section.selectors[0].label.contains("9999.5"));
        for variable in ["/bad_edges", "/bad_cf_bounds"] {
            assert!(
                MapSlice::load(&path, variable, &[0, 0], PlotKind::LatitudeDepth, &[])
                    .unwrap_err()
                    .to_string()
                    .contains("Latitude bounds")
            );
        }
    }

    #[test]
    fn sections_read_both_orientations_and_keep_depth_units() {
        let path = sample("packed.nc");
        let lat = MapSlice::load(
            &path,
            "/packed",
            &[0, 1, 0, 0],
            PlotKind::LatitudeDepth,
            &[],
        )
        .unwrap();
        assert_eq!(lat.x.centers, [30.0, -30.0]);
        assert_eq!(lat.y.centers, [5.0, 100.0]);
        assert_eq!(lat.y.units, "m");
        assert!(lat.y.positive_down);
        assert_eq!(lat.values, [12.0, 12.5, 13.0, 13.5]);
        assert_eq!(
            lat.selectors
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["lon", "time"]
        );
        let lon = MapSlice::load(
            &path,
            "/packed",
            &[0, 1, 0, 0],
            PlotKind::LongitudeDepth,
            &[],
        )
        .unwrap();
        assert_eq!(lon.values, [12.0, 16.0, 20.0, 13.0, 17.0, 21.0]);
        assert!(
            MapSlice::load(
                &sample("temperature.nc"),
                "/temperature",
                &[0, 0, 0],
                PlotKind::LatitudeDepth,
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn vertical_metadata_controls_direction_and_rejects_dimensionless_sections() {
        let path = sample("sections.nc");
        let height = MapSlice::load(
            &path,
            "/height_field",
            &[0, 0, 0],
            PlotKind::LatitudeDepth,
            &[],
        )
        .unwrap();
        assert!(!height.y.positive_down);
        assert_eq!(height.y.bounds[0], [-1500.0, -550.0]);
        assert_eq!(height.values, [1.0, 4.0, 7.0, 10.0, 13.0, 16.0]);
        let pressure = MapSlice::load(
            &path,
            "/pressure_field",
            &[0, 0, 0],
            PlotKind::LatitudeDepth,
            &[],
        )
        .unwrap();
        assert!(pressure.y.positive_down);
        assert_eq!(pressure.y.centers, [1000.0, 500.0, 100.0]);
        assert_eq!(pressure.y.bounds[0], [750.0, 1100.0]);
        assert!(
            MapSlice::load(
                &path,
                "/sigma_field",
                &[0, 0, 0],
                PlotKind::LatitudeDepth,
                &[]
            )
            .is_err()
        );
        assert!(MapSlice::load(&path, "/sigma_field", &[0, 0, 0], PlotKind::Map, &[]).is_ok());
    }

    #[test]
    fn differences_preserve_missing_values_and_reject_mismatched_units_or_grids() {
        let path = sample("packed.nc");
        let current = MapSlice::load(&path, "/packed", &[0, 1, 0, 0], PlotKind::Map, &[]).unwrap();
        let reference =
            MapSlice::load(&path, "/packed", &[0, 0, 0, 0], PlotKind::Map, &[]).unwrap();
        let mut difference = current.clone();
        difference.subtract(&reference).unwrap();
        assert_eq!(difference.range, Some([2.0, 2.0]));
        assert_eq!(difference.valid_count, 3);
        assert!(difference.values[2].is_nan());
        assert!(difference.values[3].is_nan());
        assert!(difference.reference.is_some());
        let mut bad = reference.clone();
        bad.units = "K".into();
        assert!(current.clone().subtract(&bad).is_err());
        bad = reference.clone();
        bad.x.bounds[0][0] += 1.0;
        assert!(current.clone().subtract(&bad).is_err());
        let mut wrapped = reference.clone();
        for value in &mut wrapped.x.centers {
            *value += 360.0;
        }
        for bounds in &mut wrapped.x.bounds {
            for value in bounds {
                *value += 360.0;
            }
        }
        assert!(current.clone().subtract(&wrapped).is_ok());
        let mut reversed = reference;
        reversed.y.centers.reverse();
        reversed.y.bounds.reverse();
        assert!(current.clone().subtract(&reversed).is_err());
    }

    #[test]
    fn wraps_cells_without_stretching_across_the_dateline() {
        assert_eq!(longitude_parts([-260.0, -250.0]), [[100.0, 110.0]]);
        assert_eq!(
            longitude_parts([170.0, 190.0]),
            [[170.0, 180.0], [-180.0, -170.0]]
        );
        assert_eq!(longitude_parts([-180.0, 180.0]), [[-180.0, 180.0]]);
        assert!(
            MapSlice::load(&sample("temperature.nc"), "/time", &[0], PlotKind::Map, &[]).is_err()
        );
        assert!(
            MapSlice::load(
                &sample("temperature.nc"),
                "/temperature",
                &[5, 0, 0],
                PlotKind::Map,
                &[]
            )
            .is_err()
        );
    }
}
