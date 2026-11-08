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
    // `_secret` holds a usql passfile until the probe has exited.
    let own = probe_settings(c.engine, s);
    let (argv, env, _secret) = match c.engine {
        Engine::Pg | Engine::MySql if !engines::client_installed(c.engine, &own) => return None,
        Engine::Pg => {
            let (argv, env) = engines::pg::probe_argv(c, &own);
            (argv, env, None)
        }
        Engine::MySql => {
            let (argv, env) = engines::mysql::probe_argv(c, &own);
            (argv, env, None)
        }
        Engine::MsSql => {
            let (argv, secret) = engines::mssql::probe_argv(c, s);
            (argv, secret.env.clone(), Some(secret))
        }
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
    let (fix, line) = classify(&text)?;
    Some(Failure {
        fix,
        said: shown(line),
    })
}

/// The settings a Postgres or MySQL probe runs under: that engine's own
/// client, whose words `classify` reads, as configured when the command takes
/// its flags (a full path, a wrapper, `mariadb`) and as shipped otherwise,
/// never pgcli or usql, which answer in flags and words of their own. SQL
/// Server keeps the settings as they are, since its probe speaks usql too.
fn probe_settings(engine: Engine, s: &Settings) -> Settings {
    let base = || Settings {
        probe_timeout: s.probe_timeout,
        in_tui: s.in_tui,
        mssql_passwords: s.mssql_passwords,
        ..Settings::default()
    };
    let mut own = base();
    match engine {
        Engine::Pg => own.psql_command = s.psql_command.clone(),
        Engine::MySql => own.mysql_command = s.mysql_command.clone(),
        Engine::MsSql => return s.clone(),
        Engine::Sqlite => {}
    }
    match engines::speaks_client_flags(engine, &own) {
        true => own,
        false => base(),
    }
}

/// A matched line as the offer shows it: `psql: error: connection to server
/// ... failed: FATAL: ...` reads better from the part the server actually said.
fn shown(line: &str) -> String {
    let line = line.trim();
    line.rsplit_once("FATAL:")
        .map(|(_, rest)| rest.trim())
        .unwrap_or(line)
        .to_string()
}

/// Match the client's own phrasing, and hand back the line that matched, so
/// the offer quotes the words it acted on. These are the exact strings libpq
/// and the MySQL client emit; keeping them literal is what makes the
/// classification honest rather than a guess about what probably went wrong.
fn classify(text: &str) -> Option<(Fix, &str)> {
    const PASSWORD: [&str; 5] = [
        "no password supplied",
        "password authentication failed",
        "Access denied for user",
        "authentication method 10 not supported",
        // SQL Server's own wording, through sqlcmd and usql alike.
        "Login failed for user",
    ];
    const TUNNEL: [&str; 14] = [
        "Connection refused",
        "Connection timed out",
        // libpq's own, when `PGCONNECT_TIMEOUT` runs out.
        "timeout expired",
        "No route to host",
        "could not translate host name",
        "Name or service not known",
        "no pg_hba.conf entry",
        "Can't connect to MySQL server",
        "Can't connect to server",
        // A name that will not resolve is the same problem as a host that will
        // not answer: you are not on a network that can see it.
        "Unknown MySQL server host",
        // MariaDB's spelling of the same.
        "Unknown server host",
        // sqlcmd's wording when nothing is listening or the name is wrong.
        "TCP Provider",
        "Login timeout expired",
        // usql's, for a port nothing answers on.
        "unable to open tcp connection with host",
    ];
    const NAME: [&str; 4] = [
        "does not exist",
        "Unknown database",
        // SQL Server's wording for a database that is not there.
        "Cannot open database",
        // MySQL's 1044: the login worked and that database is not this user's.
        "' to database '",
    ];

    // A name first: SQL Server follows "Cannot open database" with "Login
    // failed for user", so a missing database would otherwise read as a bad
    // password; and a password before a tunnel, since libpq that tried two
    // addresses says "Connection refused" for one before the password failure
    // of the other.
    [
        (Fix::Name, &NAME[..]),
        (Fix::Password, &PASSWORD[..]),
        (Fix::Tunnel, &TUNNEL[..]),
    ]
    .into_iter()
    .find_map(|(fix, phrases)| {
        let line = text
            .lines()
            .find(|l| phrases.iter().any(|p| l.contains(p)))?;
        Some((fix, line))
    })
}
