//! Parallel directory scanner. Each directory is enumerated without holding the tree lock;
//! the lock is only taken for the brief moment its entries are inserted.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use windows::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE, FindClose, FindExInfoBasic,
    FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetCompressedFileSizeW,
    WIN32_FIND_DATAW,
};
use windows::core::PCWSTR;

use crate::tree::{Entry, Kind, NodeId, Tree, flags};
use crate::util::wide;

pub struct Scan {
    pub tree: RwLock<Tree>,
    cluster: u64,
    pub cancel: AtomicBool,
    /// Bumped on every tree mutation; lets the UI skip relayout when nothing changed.
    pub version: AtomicU64,
    pub files: AtomicU64,
    /// Directories that could not be listed, with the HRESULT of the failure. Entries whose
    /// node was later removed or rescanned are filtered out by `failures()`.
    failures: Mutex<Vec<(NodeId, i32)>>,
    /// Directories queued or in progress. Zero means the scan is finished.
    pub pending: AtomicUsize,
    pub started: RwLock<Instant>,
    pub finished: RwLock<Option<Instant>>,
}

impl Scan {
    pub fn new(tree: Tree) -> Arc<Self> {
        Arc::new(Self {
            cluster: tree.cluster,
            tree: RwLock::new(tree),
            cancel: AtomicBool::new(false),
            version: AtomicU64::new(1),
            files: AtomicU64::new(0),
            failures: Mutex::new(Vec::new()),
            pending: AtomicUsize::new(0),
            started: RwLock::new(Instant::now()),
            finished: RwLock::new(None),
        })
    }

    pub fn is_running(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }

    /// Currently inaccessible directories (still in the tree and still marked denied).
    pub fn failures(&self, tree: &Tree) -> Vec<(NodeId, i32)> {
        self.failures.lock().iter().copied().filter(|&(id, _)| tree.is_alive(id) && tree.get(id).flags & flags::DENIED != 0).collect()
    }

    pub fn bump(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }
}

/// Scans the directory `node` (already present in the tree) in the background.
pub fn spawn(scan: Arc<Scan>, node: NodeId) {
    let path = scan.tree.read().path(node);
    scan.cancel.store(false, Ordering::Release);
    *scan.started.write() = Instant::now();
    *scan.finished.write() = None;
    scan.pending.fetch_add(1, Ordering::AcqRel);
    std::thread::spawn(move || {
        // Directory enumeration stops scaling at ~8 threads (contention in the file system stack).
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("scan thread pool");
        pool.scope(|s| scan_dir(s, &scan, node, path));
        *scan.finished.write() = Some(Instant::now());
        scan.bump();
    });
}

fn scan_dir<'s>(s: &rayon::Scope<'s>, scan: &'s Arc<Scan>, node: NodeId, path: String) {
    let result = if scan.cancel.load(Ordering::Relaxed) { Err(None) } else { enumerate(&path, scan.cluster).map_err(Some) };
    let subdirs = {
        let mut tree = scan.tree.write();
        if !tree.is_alive(node) {
            Vec::new()
        } else {
            match result {
                Ok(entries) => {
                    let files = entries.iter().filter(|e| e.kind == Kind::File).count();
                    scan.files.fetch_add(files as u64, Ordering::Relaxed);
                    let dirs = tree.add_entries(node, entries);
                    tree.get_mut(node).flags &= !flags::SCANNING;
                    dirs.into_iter().map(|d| (d, tree.get(d).name.clone())).collect::<Vec<_>>()
                }
                Err(code) => {
                    let n = tree.get_mut(node);
                    n.flags &= !flags::SCANNING;
                    if let Some(hr) = code.filter(|_| !scan.cancel.load(Ordering::Relaxed)) {
                        n.flags |= flags::DENIED;
                        scan.failures.lock().push((node, hr));
                    }
                    Vec::new()
                }
            }
        }
    };
    scan.bump();
    scan.pending.fetch_add(subdirs.len(), Ordering::AcqRel);
    for (id, name) in subdirs {
        let child = join(&path, &name);
        s.spawn(move |s| scan_dir(s, scan, id, child));
    }
    scan.pending.fetch_sub(1, Ordering::AcqRel);
}

fn join(base: &str, name: &str) -> String {
    let mut p = String::with_capacity(base.len() + name.len() + 1);
    p.push_str(base);
    if !p.ends_with('\\') {
        p.push('\\');
    }
    p.push_str(name);
    p
}

fn long_path(path: &str) -> String {
    if path.starts_with(r"\\?\") {
        path.to_string()
    } else if let Some(unc) = path.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{path}")
    }
}

/// Lists one directory. Reparse points (junctions, symlinks) are skipped to avoid cycles
/// and double counting.
fn enumerate(path: &str, cluster: u64) -> Result<Vec<Entry>, i32> {
    let pattern = wide(&join(&long_path(path), "*"));
    let mut fd = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            &mut fd as *mut _ as *mut _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    }
    .map_err(|e| e.code().0)?;

    let mut out = Vec::new();
    loop {
        let len = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
        let name = &fd.cFileName[..len];
        let dots = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
        let attrs = fd.dwFileAttributes;
        if !dots && attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
            let name_s = String::from_utf16_lossy(name);
            let mtime = (fd.ftLastWriteTime.dwHighDateTime as u64) << 32 | fd.ftLastWriteTime.dwLowDateTime as u64;
            if attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
                out.push(Entry { name: name_s.into(), kind: Kind::Dir, size: 0, disk: 0, mtime });
            } else {
                let size = (fd.nFileSizeHigh as u64) << 32 | fd.nFileSizeLow as u64;
                let mut disk = size;
                if attrs & (FILE_ATTRIBUTE_COMPRESSED.0 | FILE_ATTRIBUTE_SPARSE_FILE.0) != 0 {
                    let full = wide(&join(&long_path(path), &name_s));
                    let mut high = 0u32;
                    let low = unsafe { GetCompressedFileSizeW(PCWSTR(full.as_ptr()), Some(&mut high)) };
                    if low != u32::MAX {
                        disk = (high as u64) << 32 | low as u64;
                    }
                }
                if cluster > 0 && !disk.is_multiple_of(cluster) {
                    disk += cluster - disk % cluster;
                }
                out.push(Entry { name: name_s.into(), kind: Kind::File, size, disk, mtime });
            }
        }
        if unsafe { FindNextFileW(handle, &mut fd) }.is_err() {
            break;
        }
    }
    let _ = unsafe { FindClose(handle) };
    Ok(out)
}
