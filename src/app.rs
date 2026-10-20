use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, Key, Layout, Mesh, PointerButton, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, vec2,
};
use parking_lot::Mutex;

use crate::layout::{self, Item, ItemKind, Params};
use crate::render::{self, Xform};
use crate::scan::{self, Scan};
use crate::theme;
use crate::tree::{Kind, NONE, NodeId, Tree, flags};
use crate::util::{self, DriveInfo, format_count, format_size};

const ZOOM_TIME: f32 = 0.28;
const WARN: Color32 = Color32::from_rgb(0xe0, 0x8a, 0x6a);
const RELAYOUT_INTERVAL: Duration = Duration::from_millis(250);

/// How the zoom animation should start once the new layout exists.
#[derive(Clone, Copy)]
enum PendingAnim {
    /// Zooming into a child that occupied this rectangle in the old layout.
    In(Rect),
    /// Zooming out: locate the previous focus in the new layout.
    Out(NodeId),
}

struct Anim {
    start: Instant,
    /// Rectangle (view coordinates) the new layout is mapped into at t = 0.
    from: Rect,
}

struct ViewCache {
    items: Vec<Item>,
    mesh: Arc<Mesh>,
    labels: Vec<Shape>,
    rect: Rect,
    focus: NodeId,
    selected: NodeId,
    version: u64,
    params: Option<Params>,
    built: Option<Instant>,
}

impl Default for ViewCache {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            mesh: Arc::default(),
            labels: Vec::new(),
            rect: Rect::NOTHING,
            focus: NONE,
            selected: NONE,
            version: 0,
            params: None,
            built: None,
        }
    }
}

enum OpResult {
    Recycled(NodeId, String),
    Failed(String),
}

