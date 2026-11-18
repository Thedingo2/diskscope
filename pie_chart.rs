use crate::theme::{parse_hex, ThemeConfig};
use eframe::egui::{self, epaint::Mesh, Color32, Pos2, Shape, Stroke, Vec2};
use std::f32::consts::{FRAC_PI_2, TAU};

pub struct PieSlice {
    pub label: String,
    pub bytes: u64,
    pub color: Color32,
}

/// Filters out micro-slices below `min_slice_degrees` into an "Other" bucket
/// and assigns colors from the active `ThemeConfig`.
pub fn build_slices(
    entries: &[(String, u64)],
    total_bytes: u64,
    theme: &ThemeConfig,
) -> Vec<PieSlice> {
    if total_bytes == 0 {
        return Vec::new();
    }

    let palette: Vec<Color32> = if theme.pie_chart.palette.is_empty() {
        vec![Color32::from_rgb(59, 130, 246)]
    } else {
        theme
            .pie_chart
            .palette
            .iter()
            .map(|h| parse_hex(h))
            .collect()
    };

    let min_fraction = (theme.pie_chart.min_slice_degrees.max(0.5) / 360.0) as f64;
    let mut slices = Vec::new();
    let mut other_bytes: u64 = 0;

    for (name, size) in entries {
        if *size == 0 {
            continue;
        }
        let fraction = *size as f64 / total_bytes as f64;
        if fraction >= min_fraction {
            let color = palette[slices.len() % palette.len()];
            slices.push(PieSlice {
                label: name.clone(),
                bytes: *size,
                color,
            });
        } else {
            other_bytes = other_bytes.saturating_add(*size);
        }
    }

    if other_bytes > 0 {
        slices.push(PieSlice {
            label: "Other (<1%)".to_string(),
            bytes: other_bytes,
            color: parse_hex(&theme.text.muted),
        });
    }

    slices
}

/// Renders a donut chart using a triangle-strip mesh with zero miter-spike artifacts.
/// Returns the index of the hovered slice, if any.
pub fn draw_donut_chart(
    ui: &mut egui::Ui,
    center: Pos2,
    outer_radius: f32,
    inner_radius: f32,
    slices: &[PieSlice],
    theme: &ThemeConfig,
) -> Option<usize> {
    let total_bytes: u64 = slices.iter().map(|s| s.bytes).sum();
    if total_bytes == 0 || outer_radius <= inner_radius {
        return None;
    }

    let painter = ui.painter();
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let mut hovered_index = None;

    // Start at 12 o'clock (-PI / 2)
    let mut current_angle = -FRAC_PI_2;
    let stroke_color = parse_hex(&theme.pie_chart.slice_stroke);
    let center_hole_color = parse_hex(&theme.pie_chart.center_hole);

    for (idx, slice) in slices.iter().enumerate() {
        let fraction = slice.bytes as f32 / total_bytes as f32;
        let sweep = fraction * TAU;

        // Guard against degenerate zero-angle geometry
        if sweep <= 0.002 {
            current_angle += sweep;
            continue;
        }

        let end_angle = current_angle + sweep;

        // Hit-testing for hover highlight
        let mut is_hovered = false;
        if let Some(pos) = pointer_pos {
            let delta = pos - center;
            let dist = delta.length();
            if dist >= inner_radius && dist <= outer_radius {
                let mut angle = delta.y.atan2(delta.x);
                if angle < -FRAC_PI_2 {
                    angle += TAU;
                }
                if angle >= current_angle && angle <= end_angle {
                    is_hovered = true;
                    hovered_index = Some(idx);
                }
            }
        }

        let draw_outer = if is_hovered {
            outer_radius + 4.0
        } else {
            outer_radius
        };

        // Subdivide arc smoothly (~1 segment per 3 degrees, minimum 2)
        let steps = ((sweep.to_degrees() / 3.0).ceil() as usize).max(2);
        let mut mesh = Mesh::default();

        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let theta = current_angle + sweep * t;
            let (sin, cos) = theta.sin_cos();

            let inner_pt = center + Vec2::new(cos * inner_radius, sin * inner_radius);
            let outer_pt = center + Vec2::new(cos * draw_outer, sin * draw_outer);

            mesh.colored_vertex(inner_pt, slice.color);
            mesh.colored_vertex(outer_pt, slice.color);

            if i < steps {
                let base = (i * 2) as u32;
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base + 1, base + 3, base + 2);
            }
        }

        painter.add(Shape::mesh(mesh));

        // Draw clean radial divider line (no miter joins)
        let (sin, cos) = current_angle.sin_cos();
        painter.line_segment(
            [
                center + Vec2::new(cos * inner_radius, sin * inner_radius),
                center + Vec2::new(cos * draw_outer, sin * draw_outer),
            ],
            Stroke::new(1.5, stroke_color),
        );

        current_angle = end_angle;
    }

    // Paint clean center hole and circular borders on top
    painter.circle_filled(center, inner_radius, center_hole_color);
    painter.circle_stroke(center, inner_radius, Stroke::new(1.5, stroke_color));
    painter.circle_stroke(center, outer_radius, Stroke::new(1.5, stroke_color));

    hovered_index
}