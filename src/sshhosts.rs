//! The aliases in `~/.ssh/config`, read and never written.
//!
//! A tunnel needs a machine to hop through, and that machine is already named
//! in your ssh config. Asking you to retype `bastion` when the app can offer you
//! the list is the thing easysql exists to stop, so the tunnel wizard picks from
//! here instead of giving you a text field.
//!
//! Deliberately a reader only: `~/.ssh/config` belongs to ssh, and a
//! tool that writes another tool's config behind its back is how two
//! programs end up fighting over one file.

use std::fs;
use std::path::{Path, PathBuf};

fn ssh_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".ssh")
}

/// The lines of the config that matter here, from the file and every file it
/// `Include`s, in the order ssh reads them.
enum Line {
    /// A `Host` line's patterns: what follows applies to a name they match.
    Host(Vec<String>),
    /// A `Match` line, whose conditions are not ours to evaluate, so what
    /// follows applies to no name here.
    Match,
    /// Back in the block an `Include` was inside, once the included file ends.
    Resume(Box<Line>),
    HostName(String),
}

fn lines() -> Vec<Line> {
    let mut out = Vec::new();
    read_into(&ssh_dir().join("config"), &mut out, 0);
    out
}

/// ssh_config(5): keyword and value split on a space, a tab or `=`, and the
/// keyword is case-insensitive. Any other blank, a no-break space pasted from
/// a web page say, splits nothing for ssh, so it splits nothing here.
fn keyword(raw: &str) -> Option<(String, &str)> {
    let line = raw.trim_matches([' ', '\t', '\r']);
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let i = line.find([' ', '\t', '='])?;
    let key = line[..i].trim_end_matches('=').to_ascii_lowercase();
    let value = line[i + 1..].trim_start_matches(['=', ' ', '\t']).trim();
    Some((key, value))
}

/// ssh stops following `Include` this deep, and so do we.
const INCLUDE_DEPTH: usize = 16;

fn read_into(path: &Path, out: &mut Vec<Line>, depth: usize) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    // The block a line is in, so an `Include` inside it can be resumed after.
    let mut block: Option<Line> = None;
    for raw in text.lines() {
        let Some((key, value)) = keyword(raw) else {
            continue;
        };
        match key.as_str() {
            "host" => {
                let patterns: Vec<String> = value.split_whitespace().map(str::to_string).collect();
                out.push(Line::Host(patterns.clone()));
                block = Some(Line::Host(patterns));
            }
            "match" => {
                out.push(Line::Match);
                block = Some(Line::Match);
            }
            "hostname" => out.push(Line::HostName(value.to_string())),
            "include" if depth < INCLUDE_DEPTH => {
                for pattern in value.split_whitespace() {
                    for file in expand(pattern) {
                        read_into(&file, out, depth + 1);
                    }
                }
                let resumed = match &block {
                    Some(Line::Host(p)) => Line::Host(p.clone()),
                    Some(_) => Line::Match,
                    None => Line::Host(vec!["*".to_string()]),
                };
                out.push(Line::Resume(Box::new(resumed)));
            }
            _ => {}
        }
    }
}

/// The files an `Include` names: `~/` from home, a relative path from
/// `~/.ssh`, and `*` or `?` in the file name matching the way the shell does,
/// in name order.
fn expand(pattern: &str) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None if Path::new(pattern).is_absolute() => PathBuf::from(pattern),
        None => ssh_dir().join(pattern),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut found: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.starts_with('.') && wildmatch(&name, &n)
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// `*` and `?` matching, as ssh matches a host pattern and the shell a file.
fn wildmatch(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(&c) if c == '?' || c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Whether a `Host` line's patterns take `alias`: one of them matches it and
/// none of the `!` ones does, compared without case as ssh compares them.
fn host_matches(patterns: &[String], alias: &str) -> bool {
    let alias = alias.to_ascii_lowercase();
    let mut hit = false;
    for p in patterns {
        let p = p.to_ascii_lowercase();
        match p.strip_prefix('!') {
            Some(negated) if wildmatch(negated, &alias) => return false,
            Some(_) => {}
            None => hit |= wildmatch(&p, &alias),
        }
    }
    hit
}

/// The `HostName` an alias resolves to, or the alias itself when no block for
/// it sets one (which is ssh's own rule: the first value found wins, and `%h`
/// is the name typed). Used to tell whether the machine you are tunnelling
/// *through* is the same machine the database is *on*, because that changes
/// what the far end of the forward has to say.
pub fn hostname_of(alias: &str) -> String {
    let mut active = true;
    for line in lines() {
        let line = match line {
            Line::Resume(inner) => *inner,
            line => line,
        };
        match line {
            Line::Host(patterns) => active = host_matches(&patterns, alias),
            Line::Match => active = false,
            Line::HostName(value) if active => {
                return value.replace("%h", alias).replace("%%", "%");
            }
            Line::HostName(_) | Line::Resume(_) => {}
        }
    }
    alias.to_string()
}

/// Is `host` the very machine `alias` connects to? Answered by name, without
/// resolving anything: `raspi`, the `HostName` its block gives, and the
/// literal IP written there all count as one.
pub fn is_same_machine(alias: &str, host: &str) -> bool {
    if alias.is_empty() || host.is_empty() {
        return false;
    }
    let target = hostname_of(alias);
    alias.eq_ignore_ascii_case(host) || target.eq_ignore_ascii_case(host)
}

/// Every concrete alias in the config and the files it includes, in the order
/// ssh reads them, without the pattern blocks (`Host *`) that are settings
/// rather than destinations.
pub fn aliases() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines() {
        let Line::Host(patterns) = line else {
            continue;
        };
        for alias in patterns {
            if alias.contains(['*', '?', '!']) || out.contains(&alias) {
                continue;
            }
            out.push(alias);
        }
    }
    out
}