pub struct App {
    drives: Vec<DriveInfo>,
    path_input: String,
    scan: Option<Arc<Scan>>,
    focus: NodeId,
    /// Clicked box: its children are shown one level beyond `max_depth`.
    selected: NodeId,
    /// History entries: (focus, expanded folder) so going back restores the expansion.
    back: Vec<(NodeId, NodeId)>,
    forward: Vec<(NodeId, NodeId)>,
    params: Params,
    cache: ViewCache,
    pending_anim: Option<PendingAnim>,
    anim: Option<Anim>,
    hovered: Option<usize>,
    menu_target: Option<Item>,
    ops: Arc<Mutex<Vec<OpResult>>>,
    toast: Option<(String, Instant)>,
    show_failures: bool,
    elevated: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<String>) -> Self {
        theme::install(&cc.egui_ctx);
        cc.egui_ctx.global_style_mut(|s| s.interaction.tooltip_delay = 0.2);
        let mut app = Self {
            drives: util::list_drives(),
            path_input: String::new(),
            scan: None,
            focus: 0,
            selected: NONE,
            back: Vec::new(),
            forward: Vec::new(),
            params: Params::default(),
            cache: ViewCache::default(),
            pending_anim: None,
            anim: None,
            hovered: None,
            menu_target: None,
            ops: Arc::default(),
            toast: None,
            show_failures: false,
            elevated: util::is_elevated(),
        };
        if let Some(p) = initial {
            app.start_scan(p);
        }
        app
    }

    // ---------------------------------------------------------------- actions

    fn start_scan(&mut self, raw: String) {
        let mut path = raw.trim().trim_matches('"').to_string();
        if util::is_drive_root(&path) {
            path = format!("{}:\\", path[..1].to_ascii_uppercase());
        } else {
            while path.len() > 3 && path.ends_with('\\') {
                path.pop();
            }
        }
        if !std::path::Path::new(&path).is_dir() {
            self.notify(format!("Not a folder: {path}"));
            return;
        }
        self.stop_scan();
        let root = util::drive_root(&path);
        let cluster = root.as_deref().map(util::cluster_size).unwrap_or(4096);
        let volume = if util::is_drive_root(&path) { root.as_deref().and_then(util::volume) } else { None };
        let s = Scan::new(Tree::new(path, cluster, volume));
        scan::spawn(s.clone(), 0);
        self.scan = Some(s);
        self.focus = 0;
        self.selected = NONE;
        self.back.clear();
        self.forward.clear();
        self.cache = ViewCache::default();
        self.pending_anim = None;
        self.anim = None;
        self.hovered = None;
    }

    fn stop_scan(&self) {
        if let Some(s) = &self.scan {
            s.cancel.store(true, Ordering::Release);
        }
    }

    fn close_scan(&mut self) {
        self.stop_scan();
        self.scan = None;
        self.drives = util::list_drives();
    }

    fn rescan(&mut self, id: NodeId) {
        let Some(s) = self.scan.clone() else { return };
        if s.is_running() {
            self.notify("Wait for the current scan to finish".into());
            return;
        }
        if id == s.tree.read().root {
            let path = s.tree.read().root_path.clone();
            self.start_scan(path);
            return;
        }
        s.tree.write().clear_dir(id);
        s.bump();
        scan::spawn(s, id);
    }

    fn notify(&mut self, msg: String) {
        self.toast = Some((msg, Instant::now()));
    }

    /// Moves the view to `target`; `select` is the folder to show expanded there.
    fn navigate(&mut self, target: NodeId, record: bool, select: NodeId) {
        if target == self.focus {
            return;
        }
        self.pending_anim = self.zoom_start(target);
        if record {
            self.back.push((self.focus, self.selected));
            self.forward.clear();
        }
        self.focus = target;
        self.selected = select;
    }

    /// Going to an ancestor highlights the folder we came from.
    fn navigate_out(&mut self, ancestor: NodeId) {
        let from = self.focus;
        self.navigate(ancestor, true, from);
    }

    fn zoom_start(&self, target: NodeId) -> Option<PendingAnim> {
        let tree = self.scan.as_ref()?.tree.read();
        if tree.lineage(target).contains(&self.focus) {
            self.cache.items.iter().find(|i| i.id == target && i.kind == ItemKind::Dir).map(|i| PendingAnim::In(i.rect))
        } else if tree.lineage(self.focus).contains(&target) {
            Some(PendingAnim::Out(self.focus))
        } else {
            None
        }
    }

    fn go_back(&mut self) {
        if let Some((f, sel)) = self.back.pop() {
            self.forward.push((self.focus, self.selected));
            self.navigate(f, false, sel);
        }
    }

    fn go_forward(&mut self) {
        if let Some((f, sel)) = self.forward.pop() {
            self.back.push((self.focus, self.selected));
            self.navigate(f, false, sel);
        }
    }

    fn go_up(&mut self) {
        let parent = self.scan.as_ref().map(|s| s.tree.read().get(self.focus).parent).unwrap_or(NONE);
        if parent != NONE {
            self.navigate_out(parent);
        }
    }

    fn recycle(&mut self, id: NodeId) {
        let Some(s) = &self.scan else { return };
        let path = s.tree.read().path(id);
        let confirm = rfd::MessageDialog::new()
            .set_title("Move to Recycle Bin")
            .set_description(format!("Move this item to the Recycle Bin?\n\n{path}"))
            .set_buttons(rfd::MessageButtons::YesNo)
            .set_level(rfd::MessageLevel::Warning)
            .show();
        if confirm != rfd::MessageDialogResult::Yes {
            return;
        }
        let ops = self.ops.clone();
        std::thread::spawn(move || {
            let r = match trash::delete(&path) {
                Ok(()) => OpResult::Recycled(id, path),
                Err(e) => OpResult::Failed(format!("Could not recycle {path}: {e}")),
            };
            ops.lock().push(r);
        });
    }

    fn drain_ops(&mut self) {
        let done: Vec<_> = self.ops.lock().drain(..).collect();
        for op in done {
            match op {
                OpResult::Recycled(id, path) => {
                    if let Some(s) = &self.scan {
                        let mut t = s.tree.write();
                        if t.is_alive(id) {
                            if t.lineage(self.focus).contains(&id) {
                                self.focus = t.get(id).parent;
                            }
                            t.remove(id);
                        }
                        drop(t);
                        s.bump();
                    }
                    self.notify(format!("Moved to Recycle Bin: {path}"));
                }
                OpResult::Failed(msg) => self.notify(msg),
            }
        }
    }

    // ---------------------------------------------------------------- top bar

    fn toolbar(&mut self, ui: &mut Ui) {
        let Some(s) = self.scan.clone() else { return };
        ui.horizontal_centered(|ui| {
            if ui.add_enabled(!self.back.is_empty(), egui::Button::new("←")).on_hover_text("Back (Backspace / Mouse back)").clicked() {
                self.go_back();
            }
            if ui.add_enabled(!self.forward.is_empty(), egui::Button::new("→")).on_hover_text("Forward").clicked() {
                self.go_forward();
            }
            let root = s.tree.read().root;
            if ui.add_enabled(self.focus != root, egui::Button::new("↑")).on_hover_text("Up one level (Alt+Up)").clicked() {
                self.go_up();
            }
            ui.add_space(6.0);

            // Breadcrumbs.
            let crumbs: Vec<(NodeId, String)> = {
                let t = s.tree.read();
                t.lineage(self.focus).into_iter().map(|id| (id, if id == t.root { t.root_path.clone() } else { t.get(id).name.to_string() })).collect()
            };
            let mut nav = None;
            let last = crumbs.len().saturating_sub(1);
            for (i, (id, name)) in crumbs.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("›").color(theme::TEXT_DIM));
                }
                let text = if i == last { RichText::new(name).strong().color(theme::TEXT) } else { RichText::new(name).color(theme::TEXT_DIM) };
                if ui.add(egui::Button::new(text).frame(false)).clicked() && i != last {
                    nav = Some(*id);
                }
            }
            if let Some(id) = nav {
                self.navigate_out(id);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("New scan").clicked() {
                    self.close_scan();
                    return;
                }
                if s.is_running() {
                    if ui.button("Stop").clicked() {
                        self.stop_scan();
                    }
                } else if ui.button("Rescan").on_hover_text("Rescan the current folder").clicked() {
                    let f = self.focus;
                    self.rescan(f);
                }
                ui.separator();
                let has_volume = s.tree.read().volume.is_some();
                if has_volume {
                    ui.toggle_value(&mut self.params.show_free, "Free space");
                }
                ui.separator();
                if ui.add_enabled(self.params.max_depth < 32, egui::Button::new("+")).on_hover_text("More detail").clicked() {
                    self.params.max_depth += 1;
                }
                ui.label(RichText::new(format!("Depth {}", self.params.max_depth)).color(theme::TEXT_DIM));
                if ui.add_enabled(self.params.max_depth > 1, egui::Button::new("−")).on_hover_text("Less detail").clicked() {
                    self.params.max_depth -= 1;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if let Some(s) = self.scan.clone() {
                let t = s.tree.read();
                let root = t.get(t.root);
                let files = s.files.load(Ordering::Relaxed);
                let errors = s.failures(&t).len() as u64;
                let elapsed = s.finished.read().unwrap_or_else(Instant::now) - *s.started.read();
                if s.is_running() {
                    ui.spinner();
                    ui.label(format!(
                        "Scanning…  {} files  ·  {}  ·  {:.1} s",
                        format_count(files),
                        format_size(root.disk),
                        elapsed.as_secs_f32()
                    ));
                    if let Some(v) = t.volume {
                        let used = v.total.saturating_sub(v.free).max(1);
                        let p = (root.disk as f32 / used as f32).clamp(0.0, 1.0);
                        ui.add(egui::ProgressBar::new(p).desired_width(160.0).desired_height(10.0).show_percentage());
                    }
                } else {
                    ui.label(format!(
                        "{} files  ·  {} folders  ·  {} on disk  ·  {} logical  ·  {:.1} s",
                        format_count(root.files as u64),
                        format_count(root.dirs as u64),
                        format_size(root.disk),
                        format_size(root.size),
                        elapsed.as_secs_f32()
                    ));
                }
                if errors > 0 {
                    let noun = if errors == 1 { "folder" } else { "folders" };
                    let text = RichText::new(format!("·  {} {noun} not accessible", format_count(errors))).color(WARN).underline();
                    if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Click for details").clicked() {
                        self.show_failures = true;
                    }
                }
                drop(t);
                if let Some(i) = self.hovered.and_then(|i| self.cache.items.get(i)).copied() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let t = s.tree.read();
                        if t.is_alive(i.id) {
                            ui.label(RichText::new(t.path(i.id)).color(theme::TEXT_DIM));
                        }
                    });
                }
            }
            if let Some((msg, at)) = &self.toast {
                if at.elapsed() < Duration::from_secs(5) {
                    ui.label(RichText::new(msg).color(theme::ACCENT));
                } else {
                    self.toast = None;
                }
            }
        });
    }

    // ---------------------------------------------------------------- start screen

    fn start_screen(&mut self, ui: &mut Ui) {
        let mut chosen = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(36.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("DiskSpaceMap").size(30.0).strong().color(theme::TEXT));
                ui.label(RichText::new("Pick a drive or a folder to see where your space went").color(theme::TEXT_DIM));
            });
            ui.add_space(24.0);

            let card_w = 260.0;
            let cols = ((ui.available_width() - 40.0) / (card_w + 12.0)).floor().max(1.0) as usize;
            let total_w = cols as f32 * (card_w + 12.0) - 12.0;
            let side = ((ui.available_width() - total_w) * 0.5).max(0.0);
            for row in self.drives.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    for d in row {
                        if drive_card(ui, d, card_w).clicked() {
                            chosen = Some(d.root.clone());
                        }
                        ui.add_space(4.0);
                    }
                });
                ui.add_space(8.0);
            }

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(side);
                let w = (total_w - 190.0).max(200.0);
                let resp = ui.add(egui::TextEdit::singleline(&mut self.path_input).hint_text("Folder path, e.g. D:\\Projects").desired_width(w));
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Browse…").clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_folder() {
                        chosen = Some(p.display().to_string());
                    }
                if ui.add_enabled(!self.path_input.trim().is_empty(), egui::Button::new("Scan")).clicked() || enter {
                    chosen = Some(self.path_input.clone());
                }
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Tip: drop a folder onto the window to scan it").size(12.0).color(theme::TEXT_DIM));
            });
        });
        if let Some(p) = chosen {
            self.start_scan(p);
        }
    }

    // ---------------------------------------------------------------- treemap

    fn treemap(&mut self, ui: &mut Ui) {
        let Some(s) = self.scan.clone() else { return };
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        let rect = rect.shrink(4.0);
        if !s.tree.read().is_alive(self.focus) {
            self.focus = s.tree.read().root;
        }

        // Relayout when the view, focus or parameters change, or (throttled) when the tree changes.
        let version = s.version.load(Ordering::Acquire);
        let c = &self.cache;
        let structural =
            c.rect != rect || c.focus != self.focus || c.selected != self.selected || c.params != Some(self.params);
        let data_changed = c.version != version
            && (c.built.is_none_or(|b| b.elapsed() >= RELAYOUT_INTERVAL) || !s.is_running());
        if structural || (data_changed && self.anim.is_none()) {
            let tree = s.tree.read();
            if self.selected != NONE && !tree.is_alive(self.selected) {
                self.selected = NONE;
            }
            let expand = if self.selected == NONE { Vec::new() } else { tree.lineage(self.selected) };
            let items = layout::build(&tree, self.focus, rect, &self.params, &expand);
            let painter = ui.painter();
            self.cache.labels = render::build_labels(painter, &tree, &items, self.params.header_h);
            self.cache.mesh = Arc::new(render::build_mesh(&tree, &items, Xform::identity(rect), self.params.header_h, self.selected));
            self.cache.items = items;
            self.cache.rect = rect;
            self.cache.focus = self.focus;
            self.cache.selected = self.selected;
            self.cache.version = version;
            self.cache.params = Some(self.params);
            self.cache.built = Some(Instant::now());
            self.hovered = None;
            if let Some(p) = self.pending_anim.take() {
                let from = match p {
                    PendingAnim::In(r) => Some(r),
                    PendingAnim::Out(old) => self.cache.items.iter().find(|i| i.id == old && i.kind == ItemKind::Dir).map(|i| {
                        // Expand so that the old focus' rectangle fills the view, then shrink back.
                        Xform::new(i.rect, rect).rect(rect)
                    }),
                };
                self.anim = from.filter(|r| r.width() > 1.0 && r.height() > 1.0).map(|from| Anim { start: Instant::now(), from });
            }
        }
        if s.is_running() || self.cache.version != version {
            ui.ctx().request_repaint_after(RELAYOUT_INTERVAL);
        }

        let painter = ui.painter_at(rect.expand(4.0));
        painter.rect_filled(rect.expand(4.0), CornerRadius::ZERO, theme::BG);

        // Zoom animation: redraw the mesh through an interpolated transform, no labels.
        if let Some(a) = &self.anim {
            let t = (a.start.elapsed().as_secs_f32() / ZOOM_TIME).min(1.0);
            let e = ease(t);
            let target = lerp_rect(a.from, rect, e);
            let tree = s.tree.read();
            let mesh = render::build_mesh(&tree, &self.cache.items, Xform::new(rect, target), self.params.header_h, self.selected);
            painter.add(Shape::mesh(mesh));
            if t >= 1.0 {
                self.anim = None;
            }
            ui.ctx().request_repaint();
            return;
        }

        painter.add(Shape::Mesh(self.cache.mesh.clone()));
        painter.extend(self.cache.labels.iter().cloned());

        // Hover.
        let pos = resp.hover_pos();
        self.hovered = pos.and_then(|p| layout::hit(&self.cache.items, p));
        if self.selected != NONE
            && let Some(sel) = self.cache.items.iter().find(|i| i.id == self.selected && i.kind == ItemKind::Dir)
        {
            painter.rect_stroke(sel.rect, CornerRadius::ZERO, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.8)), StrokeKind::Inside);
        }
        if let Some(it) = self.hovered.map(|i| self.cache.items[i]) {
            painter.rect_stroke(it.rect, CornerRadius::ZERO, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
            if let Some(parent) = self.cache.items.iter().rev().find(|p| p.kind == ItemKind::Dir && p.rect.contains_rect(it.rect) && p.id != it.id) {
                painter.rect_stroke(parent.rect, CornerRadius::ZERO, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.45)), StrokeKind::Inside);
            }
            if !resp.context_menu_opened() {
                let s2 = s.clone();
                resp.clone().on_hover_ui_at_pointer(|ui| tooltip(ui, &s2.tree.read(), &it));
            }
        }

        // Clicks.
        if resp.clicked()
            && let Some(it) = self.hovered.map(|i| self.cache.items[i])
        {
            let tree = s.tree.read();
            let target = match it.kind {
                ItemKind::Dir => it.id,
                ItemKind::File | ItemKind::Group => tree.get(it.id).parent,
                ItemKind::Free | ItemKind::Unknown => NONE,
            };
            drop(tree);
            self.selected = if target == self.focus { NONE } else { target };
        }
        if resp.double_clicked()
            && let Some(it) = self.hovered.map(|i| self.cache.items[i]) {
                let tree = s.tree.read();
                let target = match it.kind {
                    ItemKind::Dir => Some(it.id),
                    ItemKind::File | ItemKind::Group => Some(tree.get(it.id).parent),
                    _ => None,
                };
                drop(tree);
                if let Some(t) = target.filter(|&t| t != NONE) {
                    self.navigate(t, true, NONE);
                }
            }
        if resp.secondary_clicked() {
            self.menu_target = self.hovered.map(|i| self.cache.items[i]);
        }
        let mut action = None;
        resp.context_menu(|ui| {
            if let Some(it) = self.menu_target {
                action = context_menu(ui, &s.tree.read(), &it, s.is_running());
            }
        });
        if let Some(a) = action {
            self.run(a);
        }
    }

    fn failures_window(&mut self, ctx: &egui::Context) {
        let Some(s) = self.scan.clone() else {
            self.show_failures = false;
            return;
        };
        if !self.show_failures {
            return;
        }
        let rows: Vec<(NodeId, String, String)> = {
            let t = s.tree.read();
            s.failures(&t).into_iter().map(|(id, hr)| (id, t.path(id), util::error_text(hr))).collect()
        };
        let mut open = true;
        let mut action = None;
        egui::Window::new(format!("Inaccessible folders ({})", rows.len()))
            .id(egui::Id::unique("failures"))
            .open(&mut open)
            .default_size([760.0, 420.0])
            .min_width(480.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Copy all paths").clicked() {
                        let all = rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>().join("\r\n");
                        ui.ctx().copy_text(all);
                    }
                    if self.elevated {
                        ui.label(RichText::new("Already running as administrator").color(theme::TEXT_DIM));
                    } else if ui
                        .button("Restart as administrator")
                        .on_hover_text("Relaunch elevated and scan the same path again (current results are discarded)")
                        .clicked()
                    {
                        action = Some(FailAction::Elevate);
                    }
                });
                if !self.elevated {
                    ui.label(RichText::new("Protected system and user folders can only be scanned with administrator rights.").color(theme::TEXT_DIM));
                }
                ui.separator();
                if rows.is_empty() {
                    ui.label("No inaccessible folders.");
                    return;
                }
                let row_h = 22.0;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
                    for (id, path, reason) in &rows[range] {
                        ui.horizontal(|ui| {
                            ui.set_min_height(row_h);
                            if ui.small_button("Locate").on_hover_text("Show it in the map").clicked() {
                                action = Some(FailAction::Locate(*id));
                            }
                            if ui.small_button("Explorer").clicked() {
                                util::reveal_in_explorer(path);
                            }
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(path.clone());
                            }
                            ui.label(RichText::new(reason).color(WARN));
                            ui.add(egui::Label::new(path.as_str()).truncate()).on_hover_text(path.as_str());
                        });
                    }
                });
            });
        self.show_failures = open;
        match action {
            Some(FailAction::Locate(id)) => {
                let parent = s.tree.read().get(id).parent;
                if parent == NONE || parent == self.focus {
                    self.selected = id;
                } else {
                    self.navigate(parent, true, id);
                }
            }
            Some(FailAction::Elevate) => {
                let path = s.tree.read().root_path.clone();
                if util::relaunch_as_admin(&path) {
                    self.stop_scan();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.notify("Elevation was cancelled".into());
                }
            }
            None => {}
        }
    }

    fn run(&mut self, a: MenuAction) {
        let Some(s) = self.scan.clone() else { return };
        let path = |id| s.tree.read().path(id);
        match a {
            MenuAction::Zoom(id) => self.navigate(id, true, NONE),
            MenuAction::Open(id) => util::shell_open(&path(id)),
            MenuAction::Reveal(id) => util::reveal_in_explorer(&path(id)),
            MenuAction::CopyPath(id, ctx) => ctx.copy_text(path(id)),
            MenuAction::Rescan(id) => self.rescan(id),
            MenuAction::Recycle(id) => self.recycle(id),
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.scan.is_none() {
            return;
        }
        let (back, fwd, up) = ctx.input(|i| {
            (
                i.pointer.button_pressed(PointerButton::Extra1) || (i.key_pressed(Key::Backspace) && !ctx.egui_wants_keyboard_input()) || (i.modifiers.alt && i.key_pressed(Key::ArrowLeft)),
                i.pointer.button_pressed(PointerButton::Extra2) || (i.modifiers.alt && i.key_pressed(Key::ArrowRight)),
                i.modifiers.alt && i.key_pressed(Key::ArrowUp),
            )
        });
        if back {
            self.go_back();
        }
        if fwd {
            self.go_forward();
        }
        if up {
            self.go_up();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_ops();
        self.handle_keys(&ctx);
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().display().to_string()));
        if let Some(p) = dropped.filter(|p| !p.is_empty()) {
            self.start_scan(p);
        }

        let bar = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 6));
        if self.scan.is_some() {
            egui::Panel::top("toolbar").frame(bar).exact_size(40.0).show(ui, |ui| self.toolbar(ui));
        }
        egui::Panel::bottom("status").frame(bar).exact_size(30.0).show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ui, |ui| {
            if self.scan.is_some() {
                self.treemap(ui);
            } else {
                self.start_screen(ui);
            }
        });
        self.failures_window(&ctx);
        if self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

