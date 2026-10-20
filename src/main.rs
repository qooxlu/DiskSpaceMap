#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod layout;
mod render;
mod scan;
mod theme;
mod tree;
mod util;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(concat!("DiskSpaceMap v", env!("CARGO_PKG_VERSION")))
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    let initial = args.next();
    if initial.as_deref() == Some("--bench") {
        bench(&args.next().unwrap_or_else(|| "C:\\".into()));
        return Ok(());
    }
    eframe::run_native("DiskSpaceMap", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))))
}

/// Headless scan used to check totals and throughput: `DiskSpaceMap --bench D:\path`.
fn bench(path: &str) {
    let path = path.trim_end_matches('\\').to_string();
    let path = if util::is_drive_root(&path) { format!("{path}\\") } else { path };
    let root = util::drive_root(&path);
    let cluster = root.as_deref().map(util::cluster_size).unwrap_or(4096);
    let s = scan::Scan::new(tree::Tree::new(path.clone(), cluster, None));
    let t0 = std::time::Instant::now();
    scan::spawn(s.clone(), 0);
    while s.finished.read().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let t = s.tree.read();
    let r = t.get(t.root);
    println!(
        "path={path} files={} dirs={} size={} disk={} denied={} time={:.2}s cluster={cluster}",
        r.files,
        r.dirs,
        r.size,
        r.disk,
        s.failures(&t).len(),
        t0.elapsed().as_secs_f64()
    );
}
