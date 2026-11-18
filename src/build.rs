use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=resource");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_resource = manifest_dir.join("resource");
    if !src_resource.exists() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    if let Some(target_profile_dir) = out_dir.ancestors().nth(3) {
        let dst_resource = target_profile_dir.join("resource");
        let _ = copy_dir_recursive(&src_resource, &dst_resource);
    }
}