// -------------------------------------------------------------------- helpers

enum FailAction {
    Locate(NodeId),
    Elevate,
}

enum MenuAction {
    Zoom(NodeId),
    Open(NodeId),
    Reveal(NodeId),
    CopyPath(NodeId, egui::Context),
    Rescan(NodeId),
    Recycle(NodeId),
}

fn context_menu(ui: &mut Ui, tree: &Tree, it: &Item, scanning: bool) -> Option<MenuAction> {
    if !tree.is_alive(it.id) {
        return None;
    }
    let (id, is_dir) = match it.kind {
        ItemKind::Dir => (it.id, true),
        ItemKind::File => (it.id, false),
        ItemKind::Group | ItemKind::Free | ItemKind::Unknown => {
            let id = if it.kind == ItemKind::Group { tree.get(it.id).parent } else { it.id };
            (id, true)
        }
    };
    let name = if id == tree.root { tree.root_path.clone() } else { tree.get(id).name.to_string() };
    ui.label(RichText::new(name).strong());
    ui.separator();
    let mut a = None;
    if is_dir && ui.button("Zoom in").clicked() {
        a = Some(MenuAction::Zoom(id));
    }
    if ui.button("Open").clicked() {
        a = Some(MenuAction::Open(id));
    }
    if ui.button("Show in Explorer").clicked() {
        a = Some(MenuAction::Reveal(id));
    }
    if ui.button("Copy path").clicked() {
        a = Some(MenuAction::CopyPath(id, ui.ctx().clone()));
    }
    if is_dir && ui.add_enabled(!scanning, egui::Button::new("Rescan folder")).clicked() {
        a = Some(MenuAction::Rescan(id));
    }
    ui.separator();
    if id != tree.root && ui.button(RichText::new("Move to Recycle Bin…").color(Color32::from_rgb(0xe8, 0x86, 0x76))).clicked() {
        a = Some(MenuAction::Recycle(id));
    }
    if a.is_some() {
        ui.close();
    }
    a
}

