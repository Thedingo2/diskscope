use crate::db;
use crate::debug;
use crate::health::SlimeAnimator;
use crate::pie_chart::{build_slices, draw_donut_chart};
use crate::scanner::{
    get_available_drives, get_drive_space, scan_subtree_in_memory, FileItem, ScanStatus,
};
use crate::server::{self, ServerHandle};
use crate::theme::{parse_hex, ThemeConfig};
use eframe::egui::{
    self, Align, CornerRadius, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind,
    UiBuilder, Vec2,
};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(PartialEq, Eq, Clone, Copy)]
enum RightTab {
    PieChart,
    Top10Largest,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum SlideDir {
    None,
    Forward,
    Backward,
}

pub struct DiskScopeApp {
    pub theme: ThemeConfig,
    pub active_theme_file: String,
    health_widget: SlimeAnimator,

    // Navigation & data
    current_path: String,
    history: Vec<String>,
    items: Vec<FileItem>,
    top_10_files: Vec<FileItem>,
    pending_top_10: Arc<Mutex<Option<Vec<FileItem>>>>,
    total_dir_bytes: u64,

    // Slide animation state
    prev_items: Vec<FileItem>,
    slide_dir: SlideDir,
    slide_start: Option<Instant>,

    // Drive metrics (cached so clicking folders never blocks on Win32/SQLite)
    cached_drive_root: String,
    drive_total_bytes: u64,
    drive_free_bytes: u64,

    // Search & filter
    search_query: String,
    last_search_query: String,
    filter_category: String,
    last_filter_category: String,
    search_results: Vec<FileItem>,

    // UI state
    right_tab: RightTab,
    viz_anim_t: f32,
    selected_index: Option<usize>,
    show_drive_modal: bool,
    show_logs_modal: bool,
    show_server_modal: bool,

    // Web Server Mode state
    server_handle: Option<ServerHandle>,
    server_port_input: String,
    server_pin_input: String,
    server_error: Option<String>,

    // Background scan state
    is_scanning: Arc<AtomicBool>,
    scanning_dirs: Arc<Mutex<HashSet<String>>>,
    scan_count: Arc<AtomicUsize>,
    scan_status_text: Arc<Mutex<String>>,
    live_items: Arc<Mutex<HashMap<String, (ScanStatus, u64)>>>,
    scan_dirty: Arc<AtomicBool>,
    scan_completed_flag: Arc<AtomicBool>,
}

impl DiskScopeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        db::init_db();

        let active_theme_file = "default.toml".to_string();
        let theme = ThemeConfig::init_and_load(&active_theme_file);
        theme.apply_to_ctx(&cc.egui_ctx);

        let drives = get_available_drives();
        let default_path = drives
            .first()
            .cloned()
            .unwrap_or_else(|| if cfg!(windows) { "C:\\".into() } else { "/".into() });

        let mut app = Self {
            theme,
            active_theme_file,
            health_widget: SlimeAnimator::default(),
            current_path: default_path.clone(),
            history: Vec::new(),
            items: Vec::new(),
            top_10_files: Vec::new(),
            pending_top_10: Arc::new(Mutex::new(None)),
            total_dir_bytes: 0,
            prev_items: Vec::new(),
            slide_dir: SlideDir::None,
            slide_start: None,
            cached_drive_root: String::new(),
            drive_total_bytes: 0,
            drive_free_bytes: 0,
            search_query: String::new(),
            last_search_query: String::new(),
            filter_category: "All".to_string(),
            last_filter_category: "All".to_string(),
            search_results: Vec::new(),
            right_tab: RightTab::PieChart,
            viz_anim_t: 0.0,
            selected_index: None,
            show_drive_modal: false,
            show_logs_modal: false,
            show_server_modal: false,
            server_handle: None,
            server_port_input: "8080".to_string(),
            server_pin_input: String::new(),
            server_error: None,
            is_scanning: Arc::new(AtomicBool::new(false)),
            scanning_dirs: Arc::new(Mutex::new(HashSet::new())),
            scan_count: Arc::new(AtomicUsize::new(0)),
            scan_status_text: Arc::new(Mutex::new("Ready".to_string())),
            live_items: Arc::new(Mutex::new(HashMap::new())),
            scan_dirty: Arc::new(AtomicBool::new(false)),
            scan_completed_flag: Arc::new(AtomicBool::new(false)),
        };

