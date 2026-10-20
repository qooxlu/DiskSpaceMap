use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HRESULT, PCWSTR};

use crate::tree::Volume;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "C:\" for a path on drive C, `None` for UNC or relative paths.
pub fn drive_root(path: &str) -> Option<String> {
    let b = path.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| format!("{}:\\", b[0] as char))
}

pub fn is_drive_root(path: &str) -> bool {
    let p = path.trim_end_matches('\\');
    p.len() == 2 && drive_root(p).is_some()
}

pub fn cluster_size(root: &str) -> u64 {
    let w = wide(root);
    let (mut spc, mut bps) = (0u32, 0u32);
    match unsafe { GetDiskFreeSpaceW(PCWSTR(w.as_ptr()), Some(&mut spc), Some(&mut bps), None, None) } {
        Ok(()) if spc > 0 && bps > 0 => spc as u64 * bps as u64,
        _ => 4096,
    }
}

pub fn volume(root: &str) -> Option<Volume> {
    let w = wide(root);
    let (mut total, mut free) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total), Some(&mut free)) }
        .ok()
        .map(|_| Volume { total, free })
}

pub struct DriveInfo {
    pub root: String,
    pub label: String,
    pub fs: String,
    pub kind: &'static str,
    pub volume: Option<Volume>,
}

pub fn list_drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let w = wide(&root);
        let kind = match unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } {
            2 => "Removable",
            3 => "Local Disk",
            4 => "Network",
            5 => "CD/DVD",
            6 => "RAM Disk",
            _ => continue,
        };
        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, Some(&mut fs))
        }
        .is_ok();
        let to_s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(0)]);
        out.push(DriveInfo {
            label: if ok { to_s(&label) } else { String::new() },
            fs: if ok { to_s(&fs) } else { String::new() },
            kind,
            volume: if ok { volume(&root) } else { None },
            root,
        });
    }
    out
}

pub fn shell_open(path: &str) {
    let op = wide("open");
    let file = wide(path);
    unsafe {
        ShellExecuteW(None, PCWSTR(op.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin() }.as_bool()
}

/// Starts a new elevated instance (UAC prompt) that scans `path`. Returns false if the user
/// declined or the launch failed.
pub fn relaunch_as_admin(path: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let op = wide("runas");
    let file = wide(&exe.display().to_string());
    // A trailing backslash would escape the closing quote (`"C:\"`), so drop it; "C:" is
    // normalised back to the drive root on startup.
    let params = wide(&format!("\"{}\"", path.trim_end_matches('\\')));
    let r = unsafe {
        ShellExecuteW(None, PCWSTR(op.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR(params.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL)
    };
    r.0 as isize > 32
}

/// System description of a failure code (localised by Windows).
pub fn error_text(hr: i32) -> String {
    let msg = HRESULT(hr).message();
    let msg = msg.trim().trim_end_matches(['.', '。']).to_string();
    if msg.is_empty() { format!("Error 0x{:08X}", hr as u32) } else { msg }
}

pub fn reveal_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
}

/// FILETIME (100ns since 1601) → "YYYY-MM-DD".
pub fn format_filetime(ft: u64) -> String {
    if ft == 0 {
        return String::new();
    }
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let days = secs.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", y + (m <= 2) as i64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(150 * 1024 * 1024), "150 MB");
        assert_eq!(format_count(1234567), "1,234,567");
    }

    #[test]
    fn roots() {
        assert!(is_drive_root("C:\\"));
        assert!(is_drive_root("d:"));
        assert!(!is_drive_root("C:\\Windows"));
        assert_eq!(drive_root("e:\\x").as_deref(), Some("e:\\"));
        assert_eq!(drive_root("\\\\server\\share"), None);
    }

    #[test]
    fn error_messages() {
        // E_ACCESSDENIED (HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED)) has a system description.
        let msg = error_text(0x8007_0005_u32 as i32);
        assert!(!msg.is_empty() && !msg.starts_with("Error 0x"), "{msg}");
        println!("access denied -> {msg}");
    }

    #[test]
    fn filetime() {
        // 2024-01-01T00:00:00Z
        assert_eq!(format_filetime(133_485_408_000_000_000), "2024-01-01");
    }
}
