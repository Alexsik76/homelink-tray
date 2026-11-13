//! The log file. The configuration names it (`[log] file`, relative to the configuration
//! folder) and the level. It is rotated at 1 MB with two backups (`homelink.log.1` and
//! `.2`), as the Python version did. A line is
//! `2026-11-13T10:15:30Z INFO    target: message`; times are UTC, because the standard library
//! has no local time zone.
//!
//! Nothing secret is logged: states, ids, host names and error texts only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use homelink_core::config::LogLevel;
use log::{LevelFilter, Log, Metadata, Record};

/// Rotate when the file would grow beyond this many bytes.
pub(crate) const MAX_BYTES: u64 = 1_000_000;
/// How many rotated files are kept.
pub(crate) const BACKUPS: usize = 2;

/// `secs` seconds after 1970-01-01T00:00:00Z as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn format_utc(secs: u64) -> String {
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The calendar date of a day number counted from 1970-01-01 (proleptic Gregorian calendar,
/// after Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// One log line, without the line end.
pub(crate) fn format_line(secs: u64, level: log::Level, target: &str, message: &str) -> String {
    format!("{} {level:<7} {target}: {message}", format_utc(secs))
}

/// The file name of rotated copy number `n` of `path`: `homelink.log` -> `homelink.log.1`.
pub(crate) fn backup_path(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// An append-only text file that rotates itself.
pub(crate) struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    backups: usize,
    /// `None` only while the file is being rotated (or after a rotation that failed).
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    /// Opens (or creates) the file for appending.
    pub(crate) fn open(path: &Path, max_bytes: u64, backups: usize) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_owned(),
            max_bytes,
            backups,
            file: Some(file),
            size,
        })
    }

    /// Appends a line (and its line end), rotating first when it would not fit.
    pub(crate) fn write_line(&mut self, line: &str) -> io::Result<()> {
        let needed = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if self.size > 0 && self.size.saturating_add(needed) > self.max_bytes {
            self.rotate()?;
        }
        if self.file.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.size = file.metadata()?.len();
            self.file = Some(file);
        }
        if let Some(file) = self.file.as_mut() {
            writeln!(file, "{line}")?;
            self.size = self.size.saturating_add(needed);
        }
        Ok(())
    }

    /// Writes buffered data out.
    fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
    }

    /// Moves `file.1` to `file.2` (dropping the old `.2`), the file to `.1`, and starts a new
    /// file. With no backups the file is simply emptied.
    fn rotate(&mut self) -> io::Result<()> {
        // Close the file first: a file that is open cannot always be renamed on Windows.
        self.file = None;
        for n in (1..self.backups).rev() {
            let from = backup_path(&self.path, n);
            if from.exists() {
                let to = backup_path(&self.path, n + 1);
                let _ = fs::remove_file(&to);
                fs::rename(&from, &to)?;
            }
        }
        if self.backups > 0 {
            let first = backup_path(&self.path, 1);
            let _ = fs::remove_file(&first);
            fs::rename(&self.path, &first)?;
        }
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?,
        );
        self.size = 0;
        Ok(())
    }
}

/// The `log` backend that writes to a [`RotatingFile`].
struct FileLogger {
    level: LevelFilter,
    file: Mutex<RotatingFile>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let line = format_line(
            secs,
            record.level(),
            record.target(),
            &record.args().to_string(),
        );
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_line(&line);
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            file.flush();
        }
    }
}

/// The `log` level filter of a configured level.
pub(crate) fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::Error,
        LogLevel::Warn => LevelFilter::Warn,
        LogLevel::Info => LevelFilter::Info,
        LogLevel::Debug => LevelFilter::Debug,
    }
}

/// Starts logging to `path` and makes a panic a log line too. Fails if the file cannot be
/// opened or a logger is already set; the program then runs without a log.
pub(crate) fn init(path: &Path, level: LogLevel) -> Result<(), String> {
    let file = RotatingFile::open(path, MAX_BYTES, BACKUPS)
        .map_err(|err| format!("cannot open the log file {}: {err}", path.display()))?;
    let filter = level_filter(level);
    let logger = FileLogger {
        level: filter,
        file: Mutex::new(file),
    };
    log::set_boxed_logger(Box::new(logger)).map_err(|err| err.to_string())?;
    log::set_max_level(filter);
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("homelink-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn times_are_formatted_in_utc() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        // 29 February of a leap year, and the day after the end of February of a normal year.
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(format_utc(1_677_628_800), "2023-03-01T00:00:00Z");
        // The last second of a year.
        assert_eq!(format_utc(1_735_689_599), "2024-12-31T23:59:59Z");
        assert_eq!(format_utc(1_735_689_600), "2025-01-01T00:00:00Z");
    }

    #[test]
    fn a_line_has_time_level_target_and_message() {
        assert_eq!(
            format_line(0, log::Level::Info, "homelink", "started"),
            "1970-01-01T00:00:00Z INFO    homelink: started"
        );
        assert_eq!(
            format_line(0, log::Level::Warn, "x", "m"),
            "1970-01-01T00:00:00Z WARN    x: m"
        );
    }

    #[test]
    fn the_levels_map_to_the_log_filters() {
        assert_eq!(level_filter(LogLevel::Error), LevelFilter::Error);
        assert_eq!(level_filter(LogLevel::Debug), LevelFilter::Debug);
    }

    #[test]
    fn backups_are_numbered_after_the_file_name() {
        assert_eq!(
            backup_path(Path::new("dir/homelink.log"), 2),
            PathBuf::from("dir/homelink.log.2")
        );
    }

    #[test]
    fn lines_are_appended_and_the_file_survives_a_reopen() {
        let dir = temp_dir("append");
        let path = dir.join("a.log");
        let mut file = RotatingFile::open(&path, 1000, 2).unwrap();
        file.write_line("one").unwrap();
        drop(file);
        let mut file = RotatingFile::open(&path, 1000, 2).unwrap();
        file.write_line("two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_rotates_at_the_limit_and_keeps_two_backups() {
        let dir = temp_dir("rotate");
        let path = dir.join("a.log");
        // Each line is 10 bytes with its line end; the limit allows three of them.
        let mut file = RotatingFile::open(&path, 30, 2).unwrap();
        for n in 0..10 {
            file.write_line(&format!("line {n:04}")).unwrap();
        }
        let read = |p: &Path| fs::read_to_string(p).unwrap();
        assert_eq!(read(&path), "line 0009\n");
        assert_eq!(
            read(&backup_path(&path, 1)),
            "line 0006\nline 0007\nline 0008\n"
        );
        assert_eq!(
            read(&backup_path(&path, 2)),
            "line 0003\nline 0004\nline 0005\n"
        );
        assert!(!backup_path(&path, 3).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_backups_the_file_is_just_emptied() {
        let dir = temp_dir("nobackup");
        let path = dir.join("a.log");
        let mut file = RotatingFile::open(&path, 20, 0).unwrap();
        for n in 0..5 {
            file.write_line(&format!("line {n:04}")).unwrap();
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "line 0004\n");
        assert!(!backup_path(&path, 1).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_still_written() {
        let dir = temp_dir("long");
        let path = dir.join("a.log");
        let mut file = RotatingFile::open(&path, 5, 1).unwrap();
        file.write_line("a very long line").unwrap();
        file.write_line("next").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "next\n");
        assert_eq!(
            fs::read_to_string(backup_path(&path, 1)).unwrap(),
            "a very long line\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
