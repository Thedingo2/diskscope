# DiskScope v0.03

> **Notice:** This project was inspired by [WinDirStat](https://windirstat.net/), and I genuinely recommend using that program over mine. I vibe coded this entire project, and I think you should support the original WinDirStat developers first!
>
> That said, after that fact, here is what my version built in Rust can do for you:

---

## ✨ Features

* **Parallel Drive & Folder Scanning:** Multi-threaded directory traversal powered by `rayon` and `walkdir` that calculates folder sizes fast and progressively unlocks completed subfolders in real time while scanning continues.
* **Instant SQLite Indexing (`diskscope.db`):** Caches folder hierarchies, extensions, and file sizes locally with partial indexes for sub-millisecond **Top 10 Largest Files** queries and instant drive-wide search.
* **Fast Category & Extension Filtering:** Filter indexed files across your drive from highest to lowest size by **Videos**, **Archives**, **Images**, **Audio**, **Executables**, or custom file extensions.
* **Interactive Donut Pie Chart:** Visualizes storage distribution for the active directory with hover tooltips, dynamic slice grouping (`Other` bucket), and crisp border separation.
* **Responsive Split-Pane File Browser:**
  * Relative size fill bars behind each row so heavy folders stand out immediately.
  * Smooth sliding transitions (`0.18s`) when drilling into folders or navigating back.
  * Right-click context menu to open files, reveal items in your OS file manager, or move files/folders safely to the Recycle Bin / Trash while updating cached ancestor sizes.
* **Animated Pixel-Slime Disk Health Indicator:** Tracks free drive capacity (`> 60%` happy jump, `10%–60%` shiver, `< 10%` critical flat squish) using custom sprite assets in `src/resource/theme/slimes/`.
* **Customizable TOML Themes:** Defaults to **Clean Light** (`src/themes/default.toml`) with **Dark Slate** (`src/themes/dark.toml`) and hot-reloadable custom `.toml` themes via **☰ Menu → Themes**.
* **Built-in Web Server Mode (`--server`):**
  * Start from the desktop menu or run completely headless on Linux, Windows, or macOS servers.
  * Full browser UI (`web_ui.html`) featuring Light/Dark mode toggle, optional PIN authentication, drive switching, live scan progress, folder swipe animations, and Recycle Bin/Trash deletion.
  * **HTTP 206 Byte-Range Media Streaming:** Click any video, audio file, or image in the web UI to play it in an inline lightbox player with instant seeking, or open a direct stream link in a new tab.
* **In-App Debug Console:** Built-in debug log viewer accessible from the desktop menu for inspecting scan timings, server events, and file operations.

---

## 🛠️ Building from Source

### Prerequisites
* [Rust toolchain](https://rustup.rs/) (`cargo` & `rustc` 1.75+)
* **Linux (Fedora / Ubuntu) build dependencies** (for `egui`/`eframe` windowing):
  ```bash
  # Fedora
  sudo dnf install gtk3-devel libxcb-devel libxkbcommon-devel openssl-devel

  # Ubuntu / Debian
  sudo apt install libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
