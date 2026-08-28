//! The aliases in `~/.ssh/config`, read and never written.
//!
//! A tunnel needs a machine to hop through, and that machine is already named
//! in your ssh config. Asking you to retype `bastion` when the app can offer you
//! the list is the thing easysql exists to stop, so the tunnel wizard picks from
//! here instead of giving you a text field.
//!
//! Deliberately a reader only: `~/.ssh/config` belongs to ssh and to easyssh,
//! and a tool that writes another tool's config behind its back is how two
//! programs end up fighting over one file.

use std::fs;
use std::path::PathBuf;

fn config_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".ssh/config")
}

/// The `HostName` an alias resolves to, or the alias itself when the block does
/// not set one (which is ssh's own rule). Used to tell whether the machine you
/// are tunnelling *through* is the same machine the database is *on*, because
/// that changes what the far end of the forward has to say.
pub fn hostname_of(alias: &str) -> String {
    let Ok(text) = fs::read_to_string(config_path()) else {
        return alias.to_string();
    };
    let mut here = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(i) = line.find(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        let key = line[..i].trim_end_matches('=');
        let value = line[i + 1..].trim_start_matches(['=', ' ', '\t']).trim();
        if key.eq_ignore_ascii_case("host") {
            here = value.split_whitespace().any(|a| a == alias);
        } else if here && key.eq_ignore_ascii_case("hostname") {
            return value.to_string();
        }
    }
    alias.to_string()
}

/// Is `host` the very machine `alias` connects to? Answered by name and by
/// address, so `raspi`, its `HostName` and the literal IP all count as one.
pub fn is_same_machine(alias: &str, host: &str) -> bool {
    if alias.is_empty() || host.is_empty() {
        return false;
    }
    let target = hostname_of(alias);
    alias.eq_ignore_ascii_case(host) || target.eq_ignore_ascii_case(host)
}

/// Every concrete alias in the config, in file order, without the pattern
/// blocks (`Host *`) that are settings rather than destinations.
pub fn aliases() -> Vec<String> {
    let Ok(text) = fs::read_to_string(config_path()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // ssh_config(5): keyword and value split on whitespace or `=`, and the
        // keyword is case-insensitive.
        let Some(i) = line.find(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        let key = line[..i].trim_end_matches('=');
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        let value = line[i + 1..].trim_start_matches(['=', ' ', '\t']);
        for alias in value.split_whitespace() {
            if alias.contains(['*', '?', '!']) || out.iter().any(|a| a == alias) {
                continue;
            }
            out.push(alias.to_string());
        }
    }
    out
}
