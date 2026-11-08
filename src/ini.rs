//! The `[section]` + `key=value` files the clients already read, Postgres'
//! `~/.pg_service.conf` and MySQL's `~/.my.cnf`, and the ones easysql keeps for
//! itself in the same shape. One reader and one set of block surgeons serve
//! all of them.
//!
//! We parse just enough to list and to rewrite the one section we are
//! touching. Every other line, your comments, blank lines, ordering and line
//! endings included, is copied through byte for byte, and the file is replaced
//! whole, so a crash mid-write never leaves half of it.

use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a file's own client reads it, where the clients differ.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// libpq's service file and easysql's own files: of two lines setting the
    /// same key the first wins, and a value is the rest of the line.
    Plain,
    /// MySQL's option files: the last of two lines wins, and a value may be
    /// quoted and followed by a `#` comment.
    MySql,
}

impl Dialect {
    /// Whether two values as written mean the same thing to the client, so a
    /// line whose meaning is unchanged is left exactly as it was written.
    fn same(self, a: &str, b: &str) -> bool {
        match self {
            Dialect::Plain => a == b,
            Dialect::MySql => {
                crate::engines::mysql::option_read(a) == crate::engines::mysql::option_read(b)
            }
        }
    }
}

/// One `[name]` block and the keys under it, plus where it sits in the file so
/// an edit can replace exactly those lines and nothing else.
pub struct Section {
    pub name: String,
    /// Each key once, with the value its client acts on, as written.
    pub keys: Vec<(String, String)>,
    /// Line index of the `[name]` header.
    at: usize,
    /// One past the block's last key line. A comment or a blank line after it
    /// is the next block's, or the file's, so neither a rewrite nor a delete
    /// takes it.
    end: usize,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Everything except the keys named in `known`, in file order. This is what
    /// keeps a `sslmode` or a `connect_timeout` we have no field for from being
    /// silently dropped the first time the wizard rewrites the section.
    pub fn rest(&self, known: &[&str]) -> Vec<(String, String)> {
        self.keys
            .iter()
            .filter(|(k, _)| !known.iter().any(|n| n.eq_ignore_ascii_case(k)))
            .cloned()
            .collect()
    }
}

/// What one line of the file is.
enum Kind<'a> {
    Header(String),
    Key(&'a str, &'a str),
    /// A blank line, a comment, or MySQL's `!include` and `!includedir`.
    Other,
}

fn classify(line: &str) -> Kind<'_> {
    let t = line.trim();
    if t.is_empty() || t.starts_with(['#', ';', '!']) {
        return Kind::Other;
    }
    if let Some(name) = header(t) {
        return Kind::Header(name);
    }
    // A bare flag (`no-auto-rehash`) is a valid my.cnf line; it is kept with an
    // empty value so a rewrite does not lose it.
    match t.split_once('=') {
        Some((k, v)) => Kind::Key(k.trim(), v.trim()),
        None => Kind::Key(t, ""),
    }
}

/// `[name]` -> `name`, for a header that is actually closed. Whatever follows
/// the `]` is ignored, as both libpq and mysql ignore it, so `[b] # staging`
/// is the header of `b`.
fn header(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?;
    let close = inner.find(']')?;
    Some(inner[..close].trim().to_string())
}

#[cfg(test)]
/// Parse INI text the way libpq reads it. A key before any header belongs to
/// no section and is dropped.
pub fn parse(text: &str) -> Vec<Section> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    parse_lines(&lines, Dialect::Plain)
}

fn parse_lines(lines: &[String], d: Dialect) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        match classify(line) {
            Kind::Header(name) => out.push(Section {
                name,
                keys: Vec::new(),
                at: i,
                end: i + 1,
            }),
            Kind::Key(key, value) => {
                // a key with no section above it belongs to nothing
                let Some(sec) = out.last_mut() else {
                    continue;
                };
                sec.end = i + 1;
                match sec
                    .keys
                    .iter()
                    .position(|(k, _)| k.eq_ignore_ascii_case(key))
                {
                    Some(at) if d == Dialect::MySql => sec.keys[at].1 = value.to_string(),
                    Some(_) => {}
                    None => sec.keys.push((key.to_string(), value.to_string())),
                }
            }
            Kind::Other => {}
        }
    }
    out
}

