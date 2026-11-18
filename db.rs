use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::time::Duration;

use crate::scanner::{FileItem, ScanStatus};

fn db_path() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));

    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));

    if let Some(dir) = base.map(|b| b.join("DiskScope")) {
        let _ = std::fs::create_dir_all(&dir);
        return dir.join("diskscope.db");
    }

    PathBuf::from("diskscope.db")
}

pub struct IndexRow {
    pub path: String,
    pub parent: String,
    pub name: String,
    pub ext: String,
    pub size: u64,
    pub is_dir: bool,
}

pub fn open_conn() -> Option<Connection> {
    let conn = Connection::open(db_path()).ok()?;
    let _ = conn.busy_timeout(Duration::from_secs(2));
    Some(conn)
}

/// Instant startup init; builds the partial large-file and extension-size indexes
/// on a background thread so the GUI window opens immediately.
pub fn init_db() {
    if let Some(conn) = open_conn() {
        let _ = conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = OFF;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -32000;
             CREATE TABLE IF NOT EXISTS file_index (
                path   TEXT PRIMARY KEY,
                parent TEXT NOT NULL,
                name   TEXT NOT NULL,
                ext    TEXT NOT NULL DEFAULT '',
                size   INTEGER NOT NULL DEFAULT 0,
                is_dir INTEGER NOT NULL DEFAULT 0
             );
             DROP INDEX IF EXISTS idx_file_index_ext_size;
             DROP INDEX IF EXISTS idx_file_index_files_size;
             DROP INDEX IF EXISTS idx_file_index_name_size;
             CREATE INDEX IF NOT EXISTS idx_file_index_parent ON file_index(parent);
             CREATE TABLE IF NOT EXISTS indexed_dirs (path TEXT PRIMARY KEY);",
        );
    }

    std::thread::spawn(|| {
        if let Some(conn) = open_conn() {
            let _ = conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_file_index_large_files
                 ON file_index(size DESC)
                 WHERE is_dir = 0 AND size >= 10485760;
                 CREATE INDEX IF NOT EXISTS idx_file_index_ext_large
                 ON file_index(ext, size DESC)
                 WHERE is_dir = 0 AND size >= 1048576;",
            );
        }
    });
}

fn parent_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn ext_of(path: &str, is_dir: bool) -> String {
    if is_dir {
        return String::new();
    }
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn subtree_prefix(path: &str) -> String {
    if path.ends_with(MAIN_SEPARATOR) {
        path.to_string()
    } else {
        format!("{}{}", path, MAIN_SEPARATOR)
    }
}

pub fn is_indexed(path: &str) -> bool {
    let Some(conn) = open_conn() else { return false };
    let Ok(mut stmt) = conn.prepare("SELECT path FROM indexed_dirs") else { return false };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else { return false };

    let target = Path::new(path);
    let found = rows.flatten().any(|root| target.starts_with(Path::new(&root)));
    found
}

pub fn mark_indexed(path: &str) {
    if let Some(conn) = open_conn() {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO indexed_dirs (path) VALUES (?1)",
            params![path],
        );
    }
}

pub fn get_children(parent: &str) -> Vec<FileItem> {
    let Some(conn) = open_conn() else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT path, name, size, is_dir FROM file_index WHERE parent = ?1",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(params![parent], |r| {
        Ok(FileItem {
            path: r.get(0)?,
            name: r.get(1)?,
            size: r.get(2)?,
            is_dir: r.get(3)?,
            status: ScanStatus::Done,
        })
    }) else {
        return Vec::new();
    };

    rows.flatten().collect()
}

