//! The real binary, run in a HOME of its own under the temp dir with only
//! stand-in clients on its PATH: what reaches the client, and what a run that
//! is refused before the client starts leaves behind. Nothing real is read or
//! written.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Stage(PathBuf);

impl Stage {
    fn new() -> Stage {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("easysql-stage-{}-{stamp}", std::process::id()));
        fs::create_dir_all(root.join(".config/easysql")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        Stage(root)
    }

    /// Write a file under the stage's HOME, with this mode.
    fn file(&self, rel: &str, body: &str, mode: u32) {
        let path = self.0.join(rel);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// A client on the stage's PATH that appends what it was handed, and the
    /// lines `report` prints from its own environment, to `<name>.log`, then
    /// prints `answer`.
    fn client(&self, name: &str, report: &str, answer: &str) {
        let log = self.0.join(format!("{name}.log"));
        self.file(
            &format!("bin/{name}"),
            &format!(
                "#!/bin/sh\n{{ echo \"argv: $*\"; {report} }} >> '{}'\necho {answer}\n",
                log.display()
            ),
            0o755,
        );
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(format!("{name}.log"))).unwrap_or_default()
    }

    fn esql(&self, args: &[&str]) -> Output {
        let home = &self.0;
        Command::new(env!("CARGO_BIN_EXE_esql"))
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("PATH", home.join("bin"))
            .output()
            .unwrap()
    }

    fn history(&self) -> Option<String> {
        fs::read_to_string(self.0.join(".local/state/easysql/history")).ok()
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SERVICES: &str = "[prod]\nhost=127.0.0.1\nport=1\n\n\
                        [ro]\nhost=127.0.0.1\nport=1\noptions=-c default_transaction_read_only=on\n";

fn pg_stage(settings: &str) -> Stage {
    let s = Stage::new();
    s.file(".pg_service.conf", SERVICES, 0o644);
    s.file(".config/easysql/settings", settings, 0o644);
    s
}

#[test]
fn a_run_refused_before_the_client_starts_leaves_no_history() {
    // The configured client is not installed at all.
    let missing = pg_stage("psql_command=nosuchclient\n");
    let out = missing.esql(&["prod"]);
    assert_eq!(
        out.status.code(),
        Some(127),
        "no client is exit 127: {out:?}"
    );
    assert_eq!(
        missing.history(),
        None,
        "a session that never started is not recent"
    );

    // A query needs psql's -c, pgcli cannot take it, and there is no psql to
    // step around it with.
    let foreign = pg_stage("psql_command=pgcli\n");
    foreign.client("pgcli", "", "");
    let out = foreign.esql(&["prod", "select 1"]);
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    assert!(
        foreign.log("pgcli").is_empty(),
        "pgcli must not be handed a -c"
    );
    assert_eq!(foreign.history(), None);

    // The connection is marked read-only but the server says it could write,
    // which is what a pooler that drops `options` looks like.
    let writable = pg_stage("");
    writable.client("psql", "", "off");
    let out = writable.esql(&["ro", "truncate orders"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let ran: Vec<String> = writable.log("psql").lines().map(str::to_string).collect();
    assert_eq!(
        ran.len(),
        1,
        "psql is asked the question and never handed the query: {ran:?}"
    );
    assert!(ran[0].contains("default_transaction_read_only"), "{ran:?}");
    assert_eq!(writable.history(), None);
}

const SECRET: &str = "s3cret-pass";

fn ms_stage(settings: &str, pass_mode: u32) -> Stage {
    let s = Stage::new();
    s.file(
        ".config/easysql/mssql.conf",
        "[reporting]\nhost=db.example.com\ndatabase=app\nuser=sa\n",
        0o644,
    );
    s.file(
        ".esqlpass",
        &format!("[reporting]\npassword={SECRET}\n"),
        pass_mode,
    );
    s.file(".config/easysql/settings", settings, 0o644);
    s.client("sqlcmd", r#"echo "env: ${SQLCMDPASSWORD:-unset}";"#, "");
    s
}

#[test]
fn a_kept_sqlcmd_password_reaches_sqlcmd_in_its_environment_and_never_its_argv() {
    let allowed = ms_stage("mssql_passwords=on\n", 0o600);
    allowed.esql(&["reporting", "select 1"]);
    let log = allowed.log("sqlcmd");
    assert!(
        log.contains(&format!("env: {SECRET}")),
        "with the setting on, sqlcmd gets the password and so never prompts: {log}"
    );
    let argv = log
        .lines()
        .find(|l| l.starts_with("argv:"))
        .unwrap_or_default();
    assert!(
        !argv.contains(SECRET) && !argv.contains("-P"),
        "an argv is readable by every user through ps: {argv}"
    );

    let off = ms_stage("", 0o600);
    off.esql(&["reporting", "select 1"]);
    assert!(
        off.log("sqlcmd").contains("env: unset"),
        "with the setting off a kept password is not used: {}",
        off.log("sqlcmd")
    );
}

#[test]
fn a_password_file_others_can_read_is_not_used() {
    let loose = ms_stage("mssql_passwords=on\n", 0o644);
    loose.esql(&["reporting", "select 1"]);
    assert!(
        loose.log("sqlcmd").contains("env: unset"),
        "a group- or world-readable password file is refused, as libpq refuses .pgpass: {}",
        loose.log("sqlcmd")
    );
}
