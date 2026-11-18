use eframe::egui::{self, Color32, Visuals};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub name: String,
    pub is_dark: bool,
    pub window: WindowColors,
    pub text: TextColors,
    pub file_list: FileListColors,
    pub pie_chart: PieChartColors,
    pub health_widget: HealthWidgetColors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowColors {
    pub background: String,
    pub panel_background: String,
    pub card_background: String,
    pub border: String,
    pub accent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextColors {
    pub primary: String,
    pub secondary: String,
    pub muted: String,
    pub scanning: String,
    pub ready: String,
    pub warning: String,
    pub critical: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileListColors {
    pub row_hover: String,
    pub row_selected: String,
    pub size_bar_fill: String,
    pub folder_icon: String,
    pub file_icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PieChartColors {
    pub center_hole: String,
    pub slice_stroke: String,
    pub min_slice_degrees: f32,
    pub palette: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthWidgetColors {
    pub card_bg: String,
    pub bar_track: String,
    pub bar_healthy: String,
    pub bar_warning: String,
    pub bar_critical: String,
}

pub fn parse_hex(hex: &str) -> Color32 {
    let hex = hex.trim().trim_start_matches('#');
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);
            Color32::from_rgb(r, g, b)
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);
            let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
            Color32::from_rgba_unmultiplied(r, g, b, a)
        }
        _ => Color32::DEBUG_COLOR,
    }
}

impl ThemeConfig {
    pub fn themes_dir() -> PathBuf {
        PathBuf::from("themes")
    }

    /// Ensures `themes/default.toml` (Light) and `themes/dark.toml` exist, then loads the active theme.
    pub fn init_and_load(filename: &str) -> Self {
        let dir = Self::themes_dir();
        let _ = fs::create_dir_all(&dir);

        let default_path = dir.join("default.toml");
        if !default_path.exists() {
            if let Ok(s) = toml::to_string_pretty(&Self::default_light()) {
                let _ = fs::write(&default_path, s);
            }
        }

        let dark_path = dir.join("dark.toml");
        if !dark_path.exists() {
            if let Ok(s) = toml::to_string_pretty(&Self::default_dark()) {
                let _ = fs::write(&dark_path, s);
            }
        }

        Self::load_from_file(dir.join(filename))
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Self {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(cfg) = toml::from_str::<ThemeConfig>(&contents) {
                return cfg;
            }
        }
        Self::default_light()
    }

    /// Lists all `.toml` theme filenames inside the `themes/` directory.
    pub fn list_available_themes() -> Vec<String> {
        let mut list = Vec::new();
        if let Ok(entries) = fs::read_dir(Self::themes_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                        list.push(name.to_string());
                    }
                }
            }
        }
        list.sort();
        list
    }

    pub fn apply_to_ctx(&self, ctx: &egui::Context) {
        let mut visuals = if self.is_dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };

        let bg = parse_hex(&self.window.background);
        let panel_bg = parse_hex(&self.window.panel_background);
        let card_bg = parse_hex(&self.window.card_background);
        let text_primary = parse_hex(&self.text.primary);
        let border = parse_hex(&self.window.border);

        visuals.window_fill = bg;
        visuals.panel_fill = panel_bg;
        visuals.extreme_bg_color = bg;
        visuals.faint_bg_color = card_bg;
        visuals.override_text_color = Some(text_primary);
        visuals.widgets.noninteractive.bg_stroke.color = border;
        visuals.selection.bg_fill = parse_hex(&self.window.accent);

        ctx.set_visuals(visuals);
    }

    pub fn default_light() -> Self {
        Self {
            name: "Clean Light (Default)".into(),
            is_dark: false,
            window: WindowColors {
                background: "#FFFFFF".into(),
                panel_background: "#F8FAFC".into(),
                card_background: "#F1F5F9".into(),
                border: "#CBD5E1".into(),
                accent: "#2563EB".into(),
            },
            text: TextColors {
                primary: "#0F172A".into(),
                secondary: "#475569".into(),
                muted: "#94A3B8".into(),
                scanning: "#0284C7".into(),
                ready: "#16A34A".into(),
                warning: "#D97706".into(),
                critical: "#DC2626".into(),
            },
            file_list: FileListColors {
                row_hover: "#F1F5F9".into(),
                row_selected: "#DBEAFE".into(),
                size_bar_fill: "#BFDBFE".into(),
                folder_icon: "#2563EB".into(),
                file_icon: "#64748B".into(),
            },
            pie_chart: PieChartColors {
                center_hole: "#FFFFFF".into(),
                slice_stroke: "#FFFFFF".into(),
                min_slice_degrees: 1.5,
                palette: vec![
                    "#2563EB".into(),
                    "#16A34A".into(),
                    "#D97706".into(),
                    "#DC2626".into(),
                    "#9333EA".into(),
                    "#0891B2".into(),
                    "#DB2777".into(),
                    "#65A30D".into(),
                ],
            },
            health_widget: HealthWidgetColors {
                card_bg: "#FFFFFF".into(),
                bar_track: "#E2E8F0".into(),
                bar_healthy: "#16A34A".into(),
                bar_warning: "#D97706".into(),
                bar_critical: "#DC2626".into(),
            },
        }
    }

    pub fn default_dark() -> Self {
        Self {
            name: "Dark Slate".into(),
            is_dark: true,
            window: WindowColors {
                background: "#14161B".into(),
                panel_background: "#1A1D24".into(),
                card_background: "#222630".into(),
                border: "#2E3440".into(),
                accent: "#3B82F6".into(),
            },
            text: TextColors {
                primary: "#F3F4F6".into(),
                secondary: "#9CA3AF".into(),
                muted: "#6B7280".into(),
                scanning: "#38BDF8".into(),
                ready: "#4ADE80".into(),
                warning: "#FBBF24".into(),
                critical: "#F87171".into(),
            },
            file_list: FileListColors {
                row_hover: "#262B36".into(),
                row_selected: "#2D3545".into(),
                size_bar_fill: "#1E3A5F".into(),
                folder_icon: "#60A5FA".into(),
                file_icon: "#9CA3AF".into(),
            },
            pie_chart: PieChartColors {
                center_hole: "#14161B".into(),
                slice_stroke: "#14161B".into(),
                min_slice_degrees: 1.5,
                palette: vec![
                    "#3B82F6".into(),
                    "#22C55E".into(),
                    "#F59E0B".into(),
                    "#EF4444".into(),
                    "#A855F7".into(),
                    "#06B6D4".into(),
                    "#EC4899".into(),
                    "#84CC16".into(),
                ],
            },
            health_widget: HealthWidgetColors {
                card_bg: "#181A20".into(),
                bar_track: "#2A2E39".into(),
                bar_healthy: "#22C55E".into(),
                bar_warning: "#F59E0B".into(),
                bar_critical: "#EF4444".into(),
            },
        }
    }
}