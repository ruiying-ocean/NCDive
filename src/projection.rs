use proj::Proj;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Projection {
    #[default]
    EqualEarth,
    Equirectangular,
    Mollweide,
    Robinson,
}

impl Projection {
    pub const ALL: [Self; 4] = [
        Self::EqualEarth,
        Self::Equirectangular,
        Self::Mollweide,
        Self::Robinson,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::EqualEarth => "Equal Earth",
            Self::Equirectangular => "Equirectangular (lat–lon)",
            Self::Mollweide => "Mollweide",
            Self::Robinson => "Robinson",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::EqualEarth => "Equal-area world map; preserves relative sizes of regions.",
            Self::Equirectangular => "Rectangular grid; areas stretch toward the poles.",
            Self::Mollweide => "Equal-area world map; shapes change toward the edges.",
            Self::Robinson => "Balanced world view; neither areas nor shapes are exact.",
        }
    }
}

pub struct Projector {
    pub kind: Projection,
    engine: Proj,
    extent: [f64; 2],
}

impl Projector {
    pub fn new(kind: Projection) -> Result<Self, String> {
        let definition = match kind {
            Projection::EqualEarth => "+proj=eqearth +R=1",
            Projection::Equirectangular => "+proj=eqc +R=1",
            Projection::Mollweide => "+proj=moll +R=1",
            Projection::Robinson => "+proj=robin +R=1",
        };
        let engine = Proj::new(definition).map_err(|error| error.to_string())?;
        let east = engine
            .project((std::f64::consts::PI, 0.0), false)
            .map_err(|error| error.to_string())?;
        let north = engine
            .project((0.0, std::f64::consts::FRAC_PI_2), false)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            kind,
            engine,
            extent: [east.0, north.1],
        })
    }

    pub fn aspect_ratio(&self) -> f32 {
        (self.extent[0] / self.extent[1]) as f32
    }

    /// Degrees to the world bounding box, with both coordinates in −1..1.
    pub fn forward(&self, longitude: f64, latitude: f64) -> Option<[f64; 2]> {
        if !(-180.0..=180.0).contains(&longitude) || !(-90.0..=90.0).contains(&latitude) {
            return None;
        }
        // All longitudes meet at Mollweide's poles; PROJ leaves a small residual width.
        if self.kind == Projection::Mollweide && latitude.abs() == 90.0 {
            return Some([0.0, latitude.signum()]);
        }
        let (x, y) = self
            .engine
            .project((longitude.to_radians(), latitude.to_radians()), false)
            .ok()?;
        (x.is_finite() && y.is_finite()).then_some([x / self.extent[0], y / self.extent[1]])
    }

    /// Outside the curved world boundary has no geographic coordinate.
    pub fn inverse(&self, [x, y]: [f64; 2]) -> Option<[f64; 2]> {
        if !(-1.0..=1.0).contains(&x) || !(-1.0..=1.0).contains(&y) {
            return None;
        }
        let (longitude, latitude) = self
            .engine
            .project((x * self.extent[0], y * self.extent[1]), true)
            .ok()?;
        let longitude = longitude.to_degrees();
        let latitude = latitude.to_degrees();
        let projected = self.forward(longitude, latitude)?;
        // Some inverse operations clamp outside points to a boundary.
        ((projected[0] - x).abs() < 1e-6 && (projected[1] - y).abs() < 1e-6)
            .then_some([longitude, latitude])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projections_match_reference_positions_and_keep_their_aspect_ratio() {
        // Reference points from the PROJ CLI, using a unit sphere.
        for (kind, east, north, point) in [
            (
                Projection::EqualEarth,
                2.706629983696,
                1.317362759157,
                [1.159854499103, 0.860231085522],
            ),
            (
                Projection::Equirectangular,
                std::f64::consts::PI,
                std::f64::consts::FRAC_PI_2,
                [std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_4],
            ),
            (
                Projection::Mollweide,
                2.828427124746,
                1.414213562346,
                [1.139725025132, 0.837273472104],
            ),
            (
                Projection::Robinson,
                2.666269685102,
                1.3523,
                [1.194755447547, 0.753366327292],
            ),
        ] {
            let projection = Projector::new(kind).unwrap();
            let position = projection.forward(90.0, 45.0).unwrap();
            assert!((position[0] - point[0] / east).abs() < 1e-8);
            assert!((position[1] - point[1] / north).abs() < 1e-8);
            assert!((f64::from(projection.aspect_ratio()) - east / north).abs() < 1e-6);
            assert!(
                projection
                    .forward(0.0, 90.0)
                    .unwrap()
                    .iter()
                    .all(|v| v.is_finite())
            );
            if kind == Projection::Mollweide {
                for lon in [-180.0, -90.0, 0.0, 90.0, 180.0] {
                    assert_eq!(projection.forward(lon, 90.0), Some([0.0, 1.0]));
                    assert_eq!(projection.forward(lon, -90.0), Some([0.0, -1.0]));
                }
            }
            assert_eq!(projection.forward(0.0, 91.0), None);
        }
    }

    #[test]
    fn inverse_recovers_cell_coordinates_and_rejects_outside_the_world() {
        for kind in Projection::ALL {
            let projection = Projector::new(kind).unwrap();
            for (longitude, latitude) in [
                (-175.0, -76.4638),
                (175.0, 76.4638),
                (0.0, 0.0),
                (90.0, -45.0),
            ] {
                let point = projection.forward(longitude, latitude).unwrap();
                let recovered = projection.inverse(point).unwrap();
                assert!((recovered[0] - longitude).abs() < 1e-4, "{kind:?}");
                assert!((recovered[1] - latitude).abs() < 1e-4, "{kind:?}");
            }
            assert_eq!(projection.inverse([1.1, 0.0]), None);
            if kind != Projection::Equirectangular {
                assert_eq!(projection.inverse([0.99, 0.99]), None);
            }
        }
    }
}
