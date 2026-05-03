//! Best-effort lock files for keeping cache body/meta pairs consistent.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, SystemTime};

const STALE_AFTER: Duration = Duration::from_secs(10 * 60);
const RETRIES: usize = 100;
const SLEEP: Duration = Duration::from_millis(50);

pub(crate) struct FileLock {
    path: String,
}

impl FileLock {
    pub(crate) fn acquire(path: String) -> Result<Self, String> {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create lock dir {}: {e}", parent.display()))?;
        }

        for _ in 0..RETRIES {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", crate::http_cache::now_secs());
                    return Ok(Self { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_if_stale(&path);
                    std::thread::sleep(SLEEP);
                }
                Err(err) => return Err(format!("create lock {path}: {err}")),
            }
        }

        Err(format!("timed out waiting for cache lock {path}"))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn remove_if_stale(path: &str) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let Ok(modified) = meta.modified() else {
        return;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return;
    };
    if age > STALE_AFTER {
        let _ = std::fs::remove_file(path);
    }
}