        app.refresh_drive_stats(true);
        app.navigate_to(&default_path, SlideDir::None, false);
        app
    }

    fn drive_root_of(path: &str) -> String {
        Path::new(path)
            .components()
            .next()
            .map(|c| {
                let mut s = c.as_os_str().to_string_lossy().into_owned();
                if cfg!(windows) && s.ends_with(':') {
                    s.push('\\');
                }
                s
            })
            .unwrap_or_else(|| path.to_string())
    }

    fn normalize_dir(target: &str) -> String {
        if target.len() > 3 && (target.ends_with('\\') || target.ends_with('/')) {
            target[..target.len() - 1].to_string()
        } else {
            target.to_string()
        }
    }

    fn refresh_drive_stats(&mut self, force: bool) {
        let root = Self::drive_root_of(&self.current_path);
        if !force && self.cached_drive_root == root {
            return;
        }
        self.cached_drive_root = root.clone();
        let (total, free) = get_drive_space(&root);
        self.drive_total_bytes = total;
        self.drive_free_bytes = free;

        let slot = Arc::clone(&self.pending_top_10);
        thread::spawn(move || {
            let files = db::get_top_files(&root, 10);
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(files);
            }
        });
    }

    fn navigate_to(&mut self, target: &str, slide: SlideDir, push_history: bool) {
        let normalized = Self::normalize_dir(target);
        if push_history && self.current_path != normalized {
            self.history.push(self.current_path.clone());
        }

        if slide != SlideDir::None {
            self.prev_items = self.items.clone();
            self.slide_dir = slide;
            self.slide_start = Some(Instant::now());
        } else {
            self.slide_dir = SlideDir::None;
            self.slide_start = None;
        }

        self.current_path = normalized;
        self.selected_index = None;
        self.search_query.clear();
        self.last_search_query.clear();
        self.search_results.clear();

        self.refresh_drive_stats(false);
        self.reload_current_items();
    }

    fn navigate_back(&mut self) {
        if let Some(prev) = self.history.pop() {
            self.navigate_to(&prev, SlideDir::Backward, false);
            return;
        }

        let norm_drive = Self::normalize_dir(&Self::drive_root_of(&self.current_path));
        if self.current_path != norm_drive {
            if let Some(parent) = Path::new(&self.current_path).parent() {
                let p_str = parent.to_string_lossy().into_owned();
                if !p_str.is_empty() {
                    self.navigate_to(&p_str, SlideDir::Backward, false);
                }
            }
        }
    }

    fn reload_current_items(&mut self) {
        let live_snapshot: HashMap<String, (ScanStatus, u64)> = self
            .live_items
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        let scanning_set: HashSet<String> = self
            .scanning_dirs
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        let mut loaded = db::get_children(&self.current_path);
        if loaded.is_empty() {
            let cached_sizes = db::get_cached_sizes(&self.current_path);
            if let Ok(entries) = fs::read_dir(&self.current_path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    let path_str = p.to_string_lossy().into_owned();
                    let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    let (size, status) = if let Some(&(st, sz)) = live_snapshot.get(&path_str) {
                        (sz, st)
                    } else if scanning_set.contains(&path_str) {
                        (0, ScanStatus::Scanning)
                    } else if let Some(&sz) = cached_sizes.get(&path_str) {
                        (sz, ScanStatus::Done)
                    } else if !is_dir {
                        (
                            entry.metadata().map(|m| m.len()).unwrap_or(0),
                            ScanStatus::Done,
                        )
                    } else {
                        (0, ScanStatus::Pending)
                    };

                    loaded.push(FileItem {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        path: path_str,
                        size,
                        is_dir,
                        status,
                    });
                }
            }
        } else {
            for item in &mut loaded {
                if let Some(&(st, sz)) = live_snapshot.get(&item.path) {
                    item.status = st;
                    if sz > 0 {
                        item.size = sz;
                    }
                } else if scanning_set.contains(&item.path) {
                    item.status = ScanStatus::Scanning;
                }
            }
        }

        loaded.sort_by(|a, b| {
            if a.is_dir != b.is_dir {
                b.is_dir.cmp(&a.is_dir)
            } else {
                b.size.cmp(&a.size)
            }
        });
        self.total_dir_bytes = loaded.iter().map(|i| i.size).sum();
        self.items = loaded;
    }

    fn start_scan(&mut self, target: String) {
        if self
            .is_scanning
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let normalized = Self::normalize_dir(&target);
        self.scan_count.store(0, Ordering::SeqCst);
        self.scan_completed_flag.store(false, Ordering::SeqCst);
        if let Ok(mut st) = self.scan_status_text.lock() {
            *st = format!("Scanning {}...", normalized);
        }

        let is_scanning = Arc::clone(&self.is_scanning);
        let scanning_dirs = Arc::clone(&self.scanning_dirs);
        let scan_count = Arc::clone(&self.scan_count);
        let scan_status_text = Arc::clone(&self.scan_status_text);
        let live_items = Arc::clone(&self.live_items);
        let scan_dirty = Arc::clone(&self.scan_dirty);
        let scan_completed_flag = Arc::clone(&self.scan_completed_flag);

        let mut top_items = Vec::new();
        if let Ok(entries) = fs::read_dir(&normalized) {
            if let Ok(mut sdirs) = scanning_dirs.lock() {
                sdirs.clear();
                sdirs.insert(normalized.clone());
                for entry in entries.flatten() {
                    let p = entry.path();
                    let p_str = p.to_string_lossy().into_owned();
                    let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    if is_dir {
                        sdirs.insert(p_str.clone());
                    }
                    top_items.push(FileItem {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        path: p_str,
                        size: 0,
                        is_dir,
                        status: if is_dir {
                            ScanStatus::Scanning
                        } else {
                            ScanStatus::Done
                        },
                    });
                }
            }
        }

        if let Ok(mut map) = live_items.lock() {
            map.clear();
            for item in &top_items {
                map.insert(item.path.clone(), (item.status, item.size));
            }
        }
        self.reload_current_items();

        thread::spawn(move || {
            let db_lock = Mutex::new(());

            top_items.par_iter_mut().for_each(|item| {
                let p = PathBuf::from(&item.path);
                if item.is_dir {
                    let (sz, sub_rows) = scan_subtree_in_memory(&p);
                    item.size = sz;
                    item.status = ScanStatus::Done;
                    scan_count.fetch_add(sub_rows.len() + 1, Ordering::Relaxed);

                    if let Ok(_guard) = db_lock.lock() {
                        if let Some(mut conn) = db::open_conn() {
                            db::insert_rows(&mut conn, &sub_rows);
                        }
                        db::save_items_to_cache(std::slice::from_ref(item));
                        db::mark_indexed(&item.path);
                    }

                    if let Ok(mut sdirs) = scanning_dirs.lock() {
                        sdirs.remove(&item.path);
                    }
                    if let Ok(mut map) = live_items.lock() {
                        map.insert(item.path.clone(), (ScanStatus::Done, sz));
                    }
                    scan_dirty.store(true, Ordering::Release);
                } else if let Ok(meta) = p.metadata() {
                    item.size = meta.len();
                    item.status = ScanStatus::Done;
                    scan_count.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut map) = live_items.lock() {
                        map.insert(item.path.clone(), (ScanStatus::Done, item.size));
                    }
                    scan_dirty.store(true, Ordering::Release);
                }
            });

            if let Ok(_guard) = db_lock.lock() {
                db::save_items_to_cache(&top_items);
                db::mark_indexed(&normalized);

                if Path::new(&normalized).parent().is_some() {
                    let total_sz: u64 = top_items.iter().map(|i| i.size).sum();
                    let name = Path::new(&normalized)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| normalized.clone());
                    db::save_items_to_cache(&[FileItem {
                        name,
                        path: normalized.clone(),
                        size: total_sz,
                        is_dir: true,
                        status: ScanStatus::Done,
                    }]);
                }
            }

            if let Ok(mut sdirs) = scanning_dirs.lock() {
                sdirs.clear();
            }
            if let Ok(mut map) = live_items.lock() {
                map.clear();
            }

            let total_indexed = scan_count.load(Ordering::Relaxed);
            if let Ok(mut st) = scan_status_text.lock() {
                *st = format!("Done scanning ({} items indexed)", total_indexed);
            }
            debug::log(format!(
                "[DiskScope] Finished scanning '{}' ({} items)",
                normalized, total_indexed
            ));
            is_scanning.store(false, Ordering::SeqCst);
            scan_completed_flag.store(true, Ordering::SeqCst);
        });
    }

    fn reload_theme(&mut self, ctx: &egui::Context) {
        self.theme =
            ThemeConfig::load_from_file(ThemeConfig::themes_dir().join(&self.active_theme_file));
        self.theme.apply_to_ctx(ctx);
        self.health_widget.reload_theme();
    }

    fn update_search_if_needed(&mut self) {
        if self.search_query != self.last_search_query
            || self.filter_category != self.last_filter_category
        {
            self.last_search_query = self.search_query.clone();
            self.last_filter_category = self.filter_category.clone();

            if self.search_query.trim().is_empty() && self.filter_category == "All" {
                self.search_results.clear();
            } else {
                let root = Self::drive_root_of(&self.current_path);
                self.search_results =
                    db::search_items(&root, &self.search_query, &self.filter_category, 250);
            }
        }
    }

    fn render_file_list(
        &mut self,
        ui: &mut egui::Ui,
        list: &[FileItem],
        is_search_mode: bool,
        interactive: bool,
    ) {
        let row_hover = parse_hex(&self.theme.file_list.row_hover);
        let row_selected = parse_hex(&self.theme.file_list.row_selected);
        let size_bar_fill = parse_hex(&self.theme.file_list.size_bar_fill);
        let folder_icon_color = parse_hex(&self.theme.file_list.folder_icon);
        let file_icon_color = parse_hex(&self.theme.file_list.file_icon);
        let accent_color = parse_hex(&self.theme.window.accent);
        let text_primary = parse_hex(&self.theme.text.primary);
        let text_secondary = parse_hex(&self.theme.text.secondary);
        let text_muted = parse_hex(&self.theme.text.muted);
        let text_scanning = parse_hex(&self.theme.text.scanning);
        let text_critical = parse_hex(&self.theme.text.critical);

        if list.is_empty() {
            ui.add_space(32.0);
            ui.vertical_centered(|ui| {
                let msg = if is_search_mode {
                    "No matching indexed files found."
                } else {
                    "Folder is empty or unreadable."
                };
                ui.label(RichText::new(msg).color(text_muted));
            });
            return;
        }

        let max_size = list.first().map(|e| e.size).unwrap_or(1).max(1);
        let mut navigate_target: Option<String> = None;
        let mut delete_target: Option<(String, u64)> = None;

        egui::ScrollArea::vertical()
            .id_salt(if !interactive {
                "file_list_prev_scroll"
            } else if is_search_mode {
                "file_list_search_scroll"
            } else {
                "file_list_scroll"
            })
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (idx, item) in list.iter().enumerate() {
                    let is_locked = item.is_dir && item.status == ScanStatus::Scanning;
                    let row_height = if is_search_mode { 38.0 } else { 28.0 };
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_height),
                        if interactive && !is_locked {
                            Sense::click()
                        } else {
                            Sense::hover()
                        },
                    );

                    let is_selected = interactive && self.selected_index == Some(idx);
                    let is_hovered = interactive && !is_locked && response.hovered();

                    if is_selected {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(4), row_selected);
                    } else if is_hovered {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(4), row_hover);
                    }

                    let ratio = (item.size as f32 / max_size as f32).clamp(0.0, 1.0);
                    if ratio > 0.005 {
                        let bar_rect = Rect::from_min_size(
                            rect.min,
                            Vec2::new(rect.width() * ratio, rect.height()),
                        );
                        ui.painter()
                            .rect_filled(bar_rect, CornerRadius::same(4), size_bar_fill);
                    }

                    if is_selected {
                        ui.painter().rect_stroke(
                            rect,
                            CornerRadius::same(4),
                            Stroke::new(1.0_f32, accent_color),
                            StrokeKind::Inside,
                        );
                    }

                    let icon = if is_locked {
                        "⏳"
                    } else if item.is_dir {
                        "📁"
                    } else {
                        "📄"
                    };
                    let icon_col = if item.is_dir {
                        folder_icon_color
                    } else {
                        file_icon_color
                    };

                    ui.painter().text(
                        rect.left_center() + Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        icon,
                        egui::FontId::proportional(14.0),
                        icon_col,
                    );

                    let name_color = if is_locked {
                        text_scanning
                    } else {
                        text_primary
                    };

                    if is_search_mode {
                        ui.painter().text(
                            rect.left_top() + Vec2::new(30.0, 5.0),
                            egui::Align2::LEFT_TOP,
                            &item.name,
                            egui::FontId::proportional(13.0),
                            name_color,
                        );
                        ui.painter().text(
                            rect.left_bottom() + Vec2::new(30.0, -4.0),
                            egui::Align2::LEFT_BOTTOM,
                            &item.path,
                            egui::FontId::proportional(10.5),
                            text_muted,
                        );
                    } else {
                        ui.painter().text(
                            rect.left_center() + Vec2::new(30.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &item.name,
                            egui::FontId::proportional(13.5),
                            name_color,
                        );
                    }

                    let size_label = if is_locked {
                        "Scanning...".to_string()
                    } else if item.is_dir && item.status == ScanStatus::Pending && item.size == 0 {
                        "Unscanned".to_string()
                    } else {
                        format_bytes(item.size)
                    };

                    ui.painter().text(
                        rect.right_center() - Vec2::new(10.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        size_label,
                        egui::FontId::monospace(12.5),
                        if is_locked {
                            text_scanning
                        } else {
                            text_secondary
                        },
                    );

                    if interactive && !is_locked && response.clicked() {
                        if item.is_dir {
                            navigate_target = Some(item.path.clone());
                        } else {
                            self.selected_index = Some(idx);
                        }
                    }

                    if interactive && !is_locked && response.double_clicked() && !item.is_dir {
                        open_item(&item.path);
                    }

                    if interactive && !is_locked {
                        response.context_menu(|ui| {
                            if ui.button("Open").clicked() {
                                if item.is_dir {
                                    navigate_target = Some(item.path.clone());
                                } else {
                                    open_item(&item.path);
                                }
                                ui.close_menu();
                            }
                            if ui.button("Open File Location").clicked() {
                                reveal_in_file_manager(&item.path);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui
                                .button(RichText::new("Move to Trash").color(text_critical))
                                .clicked()
                            {
                                delete_target = Some((item.path.clone(), item.size));
                                ui.close_menu();
                            }
                        });
                    }
                }
            });

        if let Some((target_path, sz)) = delete_target {
            if trash::delete(&target_path).is_ok() {
                db::remove_path(&target_path, sz);
                self.reload_current_items();
                self.refresh_drive_stats(true);
            }
        }

        if let Some(next_path) = navigate_target {
            self.navigate_to(&next_path, SlideDir::Forward, true);
        }
    }
}