fn tooltip(ui: &mut Ui, tree: &Tree, it: &Item) {
    if !tree.is_alive(it.id) {
        return;
    }
    let pct = |part: u64, whole: u64| if whole > 0 { part as f64 * 100.0 / whole as f64 } else { 0.0 };
    match it.kind {
        ItemKind::Free | ItemKind::Unknown => {
            let title = if it.kind == ItemKind::Free { "Free space" } else { "Unknown space" };
            ui.label(RichText::new(title).strong());
            ui.label(format_size(it.size));
            if let Some(v) = tree.volume {
                ui.label(RichText::new(format!("{:.1}% of the volume", pct(it.size, v.total))).color(theme::TEXT_DIM));
            }
            if it.kind == ItemKind::Unknown {
                ui.label(RichText::new("Not scanned yet, inaccessible, or used by the system").color(theme::TEXT_DIM));
            }
        }
        ItemKind::Group => {
            ui.label(RichText::new(format!("{} small items", it.count)).strong());
            ui.label(format_size(it.size));
            ui.label(RichText::new("Zoom in to see them").color(theme::TEXT_DIM));
        }
        ItemKind::Dir | ItemKind::File => {
            let n = tree.get(it.id);
            let name = if it.id == tree.root { tree.root_path.clone() } else { n.name.to_string() };
            ui.label(RichText::new(name).strong());
            egui::Grid::new("tt").num_columns(2).spacing(vec2(12.0, 2.0)).show(ui, |ui| {
                ui.label(RichText::new("On disk").color(theme::TEXT_DIM));
                ui.label(format_size(n.disk));
                ui.end_row();
                ui.label(RichText::new("Size").color(theme::TEXT_DIM));
                ui.label(format_size(n.size));
                ui.end_row();
                if n.parent != NONE {
                    ui.label(RichText::new("Of parent").color(theme::TEXT_DIM));
                    ui.label(format!("{:.1}%", pct(n.disk, tree.get(n.parent).disk)));
                    ui.end_row();
                }
                if n.kind == Kind::Dir {
                    ui.label(RichText::new("Contains").color(theme::TEXT_DIM));
                    ui.label(format!("{} files, {} folders", format_count(n.files as u64), format_count(n.dirs as u64)));
                    ui.end_row();
                } else {
                    ui.label(RichText::new("Type").color(theme::TEXT_DIM));
                    ui.label(theme::CLASS_NAMES[n.class as usize]);
                    ui.end_row();
                }
                if n.mtime != 0 {
                    ui.label(RichText::new("Modified").color(theme::TEXT_DIM));
                    ui.label(util::format_filetime(n.mtime));
                    ui.end_row();
                }
            });
            if n.flags & flags::DENIED != 0 {
                ui.label(RichText::new("Access denied").color(Color32::from_rgb(0xe0, 0x8a, 0x6a)));
            } else if n.flags & flags::SCANNING != 0 {
                ui.label(RichText::new("Scanning…").color(theme::ACCENT));
            }
        }
    }
}