/// A file as its lines: each one's bytes exactly as written, ending included,
/// and its text for parsing. A byte that is not UTF-8 is legal to libpq and
/// mysql, so the text is only ever used to find lines, never written back.
struct Doc {
    raw: Vec<Vec<u8>>,
    text: Vec<String>,
    /// The ending a new line gets: the file's own, `\r\n` or `\n`.
    nl: &'static str,
}

impl Doc {
    fn new(bytes: &[u8]) -> Doc {
        let mut raw: Vec<Vec<u8>> = bytes
            .split_inclusive(|b| *b == b'\n')
            .map(<[u8]>::to_vec)
            .collect();
        if raw.last().is_some_and(Vec::is_empty) {
            raw.pop();
        }
        let text = raw
            .iter()
            .map(|l| {
                let l = l.strip_suffix(b"\n").unwrap_or(l);
                let l = l.strip_suffix(b"\r").unwrap_or(l);
                String::from_utf8_lossy(l).into_owned()
            })
            .collect();
        let nl = match raw.first().is_some_and(|l| l.ends_with(b"\r\n")) {
            true => "\r\n",
            false => "\n",
        };
        Doc { raw, text, nl }
    }

    fn line(&self, text: String) -> Vec<u8> {
        let mut l = text.into_bytes();
        l.extend_from_slice(self.nl.as_bytes());
        l
    }
}

/// The file's bytes, empty when it does not exist yet. Any other failure is an
/// error, never an empty file: a file we could not read must not be rewritten
/// as though it held nothing.
fn read_raw(path: &Path) -> Result<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn read(path: &Path) -> Vec<Section> {
    read_as(path, Dialect::Plain)
}

/// The sections of `path` as its client reads them; a missing or unreadable
/// file lists nothing.
pub fn read_as(path: &Path, d: Dialect) -> Vec<Section> {
    parse_lines(&Doc::new(&fs::read(path).unwrap_or_default()).text, d)
}

/// The value of `key` in `[name]` exactly as written after its `=`, edge
/// spaces included, for a value whose spaces are its own: a password easysql
/// keeps, which a trimmed read would hand over changed.
pub fn raw_value(path: &Path, name: &str, key: &str) -> Option<String> {
    let doc = Doc::new(&fs::read(path).ok()?);
    let sec = parse_lines(&doc.text, Dialect::Plain)
        .into_iter()
        .find(|s| s.name == name)?;
    (sec.at + 1..sec.end).find_map(|i| match classify(&doc.text[i]) {
        Kind::Key(k, _) if k.eq_ignore_ascii_case(key) => {
            doc.text[i].split_once('=').map(|(_, v)| v.to_string())
        }
        _ => None,
    })
}

/// The keys of `[name]` whose line, the one the client acts on, has no `=` at
/// all (`no-auto-rehash`): `Section::keys` reads their value as empty, and an
/// edit that leaves them alone keeps them bare.
pub fn bare_keys(path: &Path, d: Dialect, name: &str) -> Vec<String> {
    let doc = Doc::new(&fs::read(path).unwrap_or_default());
    let Some(sec) = parse_lines(&doc.text, d)
        .into_iter()
        .find(|s| s.name == name)
    else {
        return Vec::new();
    };
    let mut seen: Vec<(String, bool)> = Vec::new();
    for i in sec.at + 1..sec.end {
        let Kind::Key(k, _) = classify(&doc.text[i]) else {
            continue;
        };
        let bare = !doc.text[i].contains('=');
        match seen.iter_mut().find(|(s, _)| s.eq_ignore_ascii_case(k)) {
            Some(last) if d == Dialect::MySql => last.1 = bare,
            Some(_) => {}
            None => seen.push((k.to_string(), bare)),
        }
    }
    seen.into_iter()
        .filter_map(|(k, bare)| bare.then_some(k))
        .collect()
}

