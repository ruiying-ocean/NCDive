use std::sync::OnceLock;

pub fn lines() -> &'static [Vec<[f64; 2]>] {
    static LINES: OnceLock<Vec<Vec<[f64; 2]>>> = OnceLock::new();
    LINES.get_or_init(|| {
        include_str!("../assets/coastlines.txt")
            .lines()
            .map(|line| {
                line.split_whitespace()
                    .map(|point| {
                        let (lon, lat) = point.split_once(',').expect("bundled coastline point");
                        [
                            lon.parse().expect("bundled longitude"),
                            lat.parse().expect("bundled latitude"),
                        ]
                    })
                    .collect()
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_coastlines_have_valid_geographic_coordinates() {
        assert_eq!(lines().len(), 134);
        assert!(lines().iter().all(|line| line.len() >= 2));
        assert!(lines().iter().flatten().all(|[lon, lat]| {
            lon.is_finite()
                && lat.is_finite()
                && (-180.0..=180.0).contains(lon)
                && (-90.0..=90.0).contains(lat)
        }));
    }
}
