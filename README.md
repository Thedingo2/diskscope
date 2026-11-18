# DiskScope v2

A fast, cross-platform GUI & Headless Server disk space analyzer written in Rust using `egui`, `rayon`, `rusqlite`, and `tiny_http`.

## Features
- **50/50 Split Desktop GUI & Matching Web GUI:** Browse directories with proportional size bars, sliding folder navigation transitions, right-click context menu (`Open`, `Open file location`, `Delete` to Recycle Bin/Trash), and live SQLite + file-type filtering.
- **Per-Folder Progressive Unlocking:** Parallel Rayon directory scanning commits each top-level folder to SQLite as soon as its worker finishes—completed folders unlock immediately for browsing while in-progress folders display a `Scanning...` lock badge.
- **Interactive Visualizations & Slime Health Widget:** Donut/pie chart breakdown, Top 10 Largest Files view, and an animated 5x5 pixel slime health widget (`resource/theme/slimes/slime_sprites.png`) reflecting free drive capacity.
- **Headless CLI & Toggleable Server Mode:** Launch the embedded web UI directly from the `☰ Menu` in the GUI or headless on any server via `--server --port 8080`.
- **Automatic Asset Bundling:** `build.rs` automatically copies `resource/` (`resource/theme/slimes/` and `resource/server/web_ui.html`) next to the compiled binary on every `cargo build`.

## Project Structure
```text
diskscope_v2/
├── Cargo.toml
├── build.rs
├── main.rs
├── app.rs
├── db.rs
├── debug.rs
├── health.rs
├── pie_chart.rs
├── scanner.rs
├── server.rs
├── .github/
│   └── workflows/
│       └── rust-ci.yml
└── resource/
    ├── server/
    │   └── web_ui.html
    └── theme/
        └── slimes/
            └── slime_sprites.png   (place your 5x5 sprite sheet here)
```

## Build & Run

### Desktop GUI
```bash
cargo build --release
cargo run --release
```

### Headless Server Mode
```bash
cargo run --release -- --server --port 8080 --pin 1234
```
Then open `http://localhost:8080` in your browser.