/// A name or a value with a line break in it would be written as lines of its
/// own: a new `[section]`, or a key in somebody else's.
fn one_line(path: &Path, name: &str, keys: &[(String, String)]) -> Result<()> {
    let broken = |s: &str| s.contains(['\n', '\r']);
    if broken(name) || name.contains(']') || keys.iter().any(|(k, v)| broken(k) || broken(v)) {
        bail!(
            "a line break, or a `]` in a name, cannot be written into {}: it would start a line of its own",
            path.display()
        );
    }
    Ok(())
}

pub fn upsert(
    path: &Path,
    original: Option<&str>,
    name: &str,
    keys: &[(String, String)],
) -> Result<()> {
    upsert_as(path, Dialect::Plain, original, name, keys)
}

/// Write `keys` as section `name`: changing that section in place if it is
/// there, appending it if it is not. `original` is the name to replace, which
/// differs from `name` when the section is renamed, and a rename onto a name
/// that is already taken is refused. In place means a key whose value is
/// unchanged keeps its line as written, a key not in `keys` loses its line, a
/// new key goes after the block's last one, and a comment inside the block
/// stays where it is.
pub fn upsert_as(
    path: &Path,
    d: Dialect,
    original: Option<&str>,
    name: &str,
    keys: &[(String, String)],
) -> Result<()> {
    one_line(path, name, keys)?;
    let doc = Doc::new(&read_raw(path)?);
    let sections = parse_lines(&doc.text, d);
    let target = original.unwrap_or(name);
    if target != name && sections.iter().any(|s| s.name == name) {
        bail!(
            "[{name}] is already in {}, so [{target}] cannot take its name",
            path.display()
        );
    }

    let mut out: Vec<Vec<u8>> = Vec::new();
    match sections.iter().find(|s| s.name == target) {
        Some(sec) => {
            backup(path)?;
            out.extend(doc.raw[..sec.at].iter().cloned());
            out.extend(rewrite(&doc, sec, d, name, keys));
            out.extend(doc.raw[sec.end..].iter().cloned());
        }
        None => {
            if !doc.raw.is_empty() {
                backup(path)?;
            }
            out.extend(doc.raw.iter().cloned());
            if doc.text.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(doc.line(String::new()));
            }
            out.push(doc.line(format!("[{name}]")));
            out.extend(keys.iter().map(|(k, v)| doc.line(format!("{k}={v}"))));
        }
    }
    write_lines(path, out, &doc)
}

/// The block `sec` with `keys` written into it in place.
fn rewrite(
    doc: &Doc,
    sec: &Section,
    d: Dialect,
    name: &str,
    keys: &[(String, String)],
) -> Vec<Vec<u8>> {
    let lines = sec.at + 1..sec.end;
    let key_at = |i: usize| match classify(&doc.text[i]) {
        Kind::Key(k, v) => Some((k, v)),
        _ => None,
    };
    // The one line a key's value is taken from, which a change has to land on.
    let effective = |key: &str| {
        let mut hits = lines
            .clone()
            .filter(|&i| key_at(i).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)));
        match d {
            Dialect::Plain => hits.next(),
            Dialect::MySql => hits.next_back(),
        }
    };

    let mut out = vec![match sec.name == name {
        true => doc.raw[sec.at].clone(),
        false => doc.line(format!("[{name}]")),
    }];
    let mut placed: Vec<&str> = Vec::new();
    for i in lines.clone() {
        let Some((key, value)) = key_at(i) else {
            out.push(doc.raw[i].clone());
            continue;
        };
        let Some((new_key, new_value)) = keys.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))
        else {
            continue;
        };
        if effective(key) != Some(i) {
            out.push(doc.raw[i].clone());
            continue;
        }
        placed.push(new_key);
        match d.same(value, new_value) {
            true => out.push(doc.raw[i].clone()),
            false => out.push(doc.line(format!("{key}={new_value}"))),
        }
    }
    for (k, v) in keys {
        if !placed.iter().any(|p| p.eq_ignore_ascii_case(k)) {
            out.push(doc.line(format!("{k}={v}")));
        }
    }
    out
}

