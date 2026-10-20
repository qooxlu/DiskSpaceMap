//! Arena-backed file system tree. Sizes are aggregated eagerly up the parent chain,
//! so a directory's totals are always current while a scan is running.

pub type NodeId = u32;
pub const NONE: NodeId = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    File,
}

pub mod flags {
    pub const SCANNING: u8 = 1;
    pub const DENIED: u8 = 2;
    pub const DELETED: u8 = 4;
}

pub struct Node {
    pub name: Box<str>,
    pub parent: NodeId,
    pub children: Vec<NodeId>,
    /// Logical size in bytes (directories: sum of descendants).
    pub size: u64,
    /// Allocated size on disk (rounded to clusters, honours compression/sparse).
    pub disk: u64,
    /// Number of descendant files (a file counts itself).
    pub files: u32,
    /// Number of descendant directories.
    pub dirs: u32,
    pub mtime: u64,
    pub kind: Kind,
    pub class: u8,
    pub flags: u8,
}

/// Raw directory entry produced by the scanner before it enters the tree.
pub struct Entry {
    pub name: Box<str>,
    pub kind: Kind,
    pub size: u64,
    pub disk: u64,
    pub mtime: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Volume {
    pub total: u64,
    pub free: u64,
}

pub struct Tree {
    nodes: Vec<Node>,
    pub root: NodeId,
    /// Absolute path of the root, without trailing separator except for drive roots ("C:\").
    pub root_path: String,
    pub cluster: u64,
    pub volume: Option<Volume>,
}

impl Tree {
    pub fn new(root_path: String, cluster: u64, volume: Option<Volume>) -> Self {
        let name: Box<str> = root_path.clone().into();
        let root = Node {
            name,
            parent: NONE,
            children: Vec::new(),
            size: 0,
            disk: 0,
            files: 0,
            dirs: 0,
            mtime: 0,
            kind: Kind::Dir,
            class: 0,
            flags: flags::SCANNING,
        };
        Self { nodes: vec![root], root: 0, root_path, cluster, volume }
    }

    #[inline]
    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    #[inline]
    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    /// Inserts scanned entries under `parent`, propagates totals to all ancestors and
    /// returns the ids of the new directories (in entry order) for further scanning.
    pub fn add_entries(&mut self, parent: NodeId, entries: Vec<Entry>) -> Vec<NodeId> {
        let mut dirs = Vec::new();
        let (mut size, mut disk, mut files, mut ndirs) = (0u64, 0u64, 0u32, 0u32);
        let base = self.nodes.len() as NodeId;
        self.nodes.reserve(entries.len());
        let mut ids = Vec::with_capacity(entries.len());
        for (i, e) in entries.into_iter().enumerate() {
            let id = base + i as NodeId;
            size += e.size;
            disk += e.disk;
            let (class, f) = match e.kind {
                Kind::File => {
                    files += 1;
                    (crate::theme::classify(&e.name), 0)
                }
                Kind::Dir => {
                    ndirs += 1;
                    dirs.push(id);
                    (0, flags::SCANNING)
                }
            };
            self.nodes.push(Node {
                name: e.name,
                parent,
                children: Vec::new(),
                size: e.size,
                disk: e.disk,
                files: (e.kind == Kind::File) as u32,
                dirs: 0,
                mtime: e.mtime,
                kind: e.kind,
                class,
                flags: f,
            });
            ids.push(id);
        }
        self.get_mut(parent).children.extend(ids);
        self.propagate(parent, size as i64, disk as i64, files as i64, ndirs as i64);
        dirs
    }

    fn propagate(&mut self, mut id: NodeId, size: i64, disk: i64, files: i64, dirs: i64) {
        while id != NONE {
            let n = &mut self.nodes[id as usize];
            n.size = n.size.saturating_add_signed(size);
            n.disk = n.disk.saturating_add_signed(disk);
            n.files = (n.files as i64 + files).max(0) as u32;
            n.dirs = (n.dirs as i64 + dirs).max(0) as u32;
            id = n.parent;
        }
    }

