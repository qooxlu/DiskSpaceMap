use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Visuals};

pub const CLASS_NAMES: [&str; 9] =
    ["Other", "Video", "Image", "Audio", "Archive", "Program", "Document", "Code", "Data"];

/// Base colours for the file classes (indexed by `Node::class`).
pub const CLASS_COLORS: [Color32; 9] = [
    Color32::from_rgb(0x6b, 0x7f, 0x9e), // other
    Color32::from_rgb(0x95, 0x7a, 0xd6), // video
    Color32::from_rgb(0xcf, 0x7c, 0xab), // image
    Color32::from_rgb(0x4b, 0xa8, 0x9f), // audio
    Color32::from_rgb(0xd0, 0x9d, 0x52), // archive
    Color32::from_rgb(0xb9, 0x77, 0x6a), // program
    Color32::from_rgb(0x6c, 0xa8, 0x68), // document
    Color32::from_rgb(0x58, 0x95, 0xcf), // code
    Color32::from_rgb(0xa3, 0x9e, 0x5e), // data
];

pub const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1d);
pub const PANEL: Color32 = Color32::from_rgb(0x1e, 0x21, 0x28);
pub const DIR_BASE: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x4b);
pub const FREE: Color32 = Color32::from_rgb(0x2b, 0x4a, 0x3a);
pub const UNKNOWN: Color32 = Color32::from_rgb(0x3b, 0x3b, 0x40);
pub const GROUP: Color32 = Color32::from_rgb(0x33, 0x37, 0x40);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9a, 0xa1, 0xad);
pub const ACCENT: Color32 = Color32::from_rgb(0x6c, 0xb4, 0xff);

pub fn classify(name: &str) -> u8 {
    let Some(dot) = name.rfind('.') else { return 0 };
    let ext = name[dot + 1..].to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" | "ts" | "mts" | "vob" | "3gp" => 1,
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "psd" | "raw" | "cr2" | "nef" | "heic" | "svg" | "ico" | "dng" => 2,
        "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" | "wma" | "opus" | "aiff" | "ape" => 3,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "cab" | "wim" | "esd" => 4,
        "exe" | "dll" | "sys" | "msi" | "msp" | "drv" | "ocx" | "cpl" | "mui" | "bin" | "efi" | "appx" | "msix" => 5,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "rtf" | "odt" | "epub" | "csv" | "chm" => 6,
        "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "java" | "py" | "js" | "tsx" | "jsx" | "go" | "html" | "css"
        | "json" | "xml" | "yml" | "yaml" | "toml" | "pdb" | "lib" | "obj" | "o" | "a" | "rlib" | "rmeta" | "pyc" | "class" | "jar" => 7,
        "db" | "sqlite" | "mdb" | "log" | "dat" | "etl" | "evtx" | "pak" | "cache" | "tmp" | "bak" | "dmp" | "ldf" | "mdf" => 8,
        _ => 0,
    }
}

pub fn shade(c: Color32, f: f32) -> Color32 {
    let m = |v: u8| (v as f32 * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Black or white text, whichever reads better on `bg`.
pub fn text_on(bg: Color32) -> Color32 {
    let lum = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if lum > 150.0 { Color32::from_rgb(0x15, 0x17, 0x1c) } else { TEXT }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    // Segoe UI for Latin text, Microsoft YaHei as fallback so CJK file names render.
    for (key, file, index) in [("segoe", "segoeui.ttf", 0u32), ("yahei", "msyh.ttc", 0u32)] {
        if let Ok(bytes) = std::fs::read(format!(r"{windir}\Fonts\{file}")) {
            let mut fd = FontData::from_owned(bytes);
            fd.index = index;
            fonts.font_data.insert(key.into(), fd.into());
            let fam = fonts.families.entry(FontFamily::Proportional).or_default();
            let pos = if key == "segoe" { 0 } else { fam.len().min(1) };
            fam.insert(pos, key.into());
            fonts.families.entry(FontFamily::Monospace).or_default().push(key.into());
        }
    }
    ctx.set_fonts(fonts);

    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x22, 0x26, 0x2e);
    v.faint_bg_color = Color32::from_rgb(0x24, 0x28, 0x30);
    v.selection.bg_fill = Color32::from_rgb(0x2f, 0x5f, 0x96);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.fg_stroke.color = TEXT_DIM;
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x2a, 0x2e, 0x37);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x2a, 0x2e, 0x37);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x34, 0x39, 0x44);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x3d, 0x43, 0x50);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
        w.corner_radius = 6.into();
    }
    v.window_corner_radius = 10.into();
    v.menu_corner_radius = 8.into();
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}
