//! File pickers (NSOpenPanel, in-process or hosted by the open/save panel service): recognising their
//! confirm buttons and reading the current selection.

use crate::apps::{pids_with_bundle, PANEL_SERVICE};
use crate::ax::Element;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// First word of a picker's confirm button (English and a few common UI languages).
pub const PICK_WORDS: &[&str] =
    &["open", "upload", "choose", "attach", "select", "insert", "abrir", "ouvrir", "öffnen", "apri", "openen", "kies"];

/// Roles of file entries in the panel's list, column and icon views.
pub const ENTRY_ROLES: &[&str] = &["AXRow", "AXCell", "AXStaticText", "AXTextField", "AXImage"];

pub fn is_pick_label(label: &str) -> bool {
    let first = label.split(|c: char| !c.is_alphanumeric()).find(|t| !t.is_empty()).unwrap_or("");
    PICK_WORDS.contains(&first)
}

/// Is `el` part of a file picker? Cheap: looks only at the element's own window / sheet.
pub fn in_file_dialog(el: &Element, el_bundle_is_panel_service: bool) -> bool {
    if el_bundle_is_panel_service {
        return true;
    }
    let top = el.element("AXTopLevelUIElement").or_else(|| el.element("AXWindow"));
    let Some(top) = top else { return false };
    if let Some(def) = top.element("AXDefaultButton") {
        if is_pick_label(&def.title()) {
            return true;
        }
    }
    let ident = top.string("AXIdentifier").unwrap_or_default().to_lowercase();
    ident.contains("open-panel") || ident.contains("openpanel") || ident.contains("nsopenpanel")
}

/// Files selected in the picker shown by the app `pid` (or by the panel service on its behalf).
pub fn selection(pid: u32) -> Vec<PathBuf> {
    let mut pids = vec![pid];
    pids.extend(pids_with_bundle(PANEL_SERVICE));
    let mut names: Vec<String> = Vec::new();
    for p in pids {
        let (paths, n) = selection_in(p);
        if !paths.is_empty() {
            return dedup(paths);
        }
        names.extend(n);
    }
    dedup(names.iter().filter_map(|n| resolve_name(n)).collect())
}

fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

/// Paths (from `AXURL`) and, failing that, displayed names of the selected rows.
fn selection_in(pid: u32) -> (Vec<PathBuf>, Vec<String>) {
    let app = Element::application(pid, 0.3);
    let Some(focused) = app.element("AXFocusedUIElement") else { return (Vec::new(), Vec::new()) };
    let mut rows = focused.elements("AXSelectedRows");
    if rows.is_empty() {
        rows = focused.elements("AXSelectedChildren");
    }
    if rows.is_empty() && ENTRY_ROLES.contains(&focused.role().as_str()) {
        rows.push(focused.clone());
    }
    let mut paths = Vec::new();
    let mut names = Vec::new();
    for row in rows.iter().take(50) {
        if let Some(p) = row_url(row) {
            paths.push(p);
        } else if let Some(n) = row_name(row) {
            names.push(n);
        }
    }
    (paths, names)
}

fn row_url(row: &Element) -> Option<PathBuf> {
    let mut found = None;
    row.find(40, 4, &mut |e| match e.url("AXURL") {
        Some(u) if u.starts_with('/') => {
            found = Some(PathBuf::from(u));
            true
        }
        _ => false,
    });
    found
}

fn row_name(row: &Element) -> Option<String> {
    let mut found = None;
    row.find(40, 4, &mut |e| {
        let v = e.string("AXValue").or_else(|| e.string("AXTitle")).unwrap_or_default();
        if v.trim().is_empty() {
            false
        } else {
            found = Some(v.trim().to_string());
            true
        }
    });
    found
}

/// Find a displayed file name on disk: common folders first, then Spotlight (exact name, unique hit).
fn resolve_name(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.exists().then_some(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    for dir in ["Desktop", "Downloads", "Documents", ""] {
        let p = home.join(dir).join(name);
        if p.exists() {
            return Some(p);
        }
    }
    spotlight_unique(&home, name)
}

fn spotlight_unique(home: &Path, name: &str) -> Option<PathBuf> {
    let mut child = Command::new("/usr/bin/mdfind")
        .arg("-onlyin")
        .arg(home)
        .arg("-name")
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_millis(600);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let hits: Vec<&str> =
        text.lines().filter(|l| Path::new(l).file_name().is_some_and(|f| f.to_string_lossy() == name)).collect();
    (hits.len() == 1).then(|| PathBuf::from(hits[0]))
}
