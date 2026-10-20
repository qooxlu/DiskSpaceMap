# DiskSpaceMap

简体中文 | [English](README.md)

DiskSpaceMap 是一个 Windows 磁盘空间可视化工具：它用 Treemap（矩形树图）显示文件夹和文件占用的空间，矩形越大，占用越多。界面用 Rust + [egui](https://github.com/emilk/egui) 编写，布局和交互参考了 [SpaceSniffer](https://www.uderzo.it/main_products/space_sniffer/)。

## 功能

- **并行扫描**：多线程枚举目录，扫描过程中 Treemap 会实时更新。
- **大小统计**：同时统计逻辑大小和实际占用（按簇取整，并考虑压缩/稀疏文件）。会跳过联接点（junction）和符号链接，避免重复计算。
- **Treemap 浏览**：
  - 点击方块可以展开下一层，双击放大进入，并有缩放动画。
  - 可以调整显示深度（`+` / `−`）。
  - 扫描整个磁盘时会显示剩余空间和未扫描的空间。
  - 按文件类型着色。
- **导航**：
  - 面包屑路径，以及后退 / 前进 / 上一级。
  - 快捷键：`Backspace` 或鼠标侧键后退，`Alt+←/→/↑`。
- **右键菜单**：打开、在资源管理器中显示、复制路径、重新扫描该文件夹、移到回收站。
- **无法访问的文件夹**：
  - 状态栏会显示数量，点击可以查看每个文件夹及失败原因。
  - 可以在 Treemap 中定位这些文件夹，也可以一键以管理员身份重新运行。
- **打开方式**：
  - 在启动页选择磁盘或文件夹。
  - 把文件夹拖到窗口上。
  - 在命令行中传入路径。

## 使用

程序是单个 exe 文件，不需要安装。支持 Windows 10/11 x64。

```sh
DiskSpaceMap.exe                    # 打开启动页
DiskSpaceMap.exe "D:\some\folder"   # 直接扫描指定路径
DiskSpaceMap.exe --bench C:\        # 无界面扫描，输出统计和耗时
```

扫描系统目录时，部分文件夹需要管理员权限才能读取。

## 从源码构建

需要 Rust 1.85 或更高版本（edition 2024），以及 MSVC 工具链（Visual Studio Build Tools）。

```sh
cargo build --release      # 产物：target/release/DiskSpaceMap.exe
cargo test
cargo clippy --all-targets
```

Release 版是 GUI 子系统程序，没有控制台窗口。如果要看 `--bench` 的输出，请用 debug 版（`cargo run -- --bench C:\`）。

## 代码结构

| 模块                       | 职责                                                                                          |
| -------------------------- | --------------------------------------------------------------------------------------------- |
| `scan.rs`                | 用 rayon 线程池并行枚举目录（`FindFirstFileExW`）。枚举时不持有树锁，只在写入结果时短暂加锁 |
| `tree.rs`                | 用一个数组存放所有节点。节点加入或移除时，大小会立即向上累加到所有祖先目录                    |
| `layout.rs`              | Treemap 布局：按大小排序后二分切割，太小的子项合并显示                                        |
| `render.rs`              | 生成渐变网格和标签，缩放动画用坐标变换实现                                                    |
| `app.rs`                 | egui 界面、导航历史、文件操作                                                                 |
| `util.rs` / `theme.rs` | Win32 辅助函数；配色和文件类型分类                                                            |

## 许可证

[MIT](LICENSE)
