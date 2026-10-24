use std::collections::HashMap;

use crate::map_data::wrap_longitude;

pub type GeoPolygon = Vec<[f64; 2]>;

pub fn infer_corners(lon: &[f64], lat: &[f64], [ny, nx]: [usize; 2]) -> Option<Vec<[[f64; 2]; 4]>> {
    if ny < 2 || nx < 2 {
        return None;
    }
    let centers: Vec<_> = lon
        .iter()
        .zip(lat)
        .map(|(&lon, &lat)| {
            let (lon, lat) = (lon.to_radians(), lat.to_radians());
            [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
        })
        .collect();
    let weights = |i: usize, n: usize| {
        if i == 0 {
            (0, [1.5, -0.5])
        } else if i == n {
            (n - 2, [-0.5, 1.5])
        } else {
            (i - 1, [0.5, 0.5])
        }
    };
    let mut vertices = Vec::with_capacity((ny + 1) * (nx + 1));
    // shortcut: approximate absent corners on the sphere; use supplied CF bounds for exact model cells.
    for row in 0..=ny {
        let (j, wy) = weights(row, ny);
        for column in 0..=nx {
            let (i, wx) = weights(column, nx);
            let mut v = [0.0; 3];
            for (dy, y) in wy.into_iter().enumerate() {
                for (dx, x) in wx.into_iter().enumerate() {
                    for (axis, value) in v.iter_mut().enumerate() {
                        *value += x * y * centers[(j + dy) * nx + i + dx][axis];
                    }
                }
            }
            let horizontal = v[0].hypot(v[1]);
            if horizontal.hypot(v[2]) < 1e-12 {
                return None;
            }
            vertices.push([
                v[1].atan2(v[0]).to_degrees(),
                v[2].atan2(horizontal).to_degrees(),
            ]);
        }
    }
    Some(
        (0..ny)
            .flat_map(|j| (0..nx).map(move |i| (j, i)))
            .map(|(j, i)| {
                let k = j * (nx + 1) + i;
                [
                    vertices[k],
                    vertices[k + 1],
                    vertices[k + nx + 2],
                    vertices[k + nx + 1],
                ]
            })
            .collect(),
    )
}

pub fn omit_duplicates(
    cells: &mut [Vec<GeoPolygon>],
    lon: &[f64],
    lat: &[f64],
    [ny, nx]: [usize; 2],
) {
    let mut seen = HashMap::new();
    let margin = |i: usize| {
        (i / nx)
            .min(ny - 1 - i / nx)
            .min(i % nx)
            .min(nx - 1 - i % nx)
    };
    for (i, (&x, &y)) in lon.iter().zip(lat).enumerate() {
        let key = (wrap_longitude(x).to_bits(), (y + 0.0).to_bits());
        if let Some(&previous) = seen.get(&key) {
            // NEMO halos can contain zero even when the coincident interior cell is valid.
            if margin(i) > margin(previous) {
                cells[previous].clear();
                seen.insert(key, i);
            } else {
                cells[i].clear();
            }
        } else {
            seen.insert(key, i);
        }
    }
}

/// Unwrap each cell locally, close polar caps, then clip at the map's date line.
pub fn cell_polygons(corners: [[f64; 2]; 4]) -> Option<Vec<GeoPolygon>> {
    polygon_parts(&corners)
}

fn polygon_parts(corners: &[[f64; 2]]) -> Option<Vec<GeoPolygon>> {
    if corners
        .iter()
        .any(|p| !p[0].is_finite() || !p[1].is_finite() || p[1].abs() > 90.0)
    {
        return None;
    }
    let mut ring = vec![[wrap_longitude(corners[0][0]), corners[0][1]]];
    for point in &corners[1..] {
        let previous = ring.last()?[0];
        ring.push([previous + wrap_longitude(point[0] - previous), point[1]]);
    }
    let closing = ring.last()?[0] + wrap_longitude(ring[0][0] - ring.last()?[0]);
    if ring.len() == 4 && (closing - ring[0][0]).abs() < 180.0 {
        let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
        };
        let intersects = |a, b, c, d| {
            cross(a, b, c) * cross(a, b, d) < 0.0 && cross(c, d, a) * cross(c, d, b) < 0.0
        };
        if intersects(ring[0], ring[1], ring[2], ring[3])
            || intersects(ring[1], ring[2], ring[3], ring[0])
        {
            // Folded cells occur on NEMO land; preserve the grid's two triangles.
            let mut parts = polygon_parts(&corners[..3])?;
            parts.extend(polygon_parts(&[corners[0], corners[2], corners[3]])?);
            return Some(parts);
        }
    }
    if (closing - ring[0][0]).abs() > 180.0 {
        let pole = if corners.iter().map(|p| p[1]).sum::<f64>() > 0.0 {
            90.0
        } else {
            -90.0
        };
        ring.extend([[closing, ring[0][1]], [closing, pole], [ring[0][0], pole]]);
    }
    let min = ring.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let max = ring.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
    let mut parts = Vec::new();
    for world in ((min + 180.0) / 360.0).floor() as i32..=((max + 180.0) / 360.0).floor() as i32 {
        let offset = f64::from(world) * 360.0;
        let part = clip_longitude(
            &clip_longitude(&ring, offset - 180.0, true),
            offset + 180.0,
            false,
        );
        if part.len() >= 3 && area(&part).abs() > 1e-10 {
            parts.push(
                part.into_iter()
                    .map(|[x, y]| [(x - offset).clamp(-180.0, 180.0), y])
                    .collect(),
            );
        }
    }
    Some(parts)
}

