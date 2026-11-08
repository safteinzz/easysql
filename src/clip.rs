//! Putting text on the system clipboard, so the things you would otherwise
//! leave the toolbox to copy - the connect command, the connection URL your
//! app config wants - are one keypress away.
//!
//! There is no portable clipboard on Linux, so we shell out to whichever helper
//! is installed. Nothing is bundled and nothing is installed for you: if none of
//! them is present we say which ones would work.

use std::io::Write;
use std::process::{Command, Stdio};

/// The helpers we know how to drive, best first. Wayland before X11 because a
/// Wayland session usually has `xclip` available through Xwayland but wants
/// `wl-copy`; `pbcopy` is macOS, where it is always the right answer.
const TOOLS: [(&str, &[&str]); 4] = [
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("pbcopy", &[]),
];

/// Copy `text`, returning the tool that took it so the status line can name the
/// real command (`copied to clipboard (wl-copy)`) rather than claiming magic.
/// Every installed helper is tried in turn, since one being on PATH is not one
/// that works here (`wl-copy` outside a Wayland session), and when none takes
/// it the first one's own words say why.
pub fn copy(text: &str) -> Result<&'static str, String> {
    let mut first_failure: Option<String> = None;
    for (cmd, args) in TOOLS.iter().filter(|(cmd, _)| on_path(cmd)) {
        match run(cmd, args, text) {
            Ok(()) => return Ok(cmd),
            Err(e) => {
                first_failure.get_or_insert(e);
            }
        }
    }
    Err(first_failure.unwrap_or_else(|| {
        "no clipboard tool found: install wl-clipboard, xclip or xsel".to_string()
    }))
}

/// One helper's try: the text on its stdin, and on a failure its own words, at
/// most five lines. Its stderr goes to a private file rather than a pipe,
/// because wl-copy and xclip leave a process behind to serve the selection,
/// which would hold a pipe open and never let a read of it end.
fn run(cmd: &str, args: &[&str], text: &str) -> Result<(), String> {
    let said = |e: &std::io::Error| crate::commands::io_said(e);
    let log = crate::engines::RunFile::write("").ok();
    let stderr = log
        .as_ref()
        .and_then(|f| std::fs::OpenOptions::new().append(true).open(f.path()).ok())
        .map_or(Stdio::null(), Stdio::from);
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("could not start {cmd}: {}", said(&e)))?;
    // Dropping stdin closes the pipe, which is what tells the helper the
    // selection is complete; without it wl-copy would sit there waiting.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("{cmd}: {}", said(&e)))?;
    }
    let status = child.wait().map_err(|e| format!("{cmd}: {}", said(&e)))?;
    if status.success() {
        return Ok(());
    }
    let words = log
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f.path()).ok())
        .unwrap_or_default();
    let lines: Vec<&str> = words
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match lines.is_empty() {
        true => Err(format!("{cmd} exited {}", status.code().unwrap_or(-1))),
        false => Err(format!(
            "{cmd}: {}",
            lines[lines.len().saturating_sub(5)..].join("\n")
        )),
    }
}

/// Whether any of the helpers is installed.
pub fn any_installed() -> bool {
    TOOLS.iter().any(|(cmd, _)| on_path(cmd))
}

/// The helper this session would use, to offer when none is installed:
/// `wl-copy` under Wayland, `xclip` under X11, and none without a display,
/// where there is no clipboard to put anything on.
pub fn session_tool() -> Option<crate::engines::Tool> {
    let set = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
    match (set("WAYLAND_DISPLAY"), set("DISPLAY")) {
        (true, _) => Some(crate::engines::Tool::WlClipboard),
        (false, true) => Some(crate::engines::Tool::Xclip),
        (false, false) => None,
    }
}

/// Is this program on PATH? Same check `engines` uses for the clients: no shell, no
/// `which`, just the search path the OS would use.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}
