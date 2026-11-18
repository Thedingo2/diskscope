use crate::theme::{parse_hex, ThemeConfig};
use eframe::egui::{self, Color32, Rect, RichText, Stroke, Vec2};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct SlimeAnimator {
    happy_tex: Option<egui::TextureHandle>,
    shiver_tex: Option<egui::TextureHandle>,
    dead_tex: Option<egui::TextureHandle>,
    initialized: bool,
    start_time: Instant,
}

impl Default for SlimeAnimator {
    fn default() -> Self {
        Self {
            happy_tex: None,
            shiver_tex: None,
            dead_tex: None,
            initialized: false,
            start_time: Instant::now(),
        }
    }
}

/// Resolves `resource/theme/slimes/` next to the executable first (copied by build.rs),
/// falling back to `./resource/theme/slimes/` or macOS `.app` bundle Resources.
fn resolve_slime_theme_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("resource").join("theme").join("slimes");
            if candidate.exists() {
                return candidate;
            }
            if let Some(contents_dir) = dir.parent() {
                let resources_candidate = contents_dir
                    .join("Resources")
                    .join("resource")
                    .join("theme")
                    .join("slimes");
                if resources_candidate.exists() {
                    return resources_candidate;
                }
                if let Some(app_bundle) = contents_dir.parent() {
                    if let Some(bundle_parent) = app_bundle.parent() {
                        let beside_app = bundle_parent
                            .join("resource")
                            .join("theme")
                            .join("slimes");
                        if beside_app.exists() {
                            return beside_app;
                        }
                    }
                }
            }
        }
    }
    PathBuf::from("resource").join("theme").join("slimes")
}

fn load_texture_from_file(
    ctx: &egui::Context,
    name: &str,
    path: &Path,
) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let size = [image.width() as usize, image.height() as usize];
    let rgba = image.to_rgba8();
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_flat_samples().as_slice());
    Some(ctx.load_texture(name, color_image, egui::TextureOptions::LINEAR))
}

fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut val = bytes as f64;
    let mut idx = 0;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.2} {}", val, UNITS[idx])
    }
}

impl SlimeAnimator {
    fn try_init_textures(&mut self, ctx: &egui::Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        let theme_dir = resolve_slime_theme_dir();
        println!(
            "[DiskScope] Loading slime theme textures from: {}",
            theme_dir.display()
        );

        self.happy_tex = load_texture_from_file(ctx, "slime_happy", &theme_dir.join("happy.png"));
        self.shiver_tex =
            load_texture_from_file(ctx, "slime_shiver", &theme_dir.join("shiver.png"));
        self.dead_tex = load_texture_from_file(ctx, "slime_dead", &theme_dir.join("dead.png"));
    }

    pub fn reload_theme(&mut self) {
        self.happy_tex = None;
        self.shiver_tex = None;
        self.dead_tex = None;
        self.initialized = false;
    }

