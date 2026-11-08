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
        let one: String = without_line_comments(&self.sql)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        one
    }
}

/// `sql` with every `--` comment cut to the end of its line, except inside a
/// '…' string (an `E'…'` one with its backslash escapes), a "…" identifier, a
/// $tag$ body or a `/* … */` block, which is kept whole (and nests, as the
/// server reads it). Joined onto one line, a trailing comment would swallow
/// everything after it: `:name` in psql then runs `select count(*) -- n from
/// t` as `select count(*)`.
fn without_line_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("--") {
            rest = &rest[rest.find('\n').unwrap_or(rest.len())..];
            continue;
        }
        if rest.starts_with("/*") {
            let len = block_comment_len(rest);
            out.push_str(&rest[..len]);
            rest = &rest[len..];
            continue;
        }
        let mut before = out.chars().rev();
        let (last, second) = (before.next(), before.next());
        let len = match c {
            '\'' => {
                let escapes =
                    matches!(last, Some('E' | 'e')) && !second.is_some_and(identifier_char);
                Some(quoted_len(rest, '\'', escapes))
            }
            '"' => Some(quoted_len(rest, '"', false)),
            // `$` inside a name (`a$b$c`) is part of the name, not a tag.
            '$' if !last.is_some_and(identifier_char) => dollar_tag(rest).map(|tag| {
                rest[tag.len()..]
                    .find(&tag)
                    .map_or(rest.len(), |i| tag.len() * 2 + i)
            }),
            _ => None,
        };
        let len = len.unwrap_or(c.len_utf8());
        out.push_str(&rest[..len]);
        rest = &rest[len..];
    }
    out
}

/// A character that continues a name, `$` included, as the server lexes one.
fn identifier_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

/// How long the run quoted with `q` at the start of `s` is, closing quote
/// included, or all of `s` when it never closes. A doubled quote stays inside,
/// and with `escapes` so does a quote after a backslash.
fn quoted_len(s: &str, q: char, escapes: bool) -> usize {
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if escapes && c == '\\' {
            chars.next();
            continue;
        }
        if c == q {
            if s[i + 1..].starts_with(q) {
                chars.next();
                continue;
            }
            return i + 1;
        }
    }
    s.len()
}

/// How long the `/* … */` at the start of `s` is, nested ones included, or all
/// of `s` when it is never closed.
fn block_comment_len(s: &str) -> usize {
    let mut depth = 0usize;
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with("/*") {
            depth += 1;
            i += 2;
        } else if s[i..].starts_with("*/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    s.len()
}

/// Whether psql takes `name` as a variable of the user's own, so `:name` can
/// expand at its prompt: letters, digits, `_` and any non-ASCII character, and
/// not all upper case, the style psql keeps for the variables it acts on
/// itself (`PROMPT1`, `ECHO`, `HISTFILE`), where a `\set` would change psql.
/// A snippet named otherwise still runs as `esql <conn> :name`, but gets no
/// `\set`.
pub fn psql_name(name: &str) -> bool {
    let takes = name
        .chars()
        .all(|c| c == '_' || c.is_ascii_alphanumeric() || !c.is_ascii());
    let psqls_own = name
        .chars()
        .all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit());
    !name.is_empty() && takes && !psqls_own
}