    /// Detaches `id` from its parent and subtracts its totals from every ancestor.
    /// The arena slot is left behind (marked deleted); it is reclaimed with the tree.
    pub fn remove(&mut self, id: NodeId) {
        let n = self.get(id);
        let parent = n.parent;
        if parent == NONE {
            return;
        }
        let (size, disk) = (n.size as i64, n.disk as i64);
        let (files, dirs) = (n.files as i64, n.dirs as i64 + (n.kind == Kind::Dir) as i64);
        self.get_mut(parent).children.retain(|&c| c != id);
        self.get_mut(id).flags |= flags::DELETED;
        self.propagate(parent, -size, -disk, -files, -dirs);
    }

    /// Empties a directory so it can be rescanned in place.
    pub fn clear_dir(&mut self, id: NodeId) {
        let n = self.get(id);
        let (size, disk, files, dirs) = (n.size as i64, n.disk as i64, n.files as i64, n.dirs as i64);
        let children = std::mem::take(&mut self.get_mut(id).children);
        for c in children {
            self.get_mut(c).flags |= flags::DELETED;
        }
        let n = self.get_mut(id);
        n.flags = flags::SCANNING;
        self.propagate(id, -size, -disk, -files, -dirs);
    }

    pub fn path(&self, id: NodeId) -> String {
        let mut parts = Vec::new();
        let mut cur = id;
        while cur != self.root && cur != NONE {
            let n = self.get(cur);
            parts.push(&*n.name);
            cur = n.parent;
        }
        let mut p = self.root_path.clone();
        for part in parts.iter().rev() {
            if !p.ends_with('\\') {
                p.push('\\');
            }
            p.push_str(part);
        }
        p
    }

    /// Ancestors from the root down to `id` (inclusive).
    pub fn lineage(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = Vec::new();
        let mut cur = id;
        while cur != NONE {
            v.push(cur);
            cur = self.get(cur).parent;
        }
        v.reverse();
        v
    }

    pub fn is_alive(&self, id: NodeId) -> bool {
        (id as usize) < self.nodes.len() && self.get(id).flags & flags::DELETED == 0
    }

    /// Unaccounted space on the volume: total - free - scanned. Only meaningful for drive roots.
    pub fn unknown_space(&self) -> u64 {
        match self.volume {
            Some(v) => v.total.saturating_sub(v.free).saturating_sub(self.get(self.root).disk),
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> Entry {
        Entry { name: name.into(), kind: Kind::File, size, disk: size, mtime: 0 }
    }
    fn dir(name: &str) -> Entry {
        Entry { name: name.into(), kind: Kind::Dir, size: 0, disk: 0, mtime: 0 }
    }

    #[test]
    fn totals_propagate_and_remove() {
        let mut t = Tree::new("C:\\".into(), 4096, None);
        let d = t.add_entries(t.root, vec![dir("a"), file("x", 10)]);
        let a = d[0];
        t.add_entries(a, vec![file("y", 5), file("z", 7)]);
        assert_eq!(t.get(t.root).size, 22);
        assert_eq!(t.get(t.root).files, 3);
        assert_eq!(t.get(t.root).dirs, 1);
        assert_eq!(t.path(a), "C:\\a");
        t.remove(a);
        assert_eq!(t.get(t.root).size, 10);
        assert_eq!(t.get(t.root).files, 1);
        assert_eq!(t.get(t.root).dirs, 0);
        assert_eq!(t.get(t.root).children.len(), 1);
    }

    #[test]
    fn clear_dir_resets_totals() {
        let mut t = Tree::new("D:\\data".into(), 4096, None);
        let a = t.add_entries(t.root, vec![dir("a")])[0];
        t.add_entries(a, vec![file("y", 5)]);
        t.clear_dir(a);
        assert_eq!(t.get(t.root).size, 0);
        assert_eq!(t.get(a).children.len(), 0);
        assert_eq!(t.path(a), "D:\\data\\a");
    }
}
