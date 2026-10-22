//! Minimal file logger with size-based rotation (plexify.log, .1, .2 ...).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: usize = 3;

struct Inner {
    file: Option<File>,
    written: u64,
}

pub struct FileLogger {
    path: PathBuf,
    level: log::LevelFilter,
    echo_stderr: bool,
    inner: Mutex<Inner>,
}

impl FileLogger {
    fn open(path: &Path) -> (Option<File>, u64) {
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(f) => {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                (Some(f), len)
            }
            Err(_) => (None, 0),
        }
    }

    fn rotate(&self, inner: &mut Inner) {
        inner.file = None;
        for i in (1..KEEP).rev() {
            let from = rotated_name(&self.path, i);
            let to = rotated_name(&self.path, i + 1);
            let _ = std::fs::rename(from, to);
        }
        let _ = std::fs::rename(&self.path, rotated_name(&self.path, 1));
        let (f, len) = Self::open(&self.path);
        inner.file = f;
        inner.written = len;
    }
}

fn rotated_name(path: &Path, i: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{i}"));
    PathBuf::from(s)
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
            && !(metadata.target().starts_with("rustls") && metadata.level() > log::Level::Warn)
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} [{}] {}\n",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );
        if self.echo_stderr {
            eprint!("{line}");
        }
        let Ok(mut inner) = self.inner.lock() else { return };
        if inner.written + line.len() as u64 > MAX_BYTES {
            self.rotate(&mut inner);
        }
        if let Some(f) = inner.file.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                inner.written += line.len() as u64;
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(f) = inner.file.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// UTC timestamp "YYYY-MM-DD HH:MM:SS.mmm" without pulling in a date crate.
pub fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{:03}",
        sod / 3600,
        (sod / 60) % 60,
        sod % 60,
        now.subsec_millis()
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Install the global logger. Level comes from `PLEXIFY_LOG` (error..trace), default info.
pub fn init(dir: &Path, echo_stderr: bool) -> PathBuf {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("plexify.log");
    let level = std::env::var("PLEXIFY_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(log::LevelFilter::Info);
    let (file, written) = FileLogger::open(&path);
    let logger = FileLogger {
        path: path.clone(),
        level,
        echo_stderr,
        inner: Mutex::new(Inner { file, written }),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn rotation_keeps_bounded_files() {
        let dir = std::env::temp_dir().join(format!("plexify-logtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.log");
        let (file, written) = FileLogger::open(&path);
        let lg = FileLogger {
            path: path.clone(),
            level: log::LevelFilter::Info,
            echo_stderr: false,
            inner: Mutex::new(Inner { file, written }),
        };
        let msg = "x".repeat(1000);
        for _ in 0..5000 {
            log::Log::log(
                &lg,
                &log::Record::builder()
                    .args(format_args!("{msg}"))
                    .level(log::Level::Info)
                    .target("t")
                    .build(),
            );
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= MAX_BYTES);
        assert!(rotated_name(&path, 1).exists());
        assert!(rotated_name(&path, KEEP).exists());
        assert!(!rotated_name(&path, KEEP + 1).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
