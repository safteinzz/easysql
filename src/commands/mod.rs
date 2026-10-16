//! The CLI: connect and list, plus one noun per TUI tab (`conn`, `pw`,
//! `tunnel`, `snippet`, `set`), so a script or an agent can do what the toolbox
//! does without driving a terminal or editing a client's file by hand.

pub mod completions;
pub mod conn;
pub mod connect;
pub mod ls;
pub mod pw;
pub mod selfcmd;
pub mod set;
pub mod snippet;
pub mod tunnel;

use colored::Colorize;
use std::io::{IsTerminal, Write};

/// The words that are commands, so a connection given one of them as its name
/// is opened as `<engine>:<name>` instead.
pub const COMMANDS: [&str; 9] = [
    "ls",
    "conn",
    "pw",
    "tunnel",
    "snippet",
    "set",
    "completions",
    "self",
    "help",
];

/// `-n`, shared by every command that changes something.
#[derive(clap::Args)]
pub struct DryRun {
    /// Dry run: print the steps it would take and change nothing
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,
}

/// `-y`, shared by every command that deletes something.
#[derive(clap::Args)]
pub struct Yes {
    /// Delete without asking, which is required when there is no terminal to ask on
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

/// `--json`, shared by every listing.
#[derive(clap::Args)]
pub struct Json {
    /// Print it for a script: a JSON array whose field names are fixed
    #[arg(long)]
    pub json: bool,
}

/// One step of a change, on stdout. A dry run prints the same lines and stops
/// there, so its output is the real run's line for line.
pub fn step(line: &str) {
    println!("{line}");
}

/// Our own words for a failure, on stderr, then exit 1.
pub fn fail(msg: &str) -> ! {
    eprintln!("{}", format!("esql: {msg}").red());
    std::process::exit(1);
}

/// An error chain in our words: the contexts as written, then what went wrong
/// underneath said plainly, never an OS error number, and with what to do next
/// when the cause is one a person can fix.
pub fn said(e: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in e.chain() {
        let text = match cause.downcast_ref::<std::io::Error>() {
            Some(io) => io_said(io),
            None => cause.to_string(),
        };
        parts.push(text);
    }
    parts.join(": ")
}

fn io_said(e: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::PermissionDenied => {
            "permission denied, so check who owns it and its mode".into()
        }
        ErrorKind::NotFound => "it does not exist".into(),
        ErrorKind::AlreadyExists => "it already exists".into(),
        ErrorKind::ReadOnlyFilesystem => "the filesystem is read-only".into(),
        ErrorKind::StorageFull => "the disk is full, so free some space and try again".into(),
        _ => {
            let text = e.to_string();
            let text = match text.find(" (os error") {
                Some(i) => text[..i].to_string(),
                None => text,
            };
            let mut chars = text.chars();
            match chars.next() {
                Some(c) => c.to_lowercase().chain(chars).collect(),
                None => text,
            }
        }
    }
}

/// A warning that does not stop the command.
pub fn warn(msg: &str) {
    eprintln!("{}", format!("esql: {msg}").yellow());
}

/// The connection a management command names, or exit 1 saying why not.
pub fn find_conn(needle: &str) -> crate::engines::Conn {
    crate::engines::find(needle).unwrap_or_else(|e| fail(&said(&e)))
}

/// A path for a step line, with the home folder shown as `~`.
pub fn shown(path: &std::path::Path) -> String {
    crate::ini::collapse_tilde(&path.to_string_lossy())
}

/// Refuse a delete that could not be asked about, before any step is printed:
/// no `-y`, no dry run and no terminal to ask on.
pub fn can_ask(what: &str, yes: bool, dry: bool) {
    if !yes && !dry && !std::io::stdin().is_terminal() {
        fail(&format!(
            "deleting {what} needs a yes, and there is no terminal to ask on: pass `-y`"
        ));
    }
}

/// Ask before a delete, on the terminal only. `typed` makes the answer the
/// thing's exact name, for what exists nowhere else; otherwise `y` will do.
/// With `-y` nothing is asked; call `can_ask` first, before the steps.
pub fn confirm_delete(what: &str, typed: Option<&str>, yes: bool) {
    if yes {
        return;
    }
    can_ask(what, yes, false);
    match typed {
        Some(name) => eprint!("delete {what}? type `{name}` to confirm: "),
        None => eprint!("delete {what}? [y/N] "),
    }
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    let answer = answer.trim();
    let agreed = match typed {
        Some(name) => answer == name,
        None => matches!(answer, "y" | "Y" | "yes"),
    };
    if !agreed {
        eprintln!("{}", "esql: cancelled, nothing was deleted".dimmed());
        std::process::exit(1);
    }
}

/// Read one line on the terminal without echoing it. `None` when it was
/// cancelled with Esc or Ctrl-C, or the terminal could not be put in raw mode.
pub fn read_secret(prompt: &str) -> Option<String> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    crossterm::terminal::enable_raw_mode().ok()?;
    let mut typed = String::new();
    let answer = loop {
        let Ok(event) = read() else {
            break None;
        };
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => break Some(typed),
            KeyCode::Esc => break None,
            KeyCode::Char('c') if ctrl => break None,
            KeyCode::Char('u') if ctrl => typed.clear(),
            KeyCode::Backspace => {
                typed.pop();
            }
            KeyCode::Char(c) if !ctrl => typed.push(c),
            _ => {}
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    answer
}

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON string, or `null` for a value that is not set.
pub fn json_opt(s: Option<&str>) -> String {
    s.map_or_else(|| "null".into(), json_str)
}

/// One object per row, one row per line, so a diff of two runs reads.
pub fn json_array(rows: &[String]) -> String {
    if rows.is_empty() {
        return "[]".into();
    }
    format!("[\n  {}\n]", rows.join(",\n  "))
}