impl eframe::App for DiskScopeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(mut slot) = self.pending_top_10.lock() {
            if let Some(files) = slot.take() {
                self.top_10_files = files;
            }
        }

        let scanning_now = self.is_scanning.load(Ordering::SeqCst);
        if scanning_now {
            if self.scan_dirty.swap(false, Ordering::Acquire) {
                self.reload_current_items();
            }
            ctx.request_repaint_after(Duration::from_millis(80));
        } else if self.scan_completed_flag.swap(false, Ordering::SeqCst) {
            self.reload_current_items();
            self.refresh_drive_stats(true);
        }

        self.update_search_if_needed();

        let bg_color = parse_hex(&self.theme.window.background);
        let panel_bg = parse_hex(&self.theme.window.panel_background);
        let card_bg = parse_hex(&self.theme.window.card_background);
        let border_color = parse_hex(&self.theme.window.border);

        let text_primary = parse_hex(&self.theme.text.primary);
        let text_secondary = parse_hex(&self.theme.text.secondary);
        let text_muted = parse_hex(&self.theme.text.muted);
        let text_scanning = parse_hex(&self.theme.text.scanning);
        let text_ready = parse_hex(&self.theme.text.ready);
        let text_critical = parse_hex(&self.theme.text.critical);

        // 1. TOP TOOLBAR
        egui::TopBottomPanel::top("top_toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(panel_bg)
                    .inner_margin(Margin::symmetric(10, 8))
                    .stroke(Stroke::new(1.0_f32, border_color)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.menu_button(RichText::new("☰ Menu").color(text_primary), |ui| {
                        ui.set_min_width(220.0);

                        let server_label = if let Some(srv) = &self.server_handle {
                            format!("🌐 Server Mode (On :{})", srv.port)
                        } else {
                            "🌐 Server Mode...".to_string()
                        };
                        if ui.button(server_label).clicked() {
                            self.show_server_modal = true;
                            ui.close_menu();
                        }

                        ui.separator();
                        ui.label(
                            RichText::new("Themes (themes/*.toml)")
                                .color(text_secondary)
                                .small(),
                        );

                        for file in ThemeConfig::list_available_themes() {
                            let selected = self.active_theme_file == file;
                            if ui.selectable_label(selected, &file).clicked() {
                                self.active_theme_file = file;
                                self.reload_theme(ctx);
                                ui.close_menu();
                            }
                        }

                        ui.separator();
                        if ui.button("↻ Reload Theme Config").clicked() {
                            self.reload_theme(ctx);
                            ui.close_menu();
                        }
                        if ui.button("🔄 Rescan Current Folder").clicked() {
                            self.start_scan(self.current_path.clone());
                            ui.close_menu();
                        }
                        if ui.button("📋 View Debug Logs").clicked() {
                            self.show_logs_modal = true;
                            ui.close_menu();
                        }
                    });

                    // Drive Selector Button
                    let drive_label = Self::drive_root_of(&self.current_path);
                    if ui
                        .button(RichText::new(format!("💾 {}", drive_label)).color(text_primary))
                        .clicked()
                    {
                        self.show_drive_modal = true;
                    }

                    // Back Button
                    let can_go_back = !self.history.is_empty()
                        || self.current_path
                            != Self::normalize_dir(&Self::drive_root_of(&self.current_path));
                    if ui
                        .add_enabled(
                            can_go_back,
                            egui::Button::new(RichText::new("⬅ Back").color(if can_go_back {
                                text_primary
                            } else {
                                text_muted
                            })),
                        )
                        .clicked()
                    {
                        self.navigate_back();
                    }

                    // Current Path Badge
                    egui::Frame::NONE
                        .fill(card_bg)
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&self.current_path)
                                    .color(text_primary)
                                    .strong(),
                            );
                        });

                    // Right-aligned Scan Folder button
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !scanning_now,
                                egui::Button::new(
                                    RichText::new(if scanning_now {
                                        "⏳ Scanning..."
                                    } else {
                                        "🔍 Scan Folder"
                                    })
                                    .color(text_primary),
                                ),
                            )
                            .clicked()
                        {
                            self.start_scan(self.current_path.clone());
                        }
                    });
                });
            });

        // 2. BOTTOM STATUS BAR (with animated Progress Bar during scan)
        egui::TopBottomPanel::bottom("bottom_status_bar")
            .frame(
                egui::Frame::NONE
                    .fill(panel_bg)
                    .inner_margin(Margin::symmetric(10, 5))
                    .stroke(Stroke::new(1.0_f32, border_color)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let status_str = self
                        .scan_status_text
                        .lock()
                        .map(|g| g.clone())
                        .unwrap_or_else(|_| "Ready".to_string());

                    let (dot_color, status_color) = if scanning_now {
                        (text_scanning, text_scanning)
                    } else {
                        (text_ready, text_secondary)
                    };

                    ui.label(RichText::new("●").color(dot_color));

                    if scanning_now {
                        let count = self.scan_count.load(Ordering::Relaxed);
                        ui.label(
                            RichText::new(format!(
                                "{} ({} items scanned — completed folders unlocked)",
                                status_str, count
                            ))
                            .color(status_color),
                        );
                        ui.add(egui::ProgressBar::new(0.5).animate(true).desired_width(140.0));
                    } else {
                        ui.label(RichText::new(status_str).color(status_color));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Theme: {}", self.theme.name))
                                .color(text_muted)
                                .small(),
                        );

                        if let Some(srv) = &self.server_handle {
                            ui.separator();
                            if ui
                                .selectable_label(
                                    false,
                                    RichText::new(format!(
                                        "🌐 Web UI: http://localhost:{}",
                                        srv.port
                                    ))
                                    .color(text_ready)
                                    .small(),
                                )
                                .clicked()
                            {
                                self.show_server_modal = true;
                            }
                        }
                    });
                });
            });

        // 3. MAIN SPLIT VIEW
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg_color).inner_margin(Margin::same(8)))
            .show(ctx, |ui| {
                let total_width = ui.available_width();
                let half_width = (total_width - 12.0) * 0.5;
                let full_height = ui.available_height();

                ui.horizontal(|ui| {
                    // ==========================================
                    // LEFT PANEL (50%): SEARCH + SWIPE FILE LIST
                    // ==========================================
                    ui.allocate_ui_with_layout(
                        Vec2::new(half_width, full_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("🔍").color(text_secondary));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.search_query)
                                        .hint_text("Search files across drive...")
                                        .desired_width((half_width - 155.0).max(100.0)),
                                );

                                egui::ComboBox::from_id_salt("category_filter")
                                    .selected_text(&self.filter_category)
                                    .width(110.0)
                                    .show_ui(ui, |ui| {
                                        for opt in [
                                            "All",
                                            "Videos",
                                            "Archives",
                                            "Images",
                                            "Audio",
                                            "Executables",
                                        ] {
                                            ui.selectable_value(
                                                &mut self.filter_category,
                                                opt.to_string(),
                                                opt,
                                            );
                                        }
                                    });
                            });

                            ui.add_space(6.0);

                            let is_search_mode = !self.search_query.trim().is_empty()
                                || self.filter_category != "All";

                            let list_rect = ui.available_rect_before_wrap();
                            ui.allocate_rect(list_rect, Sense::hover());

                            let mut clip_ui = ui.new_child(
                                UiBuilder::new()
                                    .max_rect(list_rect)
                                    .layout(Layout::top_down(Align::Min)),
                            );
                            clip_ui.set_clip_rect(list_rect);

                            if is_search_mode {
                                let results_clone = self.search_results.clone();
                                self.render_file_list(&mut clip_ui, &results_clone, true, true);
                            } else if let Some(start) = self.slide_start {
                                let elapsed = start.elapsed().as_secs_f32();
                                let dur = 0.18;
                                if elapsed >= dur {
                                    self.slide_start = None;
                                    self.slide_dir = SlideDir::None;
                                    let items_clone = self.items.clone();
                                    self.render_file_list(&mut clip_ui, &items_clone, false, true);
                                } else {
                                    ctx.request_repaint();
                                    let t = elapsed / dur;
                                    let ease = 1.0 - (1.0 - t).powi(3);
                                    let sign = if self.slide_dir == SlideDir::Forward {
                                        1.0
                                    } else {
                                        -1.0
                                    };
                                    let offset = list_rect.width() * (1.0 - ease) * sign;

                                    let old_rect = list_rect
                                        .translate(Vec2::new(offset - list_rect.width() * sign, 0.0));
                                    let new_rect = list_rect.translate(Vec2::new(offset, 0.0));

                                    let prev_clone = self.prev_items.clone();
                                    let mut old_ui = clip_ui.new_child(
                                        UiBuilder::new()
                                            .max_rect(old_rect)
                                            .layout(Layout::top_down(Align::Min)),
                                    );
                                    old_ui.set_clip_rect(list_rect);
                                    self.render_file_list(&mut old_ui, &prev_clone, false, false);

                                    let items_clone = self.items.clone();
                                    let mut new_ui = clip_ui.new_child(
                                        UiBuilder::new()
                                            .max_rect(new_rect)
                                            .layout(Layout::top_down(Align::Min)),
                                    );
                                    new_ui.set_clip_rect(list_rect);
                                    self.render_file_list(&mut new_ui, &items_clone, false, true);
                                }
                            } else {
                                let items_clone = self.items.clone();
                                self.render_file_list(&mut clip_ui, &items_clone, false, true);
                            }
                        },
                    );

                    ui.separator();

                    // ==========================================
                    // RIGHT PANEL (50%): PIE CHART + SLIME HEALTH
                    // ==========================================
                    ui.allocate_ui_with_layout(
                        Vec2::new(half_width, full_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.horizontal(|ui| {
                                let pie_active = self.right_tab == RightTab::PieChart;
                                if ui
                                    .selectable_label(
                                        pie_active,
                                        RichText::new("📊 Pie Chart").color(if pie_active {
                                            text_primary
                                        } else {
                                            text_secondary
                                        }),
                                    )
                                    .clicked()
                                {
                                    self.right_tab = RightTab::PieChart;
                                }

                                let top10_active = self.right_tab == RightTab::Top10Largest;
                                if ui
                                    .selectable_label(
                                        top10_active,
                                        RichText::new("🏆 Top 10 Largest Files").color(
                                            if top10_active {
                                                text_primary
                                            } else {
                                                text_secondary
                                            },
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.right_tab = RightTab::Top10Largest;
                                    if self.top_10_files.is_empty() {
                                        self.refresh_drive_stats(true);
                                    }
                                }
                            });

                            let target_t = if self.right_tab == RightTab::PieChart {
                                0.0
                            } else {
                                1.0
                            };
                            if (self.viz_anim_t - target_t).abs() > 0.01 {
                                self.viz_anim_t += (target_t - self.viz_anim_t) * 0.25;
                                ctx.request_repaint();
                            } else {
                                self.viz_anim_t = target_t;
                            }

                            ui.add_space(8.0);

                            let health_card_height = 88.0;
                            let main_chart_height =
                                (ui.available_height() - health_card_height - 12.0).max(120.0);

                            let chart_area = Rect::from_min_size(
                                ui.cursor().min,
                                Vec2::new(ui.available_width(), main_chart_height),
                            );
                            ui.allocate_rect(chart_area, Sense::hover());

                            let slide_px = chart_area.width() * self.viz_anim_t;
                            let pie_rect = chart_area.translate(Vec2::new(-slide_px, 0.0));
                            let top_rect = chart_area
                                .translate(Vec2::new(chart_area.width() - slide_px, 0.0));

                            if self.viz_anim_t < 0.99 {
                                let mut pie_ui = ui.new_child(
                                    UiBuilder::new()
                                        .max_rect(pie_rect)
                                        .layout(Layout::top_down(Align::Center)),
                                );
                                pie_ui.set_clip_rect(chart_area);

                                let pairs: Vec<(String, u64)> = self
                                    .items
                                    .iter()
                                    .map(|e| (e.name.clone(), e.size))
                                    .collect();
                                let slices =
                                    build_slices(&pairs, self.total_dir_bytes, &self.theme);

                                let chart_area_h =
                                    (main_chart_height * 0.48).clamp(130.0, 220.0);
                                let (donut_rect, _) = pie_ui.allocate_exact_size(
                                    Vec2::new(pie_ui.available_width(), chart_area_h),
                                    Sense::hover(),
                                );
                                let outer_r = (chart_area_h * 0.45).max(40.0);
                                let inner_r = outer_r * 0.44;

                                let hovered = draw_donut_chart(
                                    &mut pie_ui,
                                    donut_rect.center(),
                                    outer_r,
                                    inner_r,
                                    &slices,
                                    &self.theme,
                                );

                                if let Some(h_idx) = hovered {
                                    if let Some(s) = slices.get(h_idx) {
                                        egui::show_tooltip_at_pointer(
                                            ctx,
                                            pie_ui.layer_id(),
                                            egui::Id::new("pie_hover_tip"),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{}: {}",
                                                        s.label,
                                                        format_bytes(s.bytes)
                                                    ))
                                                    .color(text_primary),
                                                );
                                            },
                                        );
                                    }
                                }

                                pie_ui.add_space(6.0);

                                egui::ScrollArea::vertical()
                                    .id_salt("pie_legend_scroll")
                                    .show(&mut pie_ui, |ui| {
                                        for slice in &slices {
                                            ui.horizontal(|ui| {
                                                let (dot_rect, _) = ui.allocate_exact_size(
                                                    Vec2::splat(10.0),
                                                    Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    dot_rect,
                                                    CornerRadius::same(3),
                                                    slice.color,
                                                );
                                                ui.label(
                                                    RichText::new(&slice.label)
                                                        .color(text_primary),
                                                );
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(format_bytes(
                                                                slice.bytes,
                                                            ))
                                                            .color(text_secondary)
                                                            .monospace(),
                                                        );
                                                    },
                                                );
                                            });
                                        }
                                    });
                            }

                            if self.viz_anim_t > 0.01 {
                                let mut top_ui = ui.new_child(
                                    UiBuilder::new()
                                        .max_rect(top_rect)
                                        .layout(Layout::top_down(Align::Min)),
                                );
                                top_ui.set_clip_rect(chart_area);

                                egui::ScrollArea::vertical()
                                    .id_salt("top10_scroll")
                                    .show(&mut top_ui, |ui| {
                                        if self.top_10_files.is_empty() {
                                            ui.label(
                                                RichText::new(
                                                    "No indexed files yet. Click Scan Folder.",
                                                )
                                                .color(text_muted),
                                            );
                                        }
                                        for (rank, file) in self.top_10_files.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "#{}. {}",
                                                        rank + 1,
                                                        file.name
                                                    ))
                                                    .color(text_primary),
                                                )
                                                .on_hover_text(&file.path);

                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(format_bytes(
                                                                file.size,
                                                            ))
                                                            .color(text_secondary)
                                                            .monospace(),
                                                        );
                                                    },
                                                );
                                            });
                                        }
                                    });
                            }

                            let drive_label = Self::drive_root_of(&self.current_path);
                            self.health_widget.render_card(
                                ui,
                                &drive_label,
                                self.drive_total_bytes,
                                self.drive_free_bytes,
                                &self.theme,
                            );
                        },
                    );
                });
            });

        // 4. SERVER MODE MODAL
        if self.show_server_modal {
            egui::Window::new("🌐 DiskScope Web Server Mode")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(320.0);

                    let is_running = self.server_handle.is_some();

                    ui.horizontal(|ui| {
                        ui.label("Port:");
                        ui.add_enabled(
                            !is_running,
                            egui::TextEdit::singleline(&mut self.server_port_input)
                                .desired_width(80.0),
                        );
                    });

                    ui.horizontal(|ui| {
                        ui.label("Optional PIN:");
                        ui.add_enabled(
                            !is_running,
                            egui::TextEdit::singleline(&mut self.server_pin_input)
                                .hint_text("Leave blank for none")
                                .password(true)
                                .desired_width(160.0),
                        );
                    });

                    if let Some(err) = &self.server_error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(err).color(text_critical).small());
                    }

                    if let Some(srv) = &self.server_handle {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "● Running on http://localhost:{}",
                                srv.port
                            ))
                            .color(text_ready)
                            .strong(),
                        );
                    }

                    ui.add_space(8.0);
                    ui.separator();

                    ui.horizontal(|ui| {
                        if is_running {
                            if ui
                                .button(RichText::new("⏹ Stop Server").color(text_critical))
                                .clicked()
                            {
                                if let Some(mut handle) = self.server_handle.take() {
                                    handle.stop();
                                }
                                self.server_error = None;
                            }
                        } else if ui
                            .button(RichText::new("▶ Start Server").color(text_ready))
                            .clicked()
                        {
                            match self.server_port_input.trim().parse::<u16>() {
                                Ok(port) if port > 0 => {
                                    match server::start_background(
                                        port,
                                        self.server_pin_input.trim().to_string(),
                                    ) {
                                        Ok(handle) => {
                                            self.server_handle = Some(handle);
                                            self.server_error = None;
                                        }
                                        Err(e) => {
                                            self.server_error = Some(e);
                                        }
                                    }
                                }
                                _ => {
                                    self.server_error =
                                        Some("Please enter a valid port (1–65535).".to_string());
                                }
                            }
                        }

                        if ui.button("Close").clicked() {
                            self.show_server_modal = false;
                        }
                    });
                });
        }

        // 5. DRIVE PICKER MODAL
        if self.show_drive_modal {
            egui::Window::new("Select Drive")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    for drive in get_available_drives() {
                        if ui
                            .button(RichText::new(&drive).color(text_primary))
                            .clicked()
                        {
                            self.show_drive_modal = false;
                            self.history.clear();
                            self.navigate_to(&drive, SlideDir::None, false);
                            self.refresh_drive_stats(true);
                        }
                    }
                    ui.separator();
                    if ui.button("Cancel").clicked() {
                        self.show_drive_modal = false;
                    }
                });
        }

        // 6. DEBUG LOGS MODAL
        if self.show_logs_modal {
            egui::Window::new("Debug Logs")
                .collapsible(false)
                .default_size([520.0, 320.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Clear Logs").clicked() {
                            debug::clear_logs();
                        }
                        if ui.button("Close").clicked() {
                            self.show_logs_modal = false;
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for line in debug::get_logs() {
                            ui.label(RichText::new(line).color(text_secondary).monospace().small());
                        }
                    });
                });
        }
    }
}

fn open_item(path: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

fn reveal_in_file_manager(path: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .args(["/select,", path])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .args(["-R", path])
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(parent) = Path::new(path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}