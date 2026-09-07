//! Saved queries, kept as plain `.sql` files and run against any connection.
//!
//! psql can already do this with `\set name '…'` in `~/.psqlrc`, which is why
//! `:slots` is the shape people expect - but `\set` exists in psql alone, and
//! `mysql`, `sqlite3` and `sqlcmd` have nothing like it. So the shortcut lives
//! one level out, at `esql <connection> :<snippet>`, where easysql already
//! decides what runs and can hand each client the flag it wants.
//!
//! A snippet is a file rather than a row in a config: SQL is multi-line, an
//! editor is the right tool for it, and `o` opening `$EDITOR` on a real `.sql`
//! is a better experience than any field this program could draw. The name is
//! the file stem.
//!
//! Nothing here is engine-specific. A query that only makes sense on Postgres
//! simply fails on MySQL, and the server says so far better than easysql could
//! by guessing which dialect a file is written in.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub name: String,
    pub sql: String,
    pub path: PathBuf,
}

impl Snippet {
    /// The first line worth showing in a list, collapsed to one line.
    pub fn summary(&self) -> String {
        let one: String = self
            .sql
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("--"))
            .collect::<Vec<_>>()
            .join(" ");
        one
    }
}

pub fn dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("easysql")
        .join("snippets")
}

pub fn list() -> Vec<Snippet> {
    list_in(&dir())
}

pub fn list_in(dir: &std::path::Path) -> Vec<Snippet> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Snippet> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension()? != "sql" {
                return None;
            }
            Some(Snippet {
                name: path.file_stem()?.to_string_lossy().into_owned(),
                sql: fs::read_to_string(&path).ok()?,
                path,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn get(name: &str) -> Option<Snippet> {
    list().into_iter().find(|s| s.name == name)
}

/// Write one, creating the directory on the way. The name is used as a file
/// stem, so anything that would escape the directory is refused rather than
/// sanitised: a snippet called `../../.bashrc` is a mistake worth stopping.
pub fn save(name: &str, sql: &str) -> Result<PathBuf> {
    save_in(&dir(), name, sql)
}

pub fn save_in(dir: &std::path::Path, name: &str, sql: &str) -> Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        anyhow::bail!("a snippet name cannot be empty, contain a slash, or start with a dot");
    }
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{name}.sql"));
    let body = match sql.ends_with('\n') {
        true => sql.to_string(),
        false => format!("{sql}\n"),
    };
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn delete(name: &str) -> Result<()> {
    let path = dir().join(format!("{name}.sql"));
    fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

// --------------------------------------------------------------------------
// the psql half
// --------------------------------------------------------------------------
//
// `\set tables '…'` in `~/.psqlrc` makes `:tables` expand at the prompt. The
// block is regenerated from the snippet files and never touches a line outside
// its markers, because a `.psqlrc` is usually somebody's own work.

const START: &str = "-- easysql:start (generated from ~/.config/easysql/snippets, do not edit)";
const END: &str = "-- easysql:end";

pub fn psqlrc_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".psqlrc")
}