    pub fn render_card(
        &mut self,
        ui: &mut egui::Ui,
        drive: &str,
        total_disk_space: u64,
        free_disk_space: u64,
        theme: &ThemeConfig,
    ) {
        let ctx = ui.ctx().clone();
        self.try_init_textures(&ctx);

        let used_bytes = total_disk_space.saturating_sub(free_disk_space);
        let free_pct = if total_disk_space > 0 {
            ((free_disk_space as f64 / total_disk_space as f64) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let used_frac = ((100.0 - free_pct) / 100.0) as f32;

        let t = self.start_time.elapsed().as_secs_f32();
        let base_size = 54.0_f32;
        let stage_size = egui::vec2(68.0, 68.0);
        let uv_full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

        let card_bg = parse_hex(&theme.health_widget.card_bg);
        let track_bg = parse_hex(&theme.health_widget.bar_track);
        let border_col = parse_hex(&theme.window.border);
        let stage_bg = parse_hex(&theme.window.card_background);
        let text_primary = parse_hex(&theme.text.primary);
        let text_secondary = parse_hex(&theme.text.secondary);
        let text_muted = parse_hex(&theme.text.muted);

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));

        egui::Frame::NONE
            .fill(card_bg)
            .stroke(Stroke::new(1.0_f32, border_col))
            .corner_radius(8)
            .inner_margin(Vec2::new(10.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (stage_rect, response) =
                        ui.allocate_exact_size(stage_size, egui::Sense::click());
                    ui.painter().rect_filled(stage_rect, 6, stage_bg);

                    if response.clicked() {
                        self.reload_theme();
                    }
                    let _ = response
                        .on_hover_text("Click to reload resource/theme/slimes/ PNGs");

                    let ground_center =
                        egui::pos2(stage_rect.center().x, stage_rect.bottom() - 4.0);

                    let (active_tex, draw_rect, status_label, bar_color) = if free_pct > 60.0 {
                        let phase = (t * 4.5).sin();
                        let jump_height = phase.max(0.0) * 12.0;
                        let stretch_y = 1.0 + phase * 0.16;
                        let squash_x = 1.0 - phase * 0.12;

                        let w = base_size * squash_x;
                        let h = base_size * stretch_y;
                        let rect = Rect::from_min_max(
                            egui::pos2(
                                ground_center.x - w * 0.5,
                                ground_center.y - jump_height - h,
                            ),
                            egui::pos2(ground_center.x + w * 0.5, ground_center.y - jump_height),
                        );
                        (
                            &self.happy_tex,
                            rect,
                            "Disk Healthy!",
                            parse_hex(&theme.health_widget.bar_healthy),
                        )
                    } else if free_pct < 10.0 {
                        let pulse = (t * 2.0).sin() * 0.04;
                        let w = base_size * (1.26 + pulse);
                        let h = base_size * (0.62 - pulse);
                        let rect = Rect::from_min_max(
                            egui::pos2(ground_center.x - w * 0.5, ground_center.y - h),
                            egui::pos2(ground_center.x + w * 0.5, ground_center.y),
                        );
                        (
                            &self.dead_tex,
                            rect,
                            "Critical Space!",
                            parse_hex(&theme.health_widget.bar_critical),
                        )
                    } else {
                        let jitter_x = (t * 42.0).sin() * 3.0 + (t * 67.0).cos() * 1.2;
                        let tense_pulse = (t * 18.0).sin() * 0.03;
                        let w = base_size * (1.03 + tense_pulse);
                        let h = base_size * (0.95 - tense_pulse);
                        let rect = Rect::from_min_max(
                            egui::pos2(ground_center.x + jitter_x - w * 0.5, ground_center.y - h),
                            egui::pos2(ground_center.x + jitter_x + w * 0.5, ground_center.y),
                        );
                        (
                            &self.shiver_tex,
                            rect,
                            "Getting Low...",
                            parse_hex(&theme.health_widget.bar_warning),
                        )
                    };

                    if let Some(tex) = active_tex {
                        ui.painter()
                            .image(tex.id(), draw_rect, uv_full, Color32::WHITE);
                    } else {
                        ui.painter().rect_filled(draw_rect, 12.0, bar_color);
                    }

                    ui.add_space(6.0);

                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Drive {} — {:.0}% Free ({})",
                                drive, free_pct, status_label
                            ))
                            .color(text_primary)
                            .strong(),
                        );

                        let (bar_rect, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width().max(120.0), 8.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(bar_rect, 4, track_bg);
                        let fill_rect = Rect::from_min_size(
                            bar_rect.min,
                            Vec2::new(bar_rect.width() * used_frac, bar_rect.height()),
                        );
                        ui.painter().rect_filled(fill_rect, 4, bar_color);

                        ui.label(
                            RichText::new(format!(
                                "{} used / {} total ({} free)",
                                fmt_bytes(used_bytes),
                                fmt_bytes(total_disk_space),
                                fmt_bytes(free_disk_space)
                            ))
                            .size(11.5)
                            .color(text_secondary),
                        );

                        if active_tex.is_none() {
                            ui.label(
                                RichText::new("Missing resource/theme/slimes/*.png")
                                    .small()
                                    .color(text_muted),
                            );
                        }
                    });
                });
            });
    }
}