/// Drop a whole section, and the blank line above it, so repeated add/delete
/// cycles do not pad the file out.
pub fn remove(path: &Path, name: &str) -> Result<()> {
    let doc = Doc::new(&fs::read(path).with_context(|| format!("reading {}", path.display()))?);
    let Some(sec) = parse_lines(&doc.text, Dialect::Plain)
        .into_iter()
        .find(|s| s.name == name)
    else {
        bail!("no [{name}] in {}", path.display());
    };
    backup(path)?;
    let blank = |i: usize| doc.text[i].trim().is_empty();
    let mut start = sec.at;
    if start > 0 && blank(start - 1) {
        start -= 1;
    }
    // At the top of the file there is no blank before it to take, so take the
    // one after it, or the next section starts the file with an empty line.
    let mut end = sec.end;
    while start == 0 && end < doc.raw.len() && blank(end) {
        end += 1;
    }
    let out = doc.raw[..start]
        .iter()
        .chain(doc.raw[end..].iter())
        .cloned()
        .collect();
    write_lines(path, out, &doc)
}

#[cfg(test)]
pub fn set_key(path: &Path, name: &str, key: &str, value: Option<&str>) -> Result<()> {
    set_key_as(path, Dialect::Plain, name, key, value)
}

/// Set (or clear, with `None`) one key inside a section that already exists.
/// Used for a MySQL group's password, where the rest of the group is not ours
/// to rewrite.
pub fn set_key_as(
    path: &Path,
    d: Dialect,
    name: &str,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    let doc = Doc::new(&read_raw(path)?);
    let sections = parse_lines(&doc.text, d);
    let Some(sec) = sections.iter().find(|s| s.name == name) else {
        bail!("no [{name}] in {}", path.display());
    };
    let mut keys: Vec<(String, String)> = sec
        .keys
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case(key))
        .cloned()
        .collect();
    if let Some(v) = value {
        keys.push((key.to_string(), v.to_string()));
    }
    upsert_as(path, d, None, name, &keys)
}

/// Every line ends in a line break, the last one included.
fn write_lines(path: &Path, lines: Vec<Vec<u8>>, doc: &Doc) -> Result<()> {
    let mut body = Vec::new();
    for mut line in lines {
        if !line.ends_with(b"\n") {
            line.extend_from_slice(doc.nl.as_bytes());
        }
        body.extend(line);
    }
    write_atomic_bytes(path, &body)
}

/// The files that hold passwords. libpq refuses a `.pgpass` that others can
/// read and MySQL ignores a world-writable `.my.cnf`, so these are written
/// 0600 from the first byte; every other file keeps the mode it had.
const SECRET: [&str; 3] = [".pgpass", ".my.cnf", ".esqlpass"];

fn secret(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| SECRET.contains(&n))
}

/// Where a write to `path` lands: the file at the end of its symlinks, even
/// one that does not exist yet, so a dotfiles manager's link is written
/// through rather than replaced.
fn resolve_links(path: &Path) -> PathBuf {
    let mut at = path.to_path_buf();
    for _ in 0..40 {
        let is_link = fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink());
        let Some(next) = is_link.then(|| fs::read_link(&at).ok()).flatten() else {
            break;
        };
        at = match next.is_absolute() {
            true => next,
            false => at.parent().map(|d| d.join(&next)).unwrap_or(next),
        };
    }
    at
}