/// One `\set` per snippet.
///
/// The SQL is flattened onto one line, because a `\set` value ends at the
/// newline. That is also why `--` comments have to go: on one line they would
/// swallow the rest of the query.
pub fn psqlrc_block(snips: &[Snippet]) -> String {
    let mut out = String::from(START);
    out.push('\n');
    for s in snips {
        let one = s.summary();
        if one.is_empty() {
            continue;
        }
        // psql's own escaping inside a single-quoted \set value.
        let escaped = one.replace('\\', "\\\\").replace('\'', "\\'");
        out.push_str(&format!("\\set {} '{}'\n", s.name, escaped));
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// Put that block into `~/.psqlrc`, replacing the previous one and copying
/// every other line through verbatim.
pub fn sync_psqlrc() -> Result<PathBuf> {
    let path = psqlrc_path();
    let block = psqlrc_block(&list());
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let body = splice(&existing, &block);
    // Nothing to do is the common case, and it has to cost nothing: this runs on
    // the way into every postgres session, so writing unconditionally would drop
    // a `.psqlrc.bak.<epoch>` beside it on every single connect.
    if body == existing {
        return Ok(path);
    }
    if path.exists() {
        crate::ini::backup(&path)?;
    }
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Replace the marked block, or append one if there is not already a block.
pub fn splice(existing: &str, block: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    let mut replaced = false;
    for line in existing.lines() {
        if line.starts_with(START) {
            inside = true;
            out.push(block.trim_end().to_string());
            replaced = true;
            continue;
        }
        if inside {
            if line.trim() == END {
                inside = false;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !replaced {
        if !out.is_empty() && !out.last().is_some_and(|l| l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(block.trim_end().to_string());
    }
    let mut body = out.join("\n");
    body.push('\n');
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A throwaway snippets directory that deletes itself. The real one is
    /// never opened by this suite.
    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Temp {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("easysql-snips-{}-{stamp}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Temp(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_name_that_would_escape_the_directory_is_refused() {
        // The name becomes a filename, so this is the difference between a
        // typo and writing over something in the user's home.
        let t = Temp::new();
        for bad in ["../../.bashrc", "a/b", "", "  ", ".hidden"] {
            assert!(
                save_in(&t.0, bad, "select 1;").is_err(),
                "should have refused {bad:?}"
            );
        }
        assert!(save_in(&t.0, "fine", "select 1;").is_ok());
    }

    #[test]
    fn the_psqlrc_block_is_one_set_per_snippet_with_quotes_escaped() {
        // psql refuses to start on a malformed rc file, so the escaping here is
        // load-bearing: a stray quote breaks every session, not just the
        // shortcut. `--` comments have to go too, because the value is one line
        // and a comment would swallow the rest of the query.
        let snips = vec![
            Snippet {
                name: "quoted".into(),
                sql: "select 'it''s fine';".into(),
                path: PathBuf::new(),
            },
            Snippet {
                name: "multi".into(),
                sql: "select 1\n -- a comment\n from x;".into(),
                path: PathBuf::new(),
            },
        ];
        let block = psqlrc_block(&snips);
        assert!(
            block.contains(r"\set quoted 'select \'it\'\'s fine\';'"),
            "{block}"
        );
        assert!(block.contains(r"\set multi 'select 1 from x;'"), "{block}");
        assert!(
            !block.contains("-- a comment"),
            "comments are stripped: {block}"
        );
    }

    #[test]
    fn splicing_keeps_the_users_own_lines_and_replaces_only_our_block() {
        let mine = "\\set PROMPT1 mine\n\\timing on\n";
        let first = splice(
            mine,
            &psqlrc_block(&[Snippet {
                name: "a".into(),
                sql: "select 1;".into(),
                path: PathBuf::new(),
            }]),
        );
        assert!(first.contains("\\set PROMPT1 mine"));
        assert!(first.contains("\\timing on"));
        assert!(first.contains("\\set a 'select 1;'"));

        // Splicing again replaces the block rather than stacking a second one,
        // which is what makes it safe to run on the way into every session.
        let second = splice(
            &first,
            &psqlrc_block(&[Snippet {
                name: "b".into(),
                sql: "select 2;".into(),
                path: PathBuf::new(),
            }]),
        );
        assert_eq!(second.matches(START).count(), 1, "{second}");
        assert!(second.contains("\\set b 'select 2;'"));
        assert!(
            !second.contains("\\set a "),
            "the old block is gone: {second}"
        );
        assert!(
            second.contains("\\timing on"),
            "and the user's lines survive"
        );

        // And it is idempotent: the same snippets in produce the same file out,
        // so nothing is rewritten and no backup is taken on an unchanged run.
        let again = splice(
            &second,
            &psqlrc_block(&[Snippet {
                name: "b".into(),
                sql: "select 2;".into(),
                path: PathBuf::new(),
            }]),
        );
        assert_eq!(again, second);
    }
}