pub fn search_items(root_drive: &str, query: &str, type_filter: &str, limit: usize) -> Vec<FileItem> {
    let Some(conn) = open_conn() else { return Vec::new() };
    let prefix = subtree_prefix(root_drive);
    let upper = format!("{}\u{FFFF}", prefix);
    let q_trim = query.trim().to_ascii_lowercase();
    let has_query = !q_trim.is_empty();

    let category_exts: Option<&[&str]> = match type_filter {
        "Videos" => Some(&["mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v"]),
        "Archives" => Some(&["zip", "7z", "rar", "tar", "gz", "xz", "iso", "dmg"]),
        "Images" => Some(&["png", "jpg", "jpeg", "gif", "webp", "bmp", "psd", "tga", "svg"]),
        "Audio" => Some(&["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus"]),
        "Executables" => Some(&["exe", "msi", "app", "pkg", "deb", "rpm", "dmg", "bin"]),
        _ => None,
    };

    let custom_ext = type_filter.trim().trim_start_matches('.').to_ascii_lowercase();
    let fetch_cap = limit as i64;

    if type_filter == "All" || type_filter.is_empty() {
        let sql = if has_query {
            "SELECT path, name, size, is_dir FROM file_index
             WHERE (path >= ?1 AND path < ?2) AND instr(lower(name), ?3) > 0
             ORDER BY size DESC LIMIT ?4"
        } else {
            "SELECT path, name, size, is_dir FROM file_index
             WHERE (path >= ?1 AND path < ?2)
             ORDER BY size DESC LIMIT ?4"
        };
        let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
        let Ok(rows) = stmt.query_map(params![prefix, upper, q_trim, fetch_cap], |r| {
            Ok(FileItem {
                path: r.get(0)?,
                name: r.get(1)?,
                size: r.get(2)?,
                is_dir: r.get(3)?,
                status: ScanStatus::Done,
            })
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
    } else if let Some(exts) = category_exts {
        let placeholders: Vec<String> = (0..exts.len()).map(|i| format!("?{}", i + 5)).collect();
        let in_clause = placeholders.join(", ");

        // Fast path: uses idx_file_index_ext_large (files >= 1 MB) in descending size order
        let fast_sql = format!(
            "SELECT path, name, size, is_dir FROM file_index
             WHERE is_dir = 0 AND size >= 1048576 AND (path >= ?1 AND path < ?2)
               AND (?3 = '' OR instr(lower(name), ?3) > 0) AND ext IN ({})
             ORDER BY size DESC LIMIT ?4",
            in_clause
        );
        let mut results: Vec<FileItem> = if let Ok(mut stmt) = conn.prepare(&fast_sql) {
            let mut param_values: Vec<&dyn rusqlite::ToSql> =
                vec![&prefix, &upper, &q_trim, &fetch_cap];
            for ext in exts {
                param_values.push(ext);
            }
            stmt.query_map(rusqlite::params_from_iter(param_values), |r| {
                Ok(FileItem {
                    path: r.get(0)?,
                    name: r.get(1)?,
                    size: r.get(2)?,
                    is_dir: r.get(3)?,
                    status: ScanStatus::Done,
                })
            })
            .ok()
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
        } else {
            Vec::new()
        };

        // Fallback if fewer than `limit` files >= 1 MB exist (e.g. small audio clips)
        if results.len() < limit {
            let full_sql = format!(
                "SELECT path, name, size, is_dir FROM file_index
                 WHERE is_dir = 0 AND (path >= ?1 AND path < ?2)
                   AND (?3 = '' OR instr(lower(name), ?3) > 0) AND ext IN ({})
                 ORDER BY size DESC LIMIT ?4",
                in_clause
            );
            if let Ok(mut stmt) = conn.prepare(&full_sql) {
                let mut param_values: Vec<&dyn rusqlite::ToSql> =
                    vec![&prefix, &upper, &q_trim, &fetch_cap];
                for ext in exts {
                    param_values.push(ext);
                }
                if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(param_values), |r| {
                    Ok(FileItem {
                        path: r.get(0)?,
                        name: r.get(1)?,
                        size: r.get(2)?,
                        is_dir: r.get(3)?,
                        status: ScanStatus::Done,
                    })
                }) {
                    results = rows.flatten().collect();
                }
            }
        }
        results
    } else {
        let Ok(mut stmt) = conn.prepare(
            "SELECT path, name, size, is_dir FROM file_index
             WHERE is_dir = 0 AND (path >= ?1 AND path < ?2)
               AND (?3 = '' OR instr(lower(name), ?3) > 0) AND ext = ?4
             ORDER BY size DESC LIMIT ?5",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(
            params![prefix, upper, q_trim, custom_ext, fetch_cap],
            |r| {
                Ok(FileItem {
                    path: r.get(0)?,
                    name: r.get(1)?,
                    size: r.get(2)?,
                    is_dir: r.get(3)?,
                    status: ScanStatus::Done,
                })
            },
        ) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }
}

pub fn get_top_files(root_drive: &str, limit: usize) -> Vec<FileItem> {
    let Some(conn) = open_conn() else { return Vec::new() };
    let prefix = subtree_prefix(root_drive);
    let upper = format!("{}\u{FFFF}", prefix);

    let Ok(mut stmt) = conn.prepare(
        "SELECT path, name, size, is_dir FROM file_index
         WHERE is_dir = 0 AND size >= 10485760 AND (path >= ?1 AND path < ?2)
         ORDER BY size DESC LIMIT ?3",
    ) else {
        return Vec::new();
    };
    let mut items: Vec<FileItem> = stmt
        .query_map(params![prefix, upper, limit as i64], |r| {
            Ok(FileItem {
                path: r.get(0)?,
                name: r.get(1)?,
                size: r.get(2)?,
                is_dir: r.get(3)?,
                status: ScanStatus::Done,
            })
        })
        .ok()
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();

    if items.is_empty() {
        if let Ok(mut fallback_stmt) = conn.prepare(
            "SELECT path, name, size, is_dir FROM file_index
             WHERE is_dir = 0 AND (path >= ?1 AND path < ?2)
             ORDER BY size DESC LIMIT ?3",
        ) {
            if let Ok(rows) = fallback_stmt.query_map(params![prefix, upper, limit as i64], |r| {
                Ok(FileItem {
                    path: r.get(0)?,
                    name: r.get(1)?,
                    size: r.get(2)?,
                    is_dir: r.get(3)?,
                    status: ScanStatus::Done,
                })
            }) {
                items = rows.flatten().collect();
            }
        }
    }

    items
}

pub fn get_cached_sizes(parent: &str) -> HashMap<String, u64> {
    let mut map = HashMap::new();
    let Some(conn) = open_conn() else { return map };
    let Ok(mut stmt) = conn.prepare("SELECT path, size FROM file_index WHERE parent = ?1") else {
        return map;
    };
    let Ok(rows) = stmt.query_map(params![parent], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
    }) else {
        return map;
    };
    for (path, size) in rows.flatten() {
        map.insert(path, size);
    }
    map
}

