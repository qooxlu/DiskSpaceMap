# DiskSpaceMap

[简体中文](README.cn.md) | English

DiskSpaceMap is a disk space visualizer for Windows. It shows the space used by folders and files as a treemap: the bigger the rectangle, the more space it takes. The UI is written in Rust with [egui](https://github.com/emilk/egui), and its layout and interaction are modeled on [SpaceSniffer](https://www.uderzo.it/main_products/space_sniffer/).

## Features

- **Parallel scanning**: directories are enumerated on multiple threads, and the treemap updates live while the scan runs.
- **Size accounting**: both logical size and size on disk are tracked (rounded to clusters, honoring compressed and sparse files). Junctions and symbolic links are skipped to avoid double counting.
- **Treemap browsing**:
  - Click a box to reveal one more level; double-click to zoom in, with a zoom animation.
  - Adjustable display depth (`+` / `−`).
  - When a whole drive is scanned, free space and unscanned space are shown too.
  - Colored by file type.
- **Navigation**:
  - Breadcrumb path, plus back / forward / up.
  - Shortcuts: `Backspace` or the mouse back button to go back, `Alt+←/→/↑`.
- **Context menu**: open, show in Explorer, copy path, rescan the folder, move to Recycle Bin.
- **Inaccessible folders**:
  - The status bar shows how many there are; click it to see each folder and why it failed.
  - Locate them in the treemap, or restart as administrator with one click.
- **Ways to open**:
  - Pick a drive or folder on the start screen.
  - Drop a folder onto the window.
  - Pass a path on the command line.

## Usage

The program is a single exe and needs no installation. It supports Windows 10/11 x64.

```sh
DiskSpaceMap.exe                    # open the start screen
DiskSpaceMap.exe "D:\some\folder"   # scan a path right away
DiskSpaceMap.exe --bench C:\        # headless scan, prints totals and elapsed time
```

Some system folders can only be read with administrator rights.

## Building from source

Requires Rust 1.85 or later (edition 2024) and the MSVC toolchain (Visual Studio Build Tools).

```sh
cargo build --release      # output: target/release/DiskSpaceMap.exe
cargo test
cargo clippy --all-targets
```

The release build is a GUI-subsystem program with no console window. To see the `--bench` output, use the debug build (`cargo run -- --bench C:\`).

## Code structure

| Module                     | Responsibility                                                                                                               |
| -------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `scan.rs`                | Parallel directory enumeration on a rayon pool (`FindFirstFileExW`). The tree lock is taken only briefly to insert results |
| `tree.rs`                | All nodes live in one array. Sizes are added up to every ancestor as soon as a node is inserted or removed                   |
| `layout.rs`              | Treemap layout: sort by size, then split in two; tiny children are grouped                                                   |
| `render.rs`              | Builds the gradient mesh and labels; the zoom animation is a coordinate transform                                            |
| `app.rs`                 | egui UI, navigation history, file operations                                                                                 |
| `util.rs` / `theme.rs` | Win32 helpers; palette and file-type classification                                                                          |

## License

[MIT](LICENSE)
