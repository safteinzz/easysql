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
pub fn copy(text: &str) -> Result<&'static str, String> {
    let (cmd, args) = TOOLS.iter().find(|(cmd, _)| on_path(cmd)).ok_or_else(|| {
        "no clipboard tool found (install wl-clipboard, xclip or xsel)".to_string()
    })?;

    let mut child = Command::new(cmd)
        .args(*args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{cmd}: {e}"))?;
    // Dropping stdin closes the pipe, which is what tells the helper the
    // selection is complete; without it wl-copy would sit there waiting.
    {
        let mut stdin = child.stdin.take().ok_or("clipboard: no stdin")?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("{cmd}: {e}"))?;
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(cmd),
        Ok(s) => Err(format!("{cmd} exited {}", s.code().unwrap_or(-1))),
        Err(e) => Err(format!("{cmd}: {e}")),
    }
}

/// Is this program on PATH? Same check `engines` uses for the clients: no shell, no
/// `which`, just the search path the OS would use.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}
