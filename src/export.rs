use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui::{self, Color32, Shape, Vec2};
use resvg::{tiny_skia, usvg};

type Result<T> = std::result::Result<T, String>;

pub fn svg(size: Vec2, shapes: &[egui::epaint::ClippedShape]) -> Result<String> {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\"><rect width=\"100%\" height=\"100%\" fill=\"white\"/>",
        size.x, size.y, size.x, size.y
    );
    for (index, clipped) in shapes.iter().enumerate() {
        let rect = clipped
            .clip_rect
            .intersect(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        if !rect.is_positive() {
            continue;
        }
        write!(svg, "<defs><clipPath id=\"c{index}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs><g clip-path=\"url(#c{index})\">", rect.left(), rect.top(), rect.width(), rect.height()).unwrap();
        shape_svg(&mut svg, &clipped.shape)?;
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    Ok(svg)
}

fn colour(colour: Color32) -> String {
    let [r, g, b, a] = colour.to_srgba_unmultiplied();
    format!("rgba({r},{g},{b},{})", f32::from(a) / 255.0)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn shape_svg(svg: &mut String, shape: &Shape) -> Result<()> {
    match shape {
        Shape::Noop => {}
        Shape::Vec(shapes) => {
            for shape in shapes {
                shape_svg(svg, shape)?;
            }
        }
        Shape::LineSegment {
            points: [a, b],
            stroke,
        } => {
            write!(
                svg,
                "<path d=\"M{} {}L{} {}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"/>",
                a.x,
                a.y,
                b.x,
                b.y,
                colour(stroke.color),
                stroke.width
            )
            .unwrap();
        }
        Shape::Path(path) => {
            let egui::epaint::ColorMode::Solid(stroke) = path.stroke.color else {
                return Err("This figure contains an unsupported gradient stroke.".into());
            };
            svg.push_str("<path d=\"");
            for (index, point) in path.points.iter().enumerate() {
                write!(
                    svg,
                    "{}{} {}",
                    if index == 0 { 'M' } else { 'L' },
                    point.x,
                    point.y
                )
                .unwrap();
            }
            if path.closed {
                svg.push('Z');
            }
            write!(
                svg,
                "\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                colour(path.fill),
                colour(stroke),
                path.stroke.width
            )
            .unwrap();
        }
        Shape::Rect(rect) => {
            write!(svg, "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>", rect.rect.left(), rect.rect.top(), rect.rect.width(), rect.rect.height(), colour(rect.fill), colour(rect.stroke.color), rect.stroke.width).unwrap();
        }
        Shape::Mesh(mesh) => {
            // Combine adjacent triangles of one colour to avoid seams inside a cell.
            let mut previous = None;
            for indices in mesh.triangles() {
                let [a, b, c] = [
                    mesh.vertices[indices[0] as usize],
                    mesh.vertices[indices[1] as usize],
                    mesh.vertices[indices[2] as usize],
                ];
                if a.color != b.color
                    || a.color != c.color
                    || [a, b, c].iter().any(|v| v.uv != egui::epaint::WHITE_UV)
                {
                    return Err(
                        "This figure contains an unsupported textured or gradient triangle.".into(),
                    );
                }
                if previous != Some(a.color) {
                    if previous.is_some() {
                        svg.push_str("\"/>");
                    }
                    write!(
                        svg,
                        "<path fill=\"{}\" shape-rendering=\"crispEdges\" d=\"",
                        colour(a.color)
                    )
                    .unwrap();
                    previous = Some(a.color);
                }
                write!(
                    svg,
                    "M{} {}L{} {}L{} {}Z",
                    a.pos.x, a.pos.y, b.pos.x, b.pos.y, c.pos.x, c.pos.y
                )
                .unwrap();
            }
            if previous.is_some() {
                svg.push_str("\"/>");
            }
        }
        Shape::Text(text) => {
            let font = text.galley.job.sections.first().map(|s| &s.format.font_id);
            let size = font.map_or(12.0, |f| f.size);
            let weight = if font.is_some_and(|f| {
                f.family == egui::FontFamily::Name(crate::typography::SEMIBOLD.into())
            }) {
                600
            } else {
                400
            };
            let family = if font.is_some_and(|f| f.family == egui::FontFamily::Monospace) {
                "Hack"
            } else {
                "Inter"
            };
            let fill = text.override_text_color.unwrap_or(text.fallback_color);
            for row in &text.galley.rows {
                let Some(first) = row.glyphs.first() else {
                    continue;
                };
                let x = text.pos.x + row.pos.x + first.pos.x;
                let y = text.pos.y + row.pos.y + first.pos.y;
                // Let the exporter shape complete words so ligatures keep all their letters.
                write!(svg, "<text x=\"{x}\" y=\"{y}\" font-family=\"{family}\" font-weight=\"{weight}\" font-size=\"{size}\" fill=\"{}\" xml:space=\"preserve\">{}</text>", colour(fill), escape(&row.text())).unwrap();
            }
        }
        _ => return Err("This figure contains a drawing type that cannot be exported yet.".into()),
    }
    Ok(())
}

pub fn encode(svg: &str, extension: &str) -> Result<Vec<u8>> {
    let mut options = usvg::Options {
        font_family: "Inter".into(),
        ..Default::default()
    };
    for font in crate::typography::fonts().font_data.values() {
        options.fontdb_mut().load_font_data(font.bytes().to_vec());
    }
    options.fontdb_mut().set_sans_serif_family("Inter");
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    match extension {
        "png" => {
            let size = tree.size().to_int_size();
            let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
                .ok_or("Could not allocate the exported image.")?;
            resvg::render(
                &tree,
                tiny_skia::Transform::identity(),
                &mut pixmap.as_mut(),
            );
            pixmap.encode_png().map_err(|e| e.to_string())
        }
        "pdf" => svg2pdf::to_pdf(
            &tree,
            svg2pdf::ConversionOptions::default(),
            svg2pdf::PageOptions::default(),
        )
        .map_err(|e| e.to_string()),
        _ => Err("Choose PNG or PDF for figure export.".into()),
    }
}

pub fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("Choose a destination folder.")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(".ncdive-export-{}-{stamp}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Could not save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_and_pdf_encode_real_text_and_escape_metadata() {
        let size = Vec2::new(800.0, 120.0);
        let context = egui::Context::default();
        crate::typography::configure(&context);
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size),
                    0.0,
                    Color32::WHITE,
                );
                ui.painter().text(
                    egui::pos2(12.0, 20.0),
                    egui::Align2::LEFT_TOP,
                    "A < B & 30°S · μmol · −5",
                    egui::FontId::proportional(18.0),
                    Color32::BLACK,
                );
                ui.painter().text(
                    egui::pos2(12.0, 60.0),
                    egui::Align2::LEFT_TOP,
                    "muffin / fields_biogem_2d.nc",
                    crate::typography::heading(20.0),
                    Color32::BLACK,
                );
            },
        );
        output.textures_delta.clear();
        let document = svg(size, &output.shapes).unwrap();
        assert!(document.contains("A &lt; B &amp; 30°S"));
        assert!(document.contains("muffin / fields_biogem_2d.nc"));
        assert!(document.contains("font-weight=\"600\""));
        let png = encode(&document, "png").unwrap();
        let image = tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert_eq!((image.width(), image.height()), (800, 120));
        assert!(
            image
                .pixels()
                .chunks(800)
                .skip(20)
                .take(40)
                .flat_map(|row| &row[12..200])
                .filter(|p| p.red() < 100)
                .count()
                > 50,
            "text must be visible"
        );
        let pdf = encode(&document, "pdf").unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.windows(5).any(|w| w == b"/Font"));
        let destination = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/export-previews");
        fs::create_dir_all(&destination).unwrap();
        save(&destination.join("typography.png"), &png).unwrap();
        save(&destination.join("typography.pdf"), &pdf).unwrap();
        assert!(encode(&document, "nc").is_err());
    }
}
