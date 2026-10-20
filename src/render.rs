//! Turns layout items into one vertex-coloured mesh (cheap to draw, cached between frames)
//! plus a list of label shapes.

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, pos2, vec2};

use crate::layout::{Item, ItemKind};
use crate::theme::{self, shade};
use crate::tree::{NONE, NodeId, Tree, flags};
use crate::util::format_size;

/// Maps the layout rectangle `from` onto `to` (used by the zoom animation).
#[derive(Clone, Copy)]
pub struct Xform {
    from: Rect,
    to: Rect,
}

impl Xform {
    pub fn identity(r: Rect) -> Self {
        Self { from: r, to: r }
    }
    pub fn new(from: Rect, to: Rect) -> Self {
        Self { from, to }
    }
    fn point(&self, p: Pos2) -> Pos2 {
        let sx = self.to.width() / self.from.width().max(1e-3);
        let sy = self.to.height() / self.from.height().max(1e-3);
        pos2(self.to.min.x + (p.x - self.from.min.x) * sx, self.to.min.y + (p.y - self.from.min.y) * sy)
    }
    pub fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.point(r.min), self.point(r.max))
    }
}

fn quad(mesh: &mut Mesh, r: Rect, top: Color32, bottom: Color32) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(r.left_top(), top);
    mesh.colored_vertex(r.right_top(), top);
    mesh.colored_vertex(r.right_bottom(), bottom);
    mesh.colored_vertex(r.left_bottom(), bottom);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}

/// Fill colour of an item (before shading).
pub fn base_color(tree: &Tree, it: &Item) -> Color32 {
    match it.kind {
        ItemKind::File => theme::CLASS_COLORS[tree.get(it.id).class as usize],
        ItemKind::Dir => {
            let n = tree.get(it.id);
            let base = shade(theme::DIR_BASE, 1.0 + 0.07 * (it.depth.min(6) as f32));
            if n.flags & flags::DENIED != 0 {
                theme::mix(base, Color32::from_rgb(0x8a, 0x3a, 0x3a), 0.5)
            } else {
                base
            }
        }
        ItemKind::Free => theme::FREE,
        ItemKind::Unknown => theme::UNKNOWN,
        ItemKind::Group => theme::GROUP,
    }
}

/// Brightness boost for the clicked (expanded) folder and everything inside it.
const SELECTED_BOOST: f32 = 1.13;

/// `selected`: the expanded folder; it and its descendants are drawn brighter.
pub fn build_mesh(tree: &Tree, items: &[Item], xf: Xform, header_h: f32, selected: NodeId) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.reserve_vertices(items.len() * 8);
    mesh.reserve_triangles(items.len() * 4);
    // Items are stored parents-first, so a folder's descendants directly follow it
    // and are exactly the following items with a greater depth.
    let mut boost_depth: Option<u16> = None;
    for it in items {
        match boost_depth {
            Some(d) if it.depth <= d => boost_depth = None,
            _ => {}
        }
        if boost_depth.is_none() && it.kind == ItemKind::Dir && it.id == selected && selected != NONE {
            boost_depth = Some(it.depth);
        }
        let factor = if boost_depth.is_some() { SELECTED_BOOST } else { 1.0 };
        let r = xf.rect(it.rect);
        if r.width() < 0.5 || r.height() < 0.5 {
            continue;
        }
        let base = shade(base_color(tree, it), factor);
        // Flat style: no outlines; a 1px gap on the right/bottom lets the parent's colour
        // show through as a hairline separator.
        let fill = if it.depth == 0 || r.width() < 3.0 || r.height() < 3.0 {
            r
        } else {
            Rect::from_min_max(r.min, r.max - vec2(1.0, 1.0))
        };
        quad(&mut mesh, fill, base, base);
        if it.kind == ItemKind::Dir && it.header {
            let h = header_h * r.height() / it.rect.height().max(1e-3);
            let hr = Rect::from_min_size(fill.min, vec2(fill.width(), h.min(fill.height())));
            let scanning = tree.get(it.id).flags & flags::SCANNING != 0;
            let head = if scanning { theme::mix(shade(base, 0.8), theme::ACCENT, 0.3) } else { shade(base, 0.8) };
            quad(&mut mesh, hr, head, head);
        }
    }
    mesh
}

fn label(painter: &Painter, text: String, size: f32, color: Color32, max_w: f32) -> std::sync::Arc<eframe::egui::Galley> {
    let mut job = LayoutJob::single_section(text, TextFormat { font_id: FontId::proportional(size), color, ..Default::default() });
    job.wrap = TextWrapping::truncate_at_width(max_w);
    painter.layout_job(job)
}

pub fn build_labels(painter: &Painter, tree: &Tree, items: &[Item], header_h: f32) -> Vec<Shape> {
    const MAX_LABELS: usize = 5000;
    let mut out = Vec::new();
    for it in items {
        if out.len() >= MAX_LABELS {
            break;
        }
        let r = it.rect;
        match it.kind {
            ItemKind::Dir if it.header => {
                let w = r.width() - 10.0;
                if w < 20.0 {
                    continue;
                }
                let n = tree.get(it.id);
                let name = if it.depth == 0 { tree.path(it.id) } else { n.name.to_string() };
                let text = format!("{name}   {}", format_size(it.size));
                let g = label(painter, text, 12.0, theme::TEXT, w);
                let y = r.min.y + 1.0 + (header_h - g.size().y) * 0.5;
                out.push(Shape::galley(pos2(r.min.x + 5.0, y), g, theme::TEXT));
            }
            ItemKind::File | ItemKind::Free | ItemKind::Unknown | ItemKind::Group => {
                let (w, h) = (r.width() - 8.0, r.height());
                if w < 30.0 || h < 16.0 {
                    continue;
                }
                let base = base_color(tree, it);
                let color = theme::text_on(shade(base, 1.05));
                let name = match it.kind {
                    ItemKind::File => tree.get(it.id).name.to_string(),
                    ItemKind::Free => "Free space".to_string(),
                    ItemKind::Unknown => "Unknown space".to_string(),
                    _ => format!("{} items", it.count),
                };
                let g1 = label(painter, name, 11.5, color, w);
                let two = h >= 34.0;
                let total_h = g1.size().y * if two { 2.0 } else { 1.0 };
                let mut y = r.center().y - total_h * 0.5;
                let x1 = r.center().x - g1.size().x * 0.5;
                let line_h = g1.size().y;
                out.push(Shape::galley(pos2(x1, y), g1, color));
                if two {
                    y += line_h;
                    let g2 = label(painter, format_size(it.size), 10.5, theme::mix(color, base, 0.25), w);
                    out.push(Shape::galley(pos2(r.center().x - g2.size().x * 0.5, y), g2, color));
                }
            }
            _ => {}
        }
    }
    out
}
