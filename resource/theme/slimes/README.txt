Place your 5x5 pixel slime sprite sheet (slime_sprites.png) in this folder:
  resource/theme/slimes/slime_sprites.png

When `cargo build` or `cargo build --release` runs, `build.rs` automatically copies the
entire `resource/` folder into `target/debug/resource/` or `target/release/resource/`
right next to the compiled `diskscope_v2` executable.
