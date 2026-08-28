//! Which connections you actually open. Every connect is stamped here so the
//! Connections tab can lead with the database you were in ten minutes ago
//! instead of whatever the alphabet put first - the list you want is the list
//! you used.
//!
//! The store is one small text file, `<state dir>/easysql/history`, with one
//! line per connection: `<epoch> <count> <engine>:<name>`. No dependency, no
//! schema, and a corrupt or missing line just drops that entry back to "never
//! connected".

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What we remember about one alias.
#[derive(Clone, Copy, Default)]
pub struct Entry {
    /// Epoch seconds of the most recent connect.
    pub last: u64,
    /// How many connects we have seen in total.
    pub count: u32,
}

/// The whole file, keyed by `<engine>:<name>`.
#[derive(Default)]
pub struct History {
    entries: HashMap<String, Entry>,
}

impl History {
    pub fn get(&self, key: &str) -> Option<Entry> {
        self.entries.get(key).copied()
    }

    /// Sort key for the Connections list: most recently opened first, then every
    /// connection you have never used, alphabetically. Returning the negated
    /// stamp keeps it a plain ascending sort at the call site.
    pub fn rank(&self, key: &str) -> (i64, String) {
        let last = self.get(key).map(|e| e.last).unwrap_or(0);
        (-(last as i64), key.to_string())
    }
}

/// `~/.local/state/easysql/history` (falling back to the data dir on platforms
/// with no state dir). Connect history is state, not config: losing it costs
/// you an ordering, nothing more.
pub fn path() -> PathBuf {
    let base = dirs::state_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"));
    base.join("easysql").join("history")
}

pub fn load() -> History {
    load_from(&path())
}

/// Stamp a connect. Best-effort: a history we cannot write is not worth
/// interrupting a connect over, so every failure here is silent.
pub fn record(key: &str) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = record_in(&p, key, now());
}

/// Move a connection's record to its new name. Keyed on `Conn::key()` like
/// everything else, so a rename would otherwise read as a connection you have
/// never opened and drop it to the bottom of the list. Best-effort, for the same
/// reason `record` is: a lost stamp is not worth failing a save over.
pub fn rename(old_key: &str, new_key: &str) {
    let _ = rename_in(&path(), old_key, new_key);
}

pub fn rename_in(path: &Path, old_key: &str, new_key: &str) -> std::io::Result<()> {
    if old_key == new_key {
        return Ok(());
    }
    let mut h = load_from(path);
    let Some(e) = h.entries.remove(old_key) else {
        return Ok(());
    };
    // If the new name already has a record, keep the livelier of the two rather
    // than throwing one away: both were really opened.
    let slot = h.entries.entry(new_key.to_string()).or_default();
    slot.last = slot.last.max(e.last);
    slot.count += e.count;
    write_all(path, &h)
}

/// Parse the store. Unreadable file, unparseable line: both mean "no history",
/// never an error the caller has to handle.
pub fn load_from(path: &Path) -> History {
    let mut entries = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return History { entries };
    };
    for line in text.lines() {
        // `<epoch> <count> <key>`; the key is last because a service name and
        // an option group cannot contain whitespace, so it needs no quoting.
        let mut f = line.split_whitespace();
        let (Some(last), Some(count), Some(key)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let (Ok(last), Ok(count)) = (last.parse(), count.parse()) else {
            continue;
        };
        entries.insert(key.to_string(), Entry { last, count });
    }
    History { entries }
}

/// Read-modify-write one alias. Takes the path and the clock so tests can drive
/// it against a temp file without touching the real state dir.
pub fn record_in(path: &Path, key: &str, at: u64) -> std::io::Result<()> {
    let mut h = load_from(path);
    let e = h.entries.entry(key.to_string()).or_default();
    e.last = at;
    e.count += 1;

    write_all(path, &h)
}

/// Sorted output so the file is diffable and the order never depends on the hash
/// map's iteration order.
fn write_all(path: &Path, h: &History) -> std::io::Result<()> {
    let mut rows: Vec<(&String, &Entry)> = h.entries.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    let body: String = rows
        .iter()
        .map(|(key, e)| format!("{} {} {}\n", e.last, e.count, key))
        .collect();
    fs::write(path, body)
}

/// Epoch seconds now, or 0 if the clock is before the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "2d ago" - a coarse, glanceable age. Precision past the largest unit is
/// noise here: you want to know whether you were on a box today or last month.
pub fn ago(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        86400..=2591999 => format!("{}d ago", secs / 86400),
        2592000..=31535999 => format!("{}mo ago", secs / 2592000),
        _ => format!("{}y ago", secs / 31536000),
    }
}