pub fn write_atomic(path: &Path, body: &str) -> Result<()> {
    write_atomic_bytes(path, body.as_bytes())
}

/// Replace a file's contents in one step: a temp file beside it, renamed over
/// it, so a crash mid-write leaves the old file rather than a truncated one. A
/// symlink is written through rather than replaced, the old file's mode
/// carries over to the new one, and a password file is 0600 from the moment
/// the temp file exists.
pub fn write_atomic_bytes(path: &Path, body: &[u8]) -> Result<()> {
    write_with(path, body, secret(path))
}

/// `write_atomic_bytes` for a password file whatever its name, as a
/// `$PGPASSFILE` can give `.pgpass` any.
pub fn write_secret_bytes(path: &Path, body: &[u8]) -> Result<()> {
    write_with(path, body, true)
}

fn write_with(path: &Path, body: &[u8], secret: bool) -> Result<()> {
    let target = resolve_links(path);
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".esql-tmp.{}", std::process::id()));
    let tmp = target.with_file_name(name);
    let written = (|| {
        // Left by a run that died mid-write; `create_new` would refuse it.
        let _ = fs::remove_file(&tmp);
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(body)?;
        file.sync_all()?;
        drop(file);
        if !secret && let Ok(meta) = fs::metadata(&target) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, &target)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written.with_context(|| format!("writing {}", path.display()))
}

/// How many copies of one file `backup` keeps, 0 meaning it copies nothing.
/// Zero until whoever loaded the settings says otherwise, so a test run never
/// leaves a copy behind.
static BACKUPS: AtomicUsize = AtomicUsize::new(0);

/// Apply the `backups` setting. Called wherever the settings are loaded or
/// changed, since the writes that back up never see the settings themselves.
pub fn set_backups(keep: usize) {
    BACKUPS.store(keep, Ordering::Relaxed);
}

pub fn backups_on() -> bool {
    BACKUPS.load(Ordering::Relaxed) > 0
}

/// The tail a status line gets after a rewrite, which is nothing when no copy
/// was taken.
pub fn backup_note() -> &'static str {
    if backups_on() {
        " (backed up first)"
    } else {
        ""
    }
}

/// `~/.local/state/easysql/backups`, out of the home folder where the files
/// themselves live.
pub fn backup_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"))
        .join("easysql")
        .join("backups")
}

/// Copy the file into `backup_dir()` as `<name>.<epoch>` before an in-place
/// edit and keep only the newest copies of it, as many as the `backups`
/// setting says. With it off, do nothing.
pub fn backup(path: &Path) -> Result<()> {
    let keep = BACKUPS.load(Ordering::Relaxed);
    if keep == 0 || !path.exists() {
        return Ok(());
    }
    backup_into(path, &backup_dir(), keep)
}

fn backup_into(path: &Path, dir: &Path, keep: usize) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    // `.pgpass` is kept as `pgpass.<epoch>`, so the folder lists without `-a`.
    let stem = path.file_name().unwrap_or_default().to_string_lossy();
    let stem = stem.trim_start_matches('.');
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dest = dir.join(format!("{stem}.{secs}"));
    fs::copy(path, &dest).with_context(|| format!("backing up to {}", dest.display()))?;

    let prefix = format!("{stem}.");
    let mut copies: Vec<(u64, PathBuf)> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let secs = name.strip_prefix(&prefix)?.parse::<u64>().ok()?;
            Some((secs, e.path()))
        })
        .collect();
    copies.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, old) in copies.into_iter().skip(keep) {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

/// Expand a leading `~`. Children are spawned without a shell, so a typed
/// `~/db.sqlite` would otherwise reach sqlite3 as a directory named `~`.
pub fn expand_tilde(path: &str) -> PathBuf {
    let home = || dirs::home_dir().unwrap_or_default();
    match path {
        "~" => home(),
        _ => match path.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(path),
        },
    }
}