/// The `$tag$` opening a dollar-quoted string at the start of `s`, if one does;
/// `$1` is a parameter, since a tag cannot start with a digit.
fn dollar_tag(s: &str) -> Option<String> {
    let inner = &s[1..];
    let len = inner.find('$')?;
    let tag = &inner[..len];
    let valid = tag.chars().next().is_none_or(|c| !c.is_ascii_digit())
        && tag.chars().all(|c| c.is_alphanumeric() || c == '_');
    valid.then(|| format!("${tag}$"))
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
            // Listed even when it is not UTF-8, so it is seen and never
            // taken for a free name; `connect` refuses to run it.
            Some(Snippet {
                name: path.file_stem()?.to_string_lossy().into_owned(),
                sql: String::from_utf8_lossy(&fs::read(&path).ok()?).into_owned(),
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

/// Whether `name` can be a snippet's file stem without escaping the directory.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && !name.starts_with('.')
}

pub fn save_in(dir: &std::path::Path, name: &str, sql: &str) -> Result<PathBuf> {
    let name = name.trim();
    if !valid_name(name) {
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

/// Give a saved query another name, its file moved whole, so nothing in it
/// changes. A name another query already has is refused.
pub fn rename(from: &str, to: &str) -> Result<()> {
    let to = to.trim();
    if !valid_name(to) {
        anyhow::bail!("a snippet name cannot be empty, contain a slash, or start with a dot");
    }
    if from == to {
        return Ok(());
    }
    let (old, new) = (
        dir().join(format!("{from}.sql")),
        dir().join(format!("{to}.sql")),
    );
    if new.exists() {
        anyhow::bail!("a query called '{to}' is already saved");
    }
    fs::rename(&old, &new).with_context(|| format!("renaming {}", old.display()))
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
        if one.is_empty() || !psql_name(&s.name) {
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
/// every other line through byte for byte. Returns whether the file changed.
pub fn sync_psqlrc() -> Result<bool> {
    let path = psqlrc_path();
    let (existing, body) = psqlrc_after(&list())?;
    // Nothing to do is the common case, and it has to cost nothing: this runs on
    // the way into every postgres session, so writing unconditionally would
    // rewrite the file, and take a backup when those are on, on every connect.
    if body == existing {
        return Ok(false);
    }
    if path.exists() {
        crate::ini::backup(&path)?;
    }
    crate::ini::write_atomic_bytes(&path, &body)?;
    Ok(true)
}

/// Whether `sync_psqlrc` would change the file with `snips` saved, so a dry
/// run says exactly what the real one does.
pub fn psqlrc_would_change(snips: &[Snippet]) -> bool {
    psqlrc_after(snips).is_ok_and(|(now, after)| now != after)
}

/// `~/.psqlrc` as it is and as it would be with `snips` in its block. A file
/// that cannot be read is an error, never an empty one to write over.
fn psqlrc_after(snips: &[Snippet]) -> Result<(Vec<u8>, Vec<u8>)> {
    let path = psqlrc_path();
    let existing = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let body = splice_bytes(&existing, &psqlrc_block(snips));
    Ok((existing, body))
}

#[cfg(test)]
/// Replace the marked block, or append one if there is not already a block.
pub fn splice(existing: &str, block: &str) -> String {
    String::from_utf8_lossy(&splice_bytes(existing.as_bytes(), block)).into_owned()
}

/// `splice` over the file's bytes, every line outside the block copied as it
/// was. A start marker with no end marker below it is a block of that one
/// line, so the lines after it are never taken. A block with no `\set` in it
/// removes the old one and adds none, so a machine without snippets never
/// gains a `~/.psqlrc`.
fn splice_bytes(existing: &[u8], block: &str) -> Vec<u8> {
    let lines: Vec<&[u8]> = existing.split_inclusive(|b| *b == b'\n').collect();
    let text = |l: &[u8]| String::from_utf8_lossy(l.trim_ascii_end()).into_owned();
    let start = lines.iter().position(|l| text(l).starts_with(START));
    let end = start.map(|s| {
        (s + 1..lines.len())
            .find(|&i| text(lines[i]).trim() == END)
            .map_or(s + 1, |e| e + 1)
    });
    let empty = !block.lines().any(|l| l.starts_with("\\set "));
    let mut out: Vec<u8> = Vec::with_capacity(existing.len() + block.len() + 2);
    match (start, end) {
        (Some(s), Some(e)) => {
            lines[..s].iter().for_each(|l| out.extend_from_slice(l));
            if !empty {
                out.extend_from_slice(block.as_bytes());
            }
            lines[e..].iter().for_each(|l| out.extend_from_slice(l));
        }
        _ => {
            out.extend_from_slice(existing);
            if !empty {
                if !out.is_empty() && !out.ends_with(b"\n") {
                    out.push(b'\n');
                }
                if !out.is_empty() && !out.ends_with(b"\n\n") {
                    out.push(b'\n');
                }
                out.extend_from_slice(block.as_bytes());
            }
        }
    }
    out
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

    #[test]
    fn flattening_never_cuts_inside_a_block_comment() {
        // A `--` inside `/* */` is not a comment to the server, so cutting
        // there would drop the rest of the query from the `\set` line.
        let block = psqlrc_block(&[Snippet {
            name: "cnt".into(),
            sql: "select count(*) -- n\nfrom t /* a -- b */ where x = 1\n".into(),
            path: PathBuf::new(),
        }]);
        assert!(
            block.contains("\\set cnt 'select count(*) from t /* a -- b */ where x = 1'"),
            "{block}"
        );
    }

    #[test]
    fn a_start_marker_with_no_end_takes_only_its_own_line() {
        let existing = format!("\\set mine 1\n{START}\n\\timing on\n");
        let out = splice(&existing, &psqlrc_block(&[]));
        assert_eq!(
            out, "\\set mine 1\n\\timing on\n",
            "the user's lines after an unclosed marker must survive"
        );
    }
}
