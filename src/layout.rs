//! Treemap layout: ordered binary split (the SpaceSniffer algorithm). Children are sorted by
//! size, accumulated until half of the total, and the rectangle is cut along its longer side.

use eframe::egui::{Pos2, Rect, pos2};

use crate::tree::{Kind, NodeId, Tree};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Dir,
    File,
    Free,
    Unknown,
    /// Several children too small to draw individually.
    Group,
}

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub rect: Rect,
    /// Node for Dir/File; first member for Group; the container for Free/Unknown.
    pub id: NodeId,
    pub kind: ItemKind,
    pub depth: u16,
    pub size: u64,
    /// Members in a Group.
    pub count: u32,
    /// Header strip for directories that have room for a label.
    pub header: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Params {
    pub min_px: f32,
    pub max_depth: u16,
    pub header_h: f32,
    pub pad: f32,
    pub show_free: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self { min_px: 5.0, max_depth: 2, header_h: 16.0, pad: 1.0, show_free: true }
    }
}

#[derive(Clone, Copy)]
enum Slot {
    Node(NodeId),
    Free,
    Unknown,
}

/// `expand` lists directories whose children are laid out even beyond `max_depth`
/// (the ancestors of the clicked box plus the box itself).
pub fn build(tree: &Tree, focus: NodeId, bounds: Rect, p: &Params, expand: &[NodeId]) -> Vec<Item> {
    let mut out = Vec::with_capacity(4096);
    if bounds.width() < 1.0 || bounds.height() < 1.0 || !tree.is_alive(focus) {
        return out;
    }
    let mut builder = Builder { tree, p, expand, out: &mut out, scratch: Vec::new() };
    builder.container(focus, bounds, 0, focus == tree.root);
    out
}

/// Index of the deepest item under `pos` (items are stored parents-first).
pub fn hit(items: &[Item], pos: Pos2) -> Option<usize> {
    items.iter().rposition(|it| it.rect.contains(pos))
}

struct Builder<'a> {
    tree: &'a Tree,
    p: &'a Params,
    expand: &'a [NodeId],
    out: &'a mut Vec<Item>,
    scratch: Vec<Vec<(u64, Slot)>>,
}