fn drive_card(ui: &mut Ui, d: &DriveInfo, w: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 92.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    let bg = if hovered { Color32::from_rgb(0x2a, 0x30, 0x3b) } else { Color32::from_rgb(0x22, 0x26, 0x2e) };
    p.rect_filled(rect, 10, bg);
    if hovered {
        p.rect_stroke(rect, 10, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.6)), StrokeKind::Inside);
    }
    let inner = rect.shrink2(vec2(14.0, 12.0));
    let title = if d.label.is_empty() { format!("{} ({})", d.kind, &d.root[..2]) } else { format!("{} ({})", d.label, &d.root[..2]) };
    p.text(inner.left_top(), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(15.0), theme::TEXT);
    p.text(inner.right_top(), egui::Align2::RIGHT_TOP, &d.fs, egui::FontId::proportional(11.0), theme::TEXT_DIM);
    if let Some(v) = d.volume {
        let used = v.total.saturating_sub(v.free);
        let frac = if v.total > 0 { used as f32 / v.total as f32 } else { 0.0 };
        let bar = Rect::from_min_size(inner.left_top() + vec2(0.0, 30.0), vec2(inner.width(), 8.0));
        p.rect_filled(bar, 4, Color32::from_rgb(0x33, 0x38, 0x43));
        let fill = if frac > 0.9 { Color32::from_rgb(0xd8, 0x5c, 0x4a) } else { theme::ACCENT };
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), 4, fill);
        p.text(
            inner.left_bottom(),
            egui::Align2::LEFT_BOTTOM,
            format!("{} free of {}", format_size(v.free), format_size(v.total)),
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
    } else {
        p.text(inner.left_bottom(), egui::Align2::LEFT_BOTTOM, "Not ready", egui::FontId::proportional(12.0), theme::TEXT_DIM);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn ease(t: f32) -> f32 {
    // Cubic ease-in-out.
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min + (b.min - a.min) * t, a.max + (b.max - a.max) * t)
}
