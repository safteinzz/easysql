//! Reading what the client said when a connection failed, so the app can offer
//! the fix instead of leaving you with a screen it just redrew over.
//!
//! The client wrote the real reason to stderr and then we restored the TUI on
//! top of it. So after a failed open we ask the server the same question once
//! more, non-interactively (`select 1`, no prompt, short deadline), and match
//! its own words. That probe runs only after a failure, so the success path
//! costs nothing.

use super::*;

/// What would actually get you in. Each one is a different modal offering a
/// different next step, which is the whole reason to classify at all.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Fix {
    /// No password on file, or the wrong one.
    Password,
    /// Nothing answered, or the server will not take a connection from this
    /// address: both are solved by coming in through a bastion.
    Tunnel,
    /// We got in; the database or the role named does not exist.
    Name,
}

pub(super) struct Failure {
    /// The server's own first line, shown verbatim: it is more precise than
    /// anything we would write in its place.
    pub(super) said: String,
    pub(super) fix: Fix,
}

/// Ask once, non-interactively, and classify the answer. `None` when the client
/// could not be run at all, or when it succeeded this time (a password typed at
/// the client's own prompt, or a server that came back).
pub(super) fn why_failed(c: &Conn, s: &Settings) -> Option<Failure> {
    // A SQLite file cannot fail for any of these reasons; sqlite3 either opened
    // the file or the path is wrong, which it already said plainly.
    if !c.engine.networked() {
        return None;
    }
    let (argv, env) = match c.engine {
        Engine::Pg => engines::pg::probe_argv(c, s),
        Engine::MySql => engines::mysql::probe_argv(c, s),
        Engine::MsSql => engines::mssql::probe_argv(c, s),
        Engine::Sqlite => return None,
    };
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().ok()?;
    if out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stderr);
    let said = first_line(&text)?;
    Some(Failure {
        fix: classify(&text)?,
        said,
    })
}

/// The first line worth showing: clients prefix their real message with the
/// program name and sometimes a blank line.
fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| {
        !l.is_empty() && !l.starts_with("Try '") && !l.ends_with("--help' for more information.")
    })?;
    // `psql: error: connection to server ... failed: FATAL: ...` reads better
    // from the part the server actually said.
    let trimmed = line
        .rsplit_once("FATAL:")
        .map(|(_, rest)| rest.trim())
        .unwrap_or(line);
    Some(trimmed.to_string())
}

/// Match the client's own phrasing. These are the exact strings libpq and the
/// MySQL client emit; keeping them literal is what makes the classification
/// honest rather than a guess about what probably went wrong.
fn classify(text: &str) -> Option<Fix> {
    const PASSWORD: [&str; 5] = [
        "no password supplied",
        "password authentication failed",
        "Access denied for user",
        "authentication method 10 not supported",
        // sqlcmd's wording, via the ODBC driver.
        "Login failed for user",
    ];
    const TUNNEL: [&str; 11] = [
        "Connection refused",
        "Connection timed out",
        "No route to host",
        "could not translate host name",
        "Name or service not known",
        "no pg_hba.conf entry",
        "Can't connect to MySQL server",
        "Can't connect to server",
        // A name that will not resolve is the same problem as a host that will
        // not answer: you are not on a network that can see it.
        "Unknown MySQL server host",
        // sqlcmd's wording when nothing is listening or the name is wrong.
        "TCP Provider",
        "Login timeout expired",
    ];
    const NAME: [&str; 3] = [
        "does not exist",
        "Unknown database",
        // sqlcmd's wording for a database that is not there.
        "Cannot open database",
    ];

    if PASSWORD.iter().any(|p| text.contains(p)) {
        return Some(Fix::Password);
    }
    if TUNNEL.iter().any(|p| text.contains(p)) {
        return Some(Fix::Tunnel);
    }
    if NAME.iter().any(|p| text.contains(p)) {
        return Some(Fix::Name);
    }
    None
}