fn clip_longitude(points: &[[f64; 2]], edge: f64, keep_right: bool) -> GeoPolygon {
    let mut result = Vec::new();
    if points.is_empty() {
        return result;
    }
    let inside = |p: [f64; 2]| {
        if keep_right {
            p[0] >= edge
        } else {
            p[0] <= edge
        }
    };
    let mut a = *points.last().unwrap();
    for &b in points {
        if inside(a) != inside(b) {
            let t = (edge - a[0]) / (b[0] - a[0]);
            result.push([edge, a[1] + t * (b[1] - a[1])]);
        }
        if inside(b) {
            result.push(b);
        }
        a = b;
    }
    result
}

pub fn contains(polygon: &[[f64; 2]], [x, y]: [f64; 2]) -> bool {
    let mut inside = false;
    let mut a = polygon[polygon.len() - 1];
    for &b in polygon {
        let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        if cross.abs() <= 1e-10
            && x >= a[0].min(b[0])
            && x <= a[0].max(b[0])
            && y >= a[1].min(b[1])
            && y <= a[1].max(b[1])
        {
            return true;
        }
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        a = b;
    }
    inside
}

fn area(polygon: &[[f64; 2]]) -> f64 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_halos_yield_to_the_interior_cells() {
        let lon = [
            20., 0., 10., 20., 0., 20., 0., 10., 20., 0., 20., 0., 10., 20., 0.,
        ];
        let lat = [
            -10., -10., -10., -10., -10., 0., 0., 0., 0., 0., 10., 10., 10., 10., 10.,
        ];
        let mut cells = vec![vec![vec![[0.0, 0.0]; 4]]; 15];
        omit_duplicates(&mut cells, &lon, &lat, [3, 5]);
        assert!(cells[5].is_empty() && cells[9].is_empty());
        assert!(!cells[6].is_empty() && !cells[8].is_empty());
        assert_eq!(cells.iter().filter(|c| !c.is_empty()).count(), 9);
    }

    #[test]
    fn cells_split_at_the_date_line_and_cover_the_pole() {
        let parts = cell_polygons([
            [170.0, -10.0],
            [-170.0, -8.0],
            [-168.0, 10.0],
            [172.0, 12.0],
        ])
        .unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().any(|p| contains(p, [179.0, 0.0])));
        assert!(parts.iter().any(|p| contains(p, [-179.0, 0.0])));
        assert!(!parts.iter().any(|p| contains(p, [0.0, 0.0])));
        for sign in [-1.0, 1.0] {
            let parts = cell_polygons([
                [-135.0, sign * 80.0],
                [-45.0, sign * 80.0],
                [45.0, sign * 80.0],
                [135.0, sign * 80.0],
            ])
            .unwrap();
            for lon in [-179.0, -90.0, 0.0, 90.0, 179.0] {
                assert!(parts.iter().any(|p| contains(p, [lon, sign * 89.0])));
                assert!(!parts.iter().any(|p| contains(p, [lon, sign * 70.0])));
            }
        }
    }

    #[test]
    fn inferred_corners_are_shared_and_wrap_on_the_sphere() {
        let corners = infer_corners(
            &[178.0, -178.0, 179.0, -177.0],
            &[-2.0, -2.0, 2.0, 2.0],
            [2, 2],
        )
        .unwrap();
        assert_eq!(corners[0][2], corners[3][0]);
        assert_eq!(corners[0][1], corners[1][0]);
        assert!(
            corners
                .iter()
                .flatten()
                .all(|p| p[0].abs() > 170.0 && p[1].abs() < 5.0)
        );
        assert!(infer_corners(&[0.0], &[0.0], [1, 1]).is_none());
    }
}