/// The inverse, for display only: a path under home is shown the way you would
/// have written it. Never feed the result back to a command.
pub fn collapse_tilde(path: &str) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let home = home.to_string_lossy();
    match path.strip_prefix(&*home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway file under the temp dir that deletes itself, so a test run
    /// leaves the machine exactly as it found it.
    struct Temp(PathBuf);

    impl Temp {
        fn new(body: &str) -> Temp {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("easysql-ini-{}-{stamp}.conf", std::process::id()));
            fs::write(&path, body).unwrap();
            Temp(path)
        }

        fn read(&self) -> String {
            fs::read_to_string(&self.0).unwrap()
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn parses_sections_and_reads_keys_case_insensitively() {
        let sections = parse(
            "\
# a comment nobody may touch
[prod]
host=db.example.com
Port=5432

[staging]
host=stage.example.com
",
        );
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "prod");
        assert_eq!(sections[0].get("host"), Some("db.example.com"));
        // libpq and mysql both read keys case-insensitively, so we must too.
        assert_eq!(sections[0].get("port"), Some("5432"));
        assert_eq!(sections[1].get("host"), Some("stage.example.com"));
    }

    #[test]
    fn duplicate_key_keeps_the_first_and_a_key_before_any_header_is_dropped() {
        // libpq takes the first value; a key above the first `[header]`
        // belongs to no section, and inventing one for it would write it back
        // somewhere it was never meant to be.
        let sections = parse("stray=1\n[prod]\nhost=a\nhost=b\n");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].get("host"), Some("a"));
        assert_eq!(sections[0].get("stray"), None);
    }

    #[test]
    fn rest_hands_back_the_keys_no_wizard_field_owns() {
        // This is what stops a hand-written `connect_timeout` from vanishing
        // the first time somebody edits the connection in the wizard.
        let sections = parse("[prod]\nhost=a\nsslmode=require\nconnect_timeout=3\n");
        let rest = sections[0].rest(&["host", "port", "dbname", "user"]);
        assert_eq!(
            rest,
            vec![
                ("sslmode".to_string(), "require".to_string()),
                ("connect_timeout".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn upsert_rewrites_one_section_and_leaves_the_rest_verbatim() {
        let f = Temp::new(
            "\
# hand-written, and not ours to reformat
[keep]
host = untouched.example.com
connect_timeout=9

[prod]
host=old.example.com
",
        );
        upsert(
            &f.0,
            None,
            "prod",
            &[
                ("host".into(), "new.example.com".into()),
                ("port".into(), "5432".into()),
            ],
        )
        .unwrap();

        let out = f.read();
        assert!(out.contains("# hand-written, and not ours to reformat"));
        assert!(out.contains("host = untouched.example.com"));
        assert!(out.contains("connect_timeout=9"));
        assert!(out.contains("new.example.com"));
        assert!(!out.contains("old.example.com"));
    }

    #[test]
    fn upsert_renames_in_place_rather_than_leaving_both() {
        let f = Temp::new("[old]\nhost=a\n");
        upsert(&f.0, Some("old"), "new", &[("host".into(), "a".into())]).unwrap();
        let out = f.read();
        assert!(out.contains("[new]"));
        assert!(!out.contains("[old]"));
    }

    #[test]
    fn remove_takes_the_blank_line_above_it_too() {
        // Otherwise repeated add/delete cycles pad the file out with blanks.
        let f = Temp::new("[keep]\nhost=a\n\n[drop]\nhost=b\n");
        remove(&f.0, "drop").unwrap();
        assert_eq!(f.read(), "[keep]\nhost=a\n");
    }

    #[test]
    fn set_key_sets_and_clears_without_disturbing_the_group() {
        let f = Temp::new("[clientprod]\nhost=a\nuser=me\n");
        set_key(&f.0, "clientprod", "password", Some("s3cret")).unwrap();
        let out = f.read();
        assert!(out.contains("password=s3cret"));
        assert!(out.contains("user=me"));

        set_key(&f.0, "clientprod", "password", None).unwrap();
        let out = f.read();
        assert!(!out.contains("s3cret"));
        assert!(out.contains("user=me"));
    }

    #[test]
    fn tilde_expands_and_collapses_only_for_the_current_user() {
        let home = dirs::home_dir().unwrap_or_default();
        assert_eq!(expand_tilde("~/db.sqlite"), home.join("db.sqlite"));
        // Children run without a shell, so anything we do not expand reaches
        // sqlite3 as a directory literally named `~`.
        assert_eq!(
            expand_tilde("/srv/db.sqlite"),
            PathBuf::from("/srv/db.sqlite")
        );
        assert_eq!(expand_tilde("~root/db"), PathBuf::from("~root/db"));
        assert_eq!(
            collapse_tilde(&home.join("db.sqlite").to_string_lossy()),
            "~/db.sqlite"
        );
        assert_eq!(collapse_tilde("/srv/db.sqlite"), "/srv/db.sqlite");
    }

    /// A throwaway directory under the temp dir that deletes itself and
    /// everything in it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("easysql-{tag}-{}-{stamp}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn backups_keep_only_the_newest_n_copies_of_a_file() {
        let tmp = TempDir::new("backups");
        let file = tmp.0.join(".pgpass");
        fs::write(&file, "now").unwrap();
        let dir = tmp.0.join("backups");
        fs::create_dir_all(&dir).unwrap();
        for old in ["pgpass.100", "pgpass.200", "pgpass.300", "psqlrc.100"] {
            fs::write(dir.join(old), "old").unwrap();
        }

        backup_into(&file, &dir, 2).unwrap();

        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let pgpass: Vec<&String> = left.iter().filter(|n| n.starts_with("pgpass.")).collect();
        assert_eq!(
            pgpass.len(),
            2,
            "the copy just taken and the newest old one, got {left:?}"
        );
        assert!(
            pgpass.iter().any(|n| *n == "pgpass.300"),
            "the newest old copy should survive, got {left:?}"
        );
        assert!(
            left.iter().any(|n| n == "psqlrc.100"),
            "another file's copies are not this file's to prune, got {left:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_rewrite_through_a_symlink_keeps_the_link() {
        let tmp = TempDir::new("symlink");
        let target = tmp.0.join("dotfiles-psqlrc");
        let link = tmp.0.join(".psqlrc");
        fs::write(&target, "old\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_atomic(&link, "new\n").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link a dotfiles manager made should still be a link"
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "new\n",
            "the write should land in the file the link points at"
        );
    }

    #[test]
    fn an_edit_copies_every_other_line_byte_for_byte() {
        // Legal to both clients: a byte that is not UTF-8, CRLF endings, a
        // comment above the next header, and a header with a note after it.
        let t = Temp::new("");
        let before: &[u8] = b"# r\xe9plica\r\n[a]\r\nhost=a.example.com\r\n\r\n# staging\r\n[b] # old\r\nhost=b.example.com\r\n";
        fs::write(&t.0, before).unwrap();
        let keys = [
            ("host".to_string(), "a.example.com".to_string()),
            ("port".to_string(), "5433".to_string()),
        ];
        upsert(&t.0, Some("a"), "a", &keys).unwrap();
        let after = fs::read(&t.0).unwrap();
        let expected: &[u8] = b"# r\xe9plica\r\n[a]\r\nhost=a.example.com\r\nport=5433\r\n\r\n# staging\r\n[b] # old\r\nhost=b.example.com\r\n";
        assert_eq!(
            after,
            expected,
            "only the new key may change, got {:?}",
            String::from_utf8_lossy(&after)
        );
        let names: Vec<String> = read(&t.0).into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            ["a", "b"],
            "a header with a note after it is still a header"
        );
    }
}
