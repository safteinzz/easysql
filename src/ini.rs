//! The `[section]` + `key=value` files two of the three engines already use:
//! Postgres' `~/.pg_service.conf` and MySQL's `~/.my.cnf`. One reader and one
//! set of block surgeons serve both.
//!
//! The same promise `~/.ssh/config` gets from easyssh: we parse just enough to
//! list and to rewrite the one section we are touching. Every other line, your
//! comments and your ordering included, is copied through untouched, and any
//! write backs the file up first.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One `[name]` block and the keys under it, plus where it sits in the file so
/// an edit can replace exactly those lines and nothing else.
pub struct Section {
    pub name: String,
    pub keys: Vec<(String, String)>,
    /// Line index of the `[name]` header.
    pub at: usize,
    /// One past the block's last line, trailing blanks excluded.
    pub end: usize,
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

/// Parse INI text. A key before any header belongs to no section and is
/// dropped; a duplicate key keeps the first value, which is what both clients do.
pub fn parse(text: &str) -> Vec<Section> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Section> = Vec::new();

    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = header(line) {
            // The previous section ends where this one starts.
            if let Some(prev) = out.last_mut() {
                prev.end = trim_back(&lines, prev.at + 1, i);
            }
            out.push(Section {
                name,
                keys: Vec::new(),
                at: i,
                end: lines.len(),
            });
            continue;
        }
        let Some(sec) = out.last_mut() else {
            continue; // a key with no section above it belongs to nothing
        };
        // A bare flag (`no-auto-rehash`) is a valid my.cnf line; keep it with an
        // empty value so a rewrite does not lose it.
        let (key, value) = match line.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (line.to_string(), String::new()),
        };
        if !sec.keys.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            sec.keys.push((key, value));
        }
    }
    if let Some(last) = out.last_mut() {
        last.end = trim_back(&lines, last.at + 1, lines.len());
    }
    out
}

/// `[name]` -> `name`, for a header that is actually closed.
fn header(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim().to_string())
}

/// Walk `end` back past blank lines, so deleting a section does not leave a
/// growing gap and replacing one does not swallow the separator below it.
fn trim_back(lines: &[&str], floor: usize, mut end: usize) -> usize {
    while end > floor && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    end
}

pub fn read(path: &Path) -> Vec<Section> {
    parse(&fs::read_to_string(path).unwrap_or_default())
}

/// Write `keys` as section `name`: replacing that section in place if it is
/// there, appending it if it is not. `original` is the name to replace, which
/// differs from `name` when the wizard renamed the section.
pub fn upsert(
    path: &Path,
    original: Option<&str>,
    name: &str,
    keys: &[(String, String)],
) -> Result<()> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let target = original.unwrap_or(name);
    let found = parse(&text).into_iter().find(|s| s.name == target);

    let mut out: Vec<String> = Vec::new();
    match found {
        Some(sec) => {
            backup(path)?;
            out.extend(lines[..sec.at].iter().map(|l| l.to_string()));
            out.extend(render(name, keys));
            out.extend(lines[sec.end..].iter().map(|l| l.to_string()));
        }
        None => {
            if path.exists() {
                backup(path)?;
            }
            out.extend(lines.iter().map(|l| l.to_string()));
            if !out.is_empty() {
                out.push(String::new()); // a blank line separates the new block
            }
            out.extend(render(name, keys));
        }
    }
    write_lines(path, &out)
}

/// Drop a whole section, and the blank line above it, so repeated add/delete
/// cycles do not pad the file out.
pub fn remove(path: &Path, name: &str) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let Some(sec) = parse(&text).into_iter().find(|s| s.name == name) else {
        anyhow::bail!("no [{name}] in {}", path.display());
    };
    backup(path)?;
    let mut start = sec.at;
    if start > 0 && lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    let out: Vec<String> = lines[..start]
        .iter()
        .chain(lines[sec.end..].iter())
        .map(|l| l.to_string())
        .collect();
    write_lines(path, &out)
}

/// Set (or clear, with `None`) one key inside a section that already exists.
/// Used for a MySQL group's password, where the rest of the group is not ours
/// to rewrite.
pub fn set_key(path: &Path, name: &str, key: &str, value: Option<&str>) -> Result<()> {
    let sections = read(path);
    let Some(sec) = sections.iter().find(|s| s.name == name) else {
        anyhow::bail!("no [{name}] in {}", path.display());
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
    upsert(path, None, name, &keys)
}

fn render(name: &str, keys: &[(String, String)]) -> Vec<String> {
    let mut v = vec![format!("[{name}]")];
    for (k, val) in keys {
        if val.is_empty() {
            v.push(k.clone()); // a bare flag stays a bare flag
        } else {
            v.push(format!("{k}={val}"));
        }
    }
    v
}

fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    harden(path);
    Ok(())
}

/// Copy the file aside as `<name>.bak.<epoch>` before any in-place edit, so a
/// wrong answer in a wizard is never the last copy of your connection list.
pub fn backup(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".bak.{secs}"));
    let dest: PathBuf = path.with_file_name(name);
    fs::copy(path, &dest).with_context(|| format!("backing up to {}", dest.display()))?;
    Ok(())
}

/// These files hold, or sit beside, credentials, and MySQL flatly ignores a
/// world-readable `.my.cnf`. Best effort: a failure here is not worth stopping
/// a write over.
pub fn harden(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
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
        Some(rest) if !home.is_empty() => format!("~{rest}"),
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
            // Every write leaves a `<name>.bak.<epoch>` beside it.
            if let Some(dir) = self.0.parent() {
                let stem = format!("{}.bak.", self.0.file_name().unwrap().to_string_lossy());
                if let Ok(entries) = fs::read_dir(dir) {
                    for e in entries.flatten() {
                        if e.file_name().to_string_lossy().starts_with(&stem) {
                            let _ = fs::remove_file(e.path());
                        }
                    }
                }
            }
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
        // Both clients take the first value; a key above the first `[header]`
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
}