impl Builder<'_> {
    fn container(&mut self, id: NodeId, rect: Rect, depth: u16, with_volume: bool) {
        let node = self.tree.get(id);
        let header = rect.height() >= self.p.header_h * 2.0 && rect.width() >= 28.0;
        let mut size = node.disk;
        let free = if with_volume && self.p.show_free { self.tree.volume.map_or(0, |v| v.free) } else { 0 };
        let unknown = if with_volume && self.p.show_free { self.tree.unknown_space() } else { 0 };
        size += free + unknown;
        self.out.push(Item { rect, id, kind: ItemKind::Dir, depth, size, count: 0, header });

        if depth >= self.p.max_depth && !self.expand.contains(&id) {
            return;
        }
        let mut inner = rect;
        if header {
            inner.min.y += self.p.header_h;
        }
        let inner = inner.shrink(self.p.pad);
        if inner.width() < self.p.min_px || inner.height() < self.p.min_px {
            return;
        }

        let mut slots = self.scratch.pop().unwrap_or_default();
        slots.clear();
        slots.extend(
            node.children.iter().map(|&c| (self.tree.get(c).disk, Slot::Node(c))).filter(|(s, _)| *s > 0),
        );
        if free > 0 {
            slots.push((free, Slot::Free));
        }
        if unknown > 0 {
            slots.push((unknown, Slot::Unknown));
        }
        slots.sort_unstable_by_key(|s| std::cmp::Reverse(s.0));
        let total: u64 = slots.iter().map(|s| s.0).sum();
        if total > 0 {
            self.split(&slots, total, inner, depth + 1, id);
        }
        self.scratch.push(slots);
    }

    fn split(&mut self, slots: &[(u64, Slot)], total: u64, r: Rect, depth: u16, parent: NodeId) {
        if slots.len() == 1 {
            self.leaf(slots[0], r, depth, parent);
            return;
        }
        if r.width() < self.p.min_px || r.height() < self.p.min_px {
            let first = match slots[0].1 {
                Slot::Node(n) => n,
                _ => parent,
            };
            self.out.push(Item {
                rect: r,
                id: first,
                kind: ItemKind::Group,
                depth,
                size: total,
                count: slots.len() as u32,
                header: false,
            });
            return;
        }
        let half = total / 2;
        let mut acc = 0u64;
        let mut k = 0;
        while k < slots.len() - 1 {
            acc += slots[k].0;
            k += 1;
            if acc >= half {
                break;
            }
        }
        let t = acc as f64 / total as f64;
        let (a, b) = if r.width() >= r.height() {
            let x = (r.min.x as f64 + r.width() as f64 * t).round() as f32;
            (Rect::from_min_max(r.min, pos2(x, r.max.y)), Rect::from_min_max(pos2(x, r.min.y), r.max))
        } else {
            let y = (r.min.y as f64 + r.height() as f64 * t).round() as f32;
            (Rect::from_min_max(r.min, pos2(r.max.x, y)), Rect::from_min_max(pos2(r.min.x, y), r.max))
        };
        self.split(&slots[..k], acc, a, depth, parent);
        self.split(&slots[k..], total - acc, b, depth, parent);
    }

    fn leaf(&mut self, (size, slot): (u64, Slot), r: Rect, depth: u16, parent: NodeId) {
        if r.width() < 1.0 || r.height() < 1.0 {
            return;
        }
        match slot {
            Slot::Node(id) => {
                if self.tree.get(id).kind == Kind::Dir {
                    self.container(id, r, depth, false);
                } else {
                    self.out.push(Item { rect: r, id, kind: ItemKind::File, depth, size, count: 1, header: false });
                }
            }
            Slot::Free | Slot::Unknown => {
                let kind = if matches!(slot, Slot::Free) { ItemKind::Free } else { ItemKind::Unknown };
                self.out.push(Item { rect: r, id: parent, kind, depth, size, count: 0, header: false });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Entry, Kind};

    fn tree() -> Tree {
        let mut t = Tree::new("C:\\".into(), 0, None);
        let entries = (1..=20u64)
            .map(|i| Entry { name: format!("f{i}").into(), kind: Kind::File, size: i * 100, disk: i * 100, mtime: 0 })
            .collect();
        t.add_entries(t.root, entries);
        t
    }

    #[test]
    fn leaves_tile_the_inner_area() {
        let t = tree();
        let p = Params { min_px: 1.0, header_h: 0.0, pad: 0.0, ..Default::default() };
        let items = build(&t, t.root, Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0)), &p, &[]);
        let leaves: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::File).collect();
        assert_eq!(leaves.len(), 20);
        let area: f32 = leaves.iter().map(|i| i.rect.area()).sum();
        assert!((area - 120_000.0).abs() < 1.0, "area {area}");
        // The biggest file gets the biggest rectangle.
        let big = leaves.iter().max_by(|a, b| a.rect.area().total_cmp(&b.rect.area())).unwrap();
        assert_eq!(&*t.get(big.id).name, "f20");
    }

    #[test]
    fn tiny_children_are_grouped() {
        let t = tree();
        let p = Params { min_px: 40.0, header_h: 0.0, pad: 0.0, ..Default::default() };
        let items = build(&t, t.root, Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)), &p, &[]);
        assert!(items.iter().any(|i| i.kind == ItemKind::Group));
        let covered: u64 = items.iter().filter(|i| i.kind != ItemKind::Dir).map(|i| i.size).sum();
        assert_eq!(covered, t.get(t.root).disk);
    }

    #[test]
    fn expand_reveals_one_more_level() {
        let mut t = Tree::new("C:\\".into(), 0, None);
        let dirs = t.add_entries(t.root, vec![
            Entry { name: "a".into(), kind: Kind::Dir, size: 0, disk: 0, mtime: 0 },
            Entry { name: "b".into(), kind: Kind::Dir, size: 0, disk: 0, mtime: 0 },
        ]);
        for &d in &dirs {
            t.add_entries(d, vec![Entry { name: "f".into(), kind: Kind::File, size: 50, disk: 50, mtime: 0 }]);
        }
        let p = Params { min_px: 1.0, header_h: 0.0, pad: 0.0, max_depth: 1, ..Default::default() };
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
        let files = |items: &[Item]| items.iter().filter(|i| i.kind == ItemKind::File).count();
        assert_eq!(files(&build(&t, t.root, r, &p, &[])), 0);
        // Expanding "a" shows its file but not b's.
        assert_eq!(files(&build(&t, t.root, r, &p, &[t.root, dirs[0]])), 1);
    }

    #[test]
    fn hit_returns_deepest() {
        let t = tree();
        let p = Params { min_px: 1.0, header_h: 0.0, pad: 0.0, ..Default::default() };
        let items = build(&t, t.root, Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0)), &p, &[]);
        let i = hit(&items, pos2(1.0, 1.0)).unwrap();
        assert_eq!(items[i].kind, ItemKind::File);
    }
}
