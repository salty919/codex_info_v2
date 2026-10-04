use chrono::{DateTime, Days, Utc};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One failure owner writes only fixed route/stage/reason identifiers.
#[derive(Default)]
pub struct FailureLog {
    directory: Mutex<Option<PathBuf>>,
}

impl FailureLog {
    pub fn set_data_root(&self, root: &Path) {
        *self.directory.lock().unwrap_or_else(|p| p.into_inner()) = Some(root.join("logs/rest"));
    }

    pub fn record(&self, route: &str, stage: &str, reason: &str, status: Option<u16>) {
        let now = Utc::now();
        let line = json!({"timestamp":now.timestamp(), "route":route, "stage":stage,
            "reason":reason, "http_status":status})
        .to_string();
        eprintln!("codex-info-rest: {line}");
        if let Err(error) = self.append_at(now, &line) {
            eprintln!("codex-info-rest: log_write_failed kind={:?}", error.kind());
        }
    }

    fn append_at(&self, now: DateTime<Utc>, line: &str) -> io::Result<()> {
        let directory = self.directory.lock().unwrap_or_else(|p| p.into_inner());
        let Some(directory) = directory.as_ref() else {
            return Ok(());
        };
        fs::create_dir_all(directory)?;
        let day = now.date_naive();
        let path = directory.join(format!("rest-{}.log", day.format("%Y-%m-%d")));
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        let mut file = options.open(path)?;
        writeln!(file, "{line}")?;
        file.flush()?;
        let cutoff = format!("rest-{}.log", (day - Days::new(6)).format("%Y-%m-%d"));
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let bytes = name.as_bytes();
            if bytes.len() != 19
                || !name.starts_with("rest-")
                || !name.ends_with(".log")
                || bytes[9] != b'-'
                || bytes[12] != b'-'
                || !bytes[5..15]
                    .iter()
                    .enumerate()
                    .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
                || name >= cutoff.as_str()
            {
                continue;
            }
            fs::remove_file(entry.path())?;
        }
        Ok(())
    }
}
