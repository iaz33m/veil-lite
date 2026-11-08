//! Metadata-only activity log (JSON lines). Never stores message text, file contents, matched values,
//! file paths or full URLs: only app, site, channel, detection *types* and what happened.

use crate::findings::Finding;
use crate::types::{AppContext, Channel};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Nothing found; let through (logged only when files/images were checked).
    Allowed,
    /// The user chose to send anyway.
    SentAnyway,
    /// The user redacted and sent.
    Redacted,
    /// The user cancelled.
    Cancelled,
    /// The policy blocked it.
    Blocked,
    /// A drop was stopped.
    DropBlocked,
    /// Content could not be checked (fail-open/closed applied).
    Unchecked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub ts_ms: u64,
    pub app_id: String,
    pub app_name: String,
    /// Host only, e.g. `chatgpt.com`.
    pub site: Option<String>,
    pub channel: Channel,
    /// Detection types, e.g. `email_address`, `keyword`, `ai:secrets`.
    pub detections: Vec<String>,
    pub outcome: Outcome,
    /// Number of files involved (names are not stored).
    #[serde(default)]
    pub files: usize,
}

impl ActivityEntry {
    pub fn new(app: &AppContext, channel: Channel, findings: &[Finding], outcome: Outcome, files: usize) -> Self {
        let mut detections: Vec<String> = Vec::new();
        for f in findings {
            let t = f.type_label();
            if !detections.contains(&t) {
                detections.push(t);
            }
        }
        ActivityEntry {
            ts_ms: now_ms(),
            app_id: app.app_id.clone(),
            app_name: app.name.clone(),
            site: app.site(),
            channel,
            detections,
            outcome,
            files,
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct ActivityLog {
    path: Option<PathBuf>,
    lock: Mutex<()>,
}

impl ActivityLog {
    pub fn new(path: &Path) -> Self {
        ActivityLog { path: Some(path.to_path_buf()), lock: Mutex::new(()) }
    }

    /// A log that records nothing.
    pub fn disabled() -> Self {
        ActivityLog { path: None, lock: Mutex::new(()) }
    }

    pub fn append(&self, entry: &ActivityEntry) {
        let Some(path) = &self.path else { return };
        let _g = self.lock.lock();
        if let Err(e) = self.append_inner(path, entry) {
            tracing::warn!("activity log write failed: {e}");
        }
    }

    fn append_inner(&self, path: &Path, entry: &ActivityEntry) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
            std::fs::rename(path, rotated(path))?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        writeln!(f, "{line}")
    }

    /// Most recent entries, newest first.
    pub fn recent(&self, limit: usize) -> Vec<ActivityEntry> {
        let Some(path) = &self.path else { return Vec::new() };
        let _g = self.lock.lock();
        let mut out = Vec::new();
        for p in [path.clone(), rotated(path)] {
            let Ok(f) = std::fs::File::open(&p) else { continue };
            let mut entries: Vec<ActivityEntry> =
                BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
            entries.reverse();
            out.extend(entries);
            if out.len() >= limit {
                break;
            }
        }
        out.truncate(limit);
        out
    }

    pub fn clear(&self) {
        let Some(path) = &self.path else { return };
        let _g = self.lock.lock();
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(rotated(path));
    }
}

fn rotated(path: &Path) -> PathBuf {
    path.with_extension("1.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::Finding;

    #[test]
    fn entries_hold_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActivityLog::new(&dir.path().join("a.jsonl"));
        let app = AppContext {
            app_id: "com.google.Chrome".into(),
            name: "Google Chrome".into(),
            pid: 7,
            url: Some("https://chatgpt.com/c/secret-conversation-id".into()),
            window_title: Some("my private title".into()),
        };
        let f = vec![Finding::keyword("project falcon", 0..14, "plan.docx"), Finding::pii("EMAIL_ADDRESS", 0..3, 0.9, "m")];
        log.append(&ActivityEntry::new(&app, Channel::Drop, &f, Outcome::DropBlocked, 1));
        let raw = std::fs::read_to_string(dir.path().join("a.jsonl")).unwrap();
        for leaked in ["falcon", "plan.docx", "secret-conversation-id", "private title"] {
            assert!(!raw.contains(leaked), "log leaked {leaked}: {raw}");
        }
        let recent = log.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].site.as_deref(), Some("chatgpt.com"));
        assert_eq!(recent[0].detections, vec!["keyword", "email_address"]);
    }

    #[test]
    fn recent_is_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActivityLog::new(&dir.path().join("a.jsonl"));
        let app = AppContext { app_id: "x".into(), name: "X".into(), ..Default::default() };
        for outcome in [Outcome::Allowed, Outcome::Blocked, Outcome::Cancelled] {
            log.append(&ActivityEntry::new(&app, Channel::Message, &[], outcome, 0));
        }
        let r = log.recent(2);
        assert_eq!(r.iter().map(|e| e.outcome).collect::<Vec<_>>(), vec![Outcome::Cancelled, Outcome::Blocked]);
        log.clear();
        assert!(log.recent(10).is_empty());
    }
}