pub fn insert_rows(conn: &mut Connection, rows: &[IndexRow]) {
    let _ = conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;");
    let Ok(tx) = conn.transaction() else { return };
    {
        let Ok(mut stmt) = tx.prepare_cached(
            "INSERT OR REPLACE INTO file_index (path, parent, name, ext, size, is_dir)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        ) else {
            return;
        };
        for r in rows {
            let _ = stmt.execute(params![r.path, r.parent, r.name, r.ext, r.size, r.is_dir]);
        }
    }
    let _ = tx.commit();
}

pub fn save_items_to_cache(items: &[FileItem]) {
    let Some(mut conn) = open_conn() else { return };
    let rows: Vec<IndexRow> = items
        .iter()
        .map(|i| IndexRow {
            path: i.path.clone(),
            parent: parent_of(&i.path),
            name: i.name.clone(),
            ext: ext_of(&i.path, i.is_dir),
            size: i.size,
            is_dir: i.is_dir,
        })
        .collect();
    insert_rows(&mut conn, &rows);
}

pub fn remove_path(path: &str, fallback_size: u64) {
    let Some(conn) = open_conn() else { return };

    let size: u64 = conn
        .query_row(
            "SELECT size FROM file_index WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )
        .unwrap_or(fallback_size);

    let prefix = subtree_prefix(path);
    let len = prefix.chars().count() as i64;
    let _ = conn.execute(
        "DELETE FROM file_index WHERE path = ?1 OR substr(path, 1, ?2) = ?3",
        params![path, len, prefix],
    );
    let _ = conn.execute(
        "DELETE FROM indexed_dirs WHERE path = ?1 OR substr(path, 1, ?2) = ?3",
        params![path, len, prefix],
    );

    for ancestor in Path::new(path).ancestors().skip(1) {
        let a = ancestor.to_string_lossy().into_owned();
        let _ = conn.execute(
            "UPDATE file_index SET size = MAX(size - ?1, 0) WHERE path = ?2",
            params![size, a],
        );
    }
}