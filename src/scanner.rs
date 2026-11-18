use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::{ext_of, IndexRow};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScanStatus {
    Pending,
    Scanning,
    Done,
}

#[derive(Clone, Debug)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub status: ScanStatus,
}

pub fn get_available_drives() -> Vec<String> {
    #[cfg(windows)]
    {
        let mut drives = Vec::new();
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            if Path::new(&root).exists() {
                drives.push(root);
            }
        }
        if drives.is_empty() {
            drives.push("C:\\".to_string());
        }
        drives
    }

    #[cfg(target_os = "macos")]
    {
        let mut drives = vec!["/".to_string()];
        if let Ok(entries) = fs::read_dir("/Volumes") {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    drives.push(p.to_string_lossy().into_owned());
                }
            }
        }
        drives
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut drives = vec!["/".to_string()];
        for base in ["/mnt", "/media", "/run/media"] {
            if let Ok(entries) = fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        drives.push(p.to_string_lossy().into_owned());
                    }
                }
            }
        }
        drives
    }
}

pub fn get_drive_space(path: &str) -> (u64, u64) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes_avail = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free_bytes = 0u64;

        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_bytes_avail,
                &mut total_bytes,
                &mut total_free_bytes,
            )
        };
        if ok != 0 {
            return (total_bytes, total_free_bytes);
        }
        (0, 0)
    }

    #[cfg(unix)]
    {
        use std::ffi::CString;
        let Ok(c_path) = CString::new(path) else { return (0, 0) };
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
                let total = (stat.f_blocks as u64).saturating_mul(stat.f_frsize as u64);
                let free = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
                return (total, free);
            }
        }
        (0, 0)
    }
}

fn should_skip_dir(path: &Path) -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if matches!(
            path.to_str(),
            Some("/proc" | "/sys" | "/dev" | "/run" | "/snap")
        ) {
            return true;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(s) = path.to_str() {
            if s.starts_with("/System/Volumes") || s == "/dev" {
                return true;
            }
        }
    }
    let _ = path;
    false
}

pub fn scan_subtree_in_memory(dir: &Path) -> (u64, Vec<IndexRow>) {
    if should_skip_dir(dir) {
        return (0, Vec::new());
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return (0, Vec::new());
    };

    let mut subdirs: Vec<(PathBuf, String)> = Vec::new();
    let mut local_rows: Vec<IndexRow> = Vec::new();
    let mut local_size: u64 = 0;

    let parent_str = dir.to_string_lossy().into_owned();

    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        if ft.is_dir() {
            subdirs.push((path, name));
        } else if ft.is_file() {
            let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
            local_size = local_size.saturating_add(sz);
            let path_str = path.to_string_lossy().into_owned();
            let ext = ext_of(&path_str, false);
            local_rows.push(IndexRow {
                path: path_str,
                parent: parent_str.clone(),
                name,
                ext,
                size: sz,
                is_dir: false,
            });
        }
    }

    let sub_results: Vec<(u64, Vec<IndexRow>)> = subdirs
        .into_par_iter()
        .map(|(sub_path, sub_name)| {
            let (sub_sz, mut sub_rows) = scan_subtree_in_memory(&sub_path);
            let sub_path_str = sub_path.to_string_lossy().into_owned();
            sub_rows.push(IndexRow {
                path: sub_path_str,
                parent: parent_str.clone(),
                name: sub_name,
                ext: String::new(),
                size: sub_sz,
                is_dir: true,
            });
            (sub_sz, sub_rows)
        })
        .collect();

    for (sub_sz, mut sub_rows) in sub_results {
        local_size = local_size.saturating_add(sub_sz);
        local_rows.append(&mut sub_rows);
    }

    (local_size, local_rows)
}
