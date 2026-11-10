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

    fn esql_fed(&self, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let home = &self.0;
        let mut child = Command::new(env!("CARGO_BIN_EXE_esql"))
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("PATH", home.join("bin"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// Every file under the stage's HOME with what it holds, for comparing
    /// before and after.
    fn files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push((path.clone(), fs::read(&path).unwrap_or_default()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &mut out);
        out.sort();
        out
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

#[test]
fn a_dry_run_changes_no_file() {
    let s = Stage::new();
    for setup in [
        &[
            "conn",
            "add",
            "pg",
            "app",
            "--host",
            "db.example.com",
            "--user",
            "alice",
        ][..],
        &["snippet", "add", "q", "select 1"],
    ] {
        let out = s.esql(setup);
        assert!(out.status.success(), "setup {setup:?}: {out:?}");
    }
    let before = s.files();

    for (args, stdin) in [
        (
            &[
                "conn",
                "add",
                "pg",
                "other",
                "--host",
                "db.example.com",
                "--dry-run",
            ][..],
            "",
        ),
        (
            &[
                "conn",
                "edit",
                "app",
                "--port",
                "6543",
                "--rename",
                "app2",
                "--dry-run",
            ],
            "",
        ),
        (&["conn", "rm", "app", "-y", "--dry-run"], ""),
        (&["pw", "set", "app", "--stdin", "--dry-run"], "s3cret\n"),
        (&["snippet", "add", "q2", "select 2", "--dry-run"], ""),
        (&["snippet", "rm", "q", "-y", "--dry-run"], ""),
        (&["set", "backups", "3", "--dry-run"], ""),
        (&["set", "hints", "--default", "--dry-run"], ""),
    ] {
        let out = s.esql_fed(args, stdin);
        assert!(out.status.success(), "{args:?} should succeed: {out:?}");
        assert!(
            !out.stdout.is_empty(),
            "{args:?} should print the steps it would take"
        );
        assert!(
            s.files() == before,
            "{args:?} with --dry-run changed a file"
        );
    }
}

/// The tool a Postgres `--restore` handed `file` to, from the stand-ins' logs.
fn restored_by(s: &Stage, file: &std::path::Path) -> &'static str {
    let file = file.display().to_string();
    let psql = s.log("psql");
    let pg_restore = s.log("pg_restore");
    match (psql.contains(&file), pg_restore.contains(&file)) {
        (true, false) => "psql",
        (false, true) => "pg_restore",
        _ => panic!(
            "expected exactly one tool to get {file}\npsql: {psql}\npg_restore: {pg_restore}"
        ),
    }
}

#[test]
fn a_postgres_restore_sends_archives_to_pg_restore_and_sql_to_psql() {
    let s = pg_stage("");
    s.client("psql", "", "");
    s.client("pg_restore", "", "");

    let mut tar = vec![0u8; 512];
    tar[257..262].copy_from_slice(b"ustar");
    let dir = s.0.join("app.dir");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("toc.dat"), b"PGDMP").unwrap();
    for (name, body, tool) in [
        ("app.dump", b"PGDMP\x01\x0e\x00".to_vec(), "pg_restore"),
        ("app.tar", tar, "pg_restore"),
        ("app.sql", b"create table t (i int);\n".to_vec(), "psql"),
    ] {
        let path = s.0.join(name);
        fs::write(&path, body).unwrap();
        let out = s.esql(&["--restore", "prod", path.to_str().unwrap()]);
        assert!(out.status.success(), "{name}: {out:?}");
        assert_eq!(
            restored_by(&s, &path),
            tool,
            "{name} went to the wrong tool"
        );
    }
    let out = s.esql(&["--restore", "prod", dir.to_str().unwrap()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        restored_by(&s, &dir),
        "pg_restore",
        "a `pg_dump -Fd` directory is an archive"
    );
}

#[test]
fn a_postgres_restore_from_a_pipe_never_reads_it_first() {
    use std::os::unix::fs::OpenOptionsExt;
    let s = pg_stage("");
    s.client("psql", "", "");
    s.client("pg_restore", "", "");
    let fifo = s.0.join("app.pipe");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success(), "mkfifo failed");

    // An archive's mark waits in the pipe, so a peek would send it to pg_restore.
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            use std::io::Write;
            if let Ok(mut w) = fs::OpenOptions::new().write(true).open(&fifo) {
                let _ = w.write_all(b"PGDMP");
            }
        })
    };
    let out = s.esql(&["--restore", "prod", fifo.to_str().unwrap()]);
    // Lets the writer's open return whether or not esql opened the pipe.
    let _unblock = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000) // O_NONBLOCK on Linux
        .open(&fifo)
        .unwrap();
    writer.join().unwrap();

    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        restored_by(&s, &fifo),
        "psql",
        "bytes read from a pipe never reach the tool, so a pipe is SQL, never peeked"
    );
}

#[test]
fn a_restore_it_refuses_runs_no_tool_and_changes_no_file() {
    let s = pg_stage("");
    s.client("psql", "", "");
    s.client("pg_restore", "", "");
    s.file(
        ".config/easysql/mssql.conf",
        "[reporting]\nhost=db.example.com\nuser=sa\n",
        0o644,
    );
    s.client("sqlcmd", "", "");
    let dump = s.0.join("app.dump");
    fs::write(&dump, b"PGDMP").unwrap();
    let dump = dump.to_str().unwrap().to_string();
    let missing = s.0.join("nope.sql").to_str().unwrap().to_string();
    let before = s.files();

    for (args, why) in [
        (
            vec!["--restore", "ro", &dump],
            "a read-only connection takes no restore",
        ),
        (
            vec!["--restore", "reporting", &dump],
            "SQL Server has no restore tool",
        ),
        (vec!["--restore", "prod", &missing], "the file is missing"),
        (
            vec!["--restore", "prod", "--clean", &dump],
            "a flag where the file goes is not read as the file",
        ),
        (vec!["--restore", "prod"], "there is no file"),
    ] {
        let out = s.esql(&args);
        assert_eq!(out.status.code(), Some(2), "{why}: {out:?}");
        assert!(
            s.files() == before,
            "{why}: the refused run ran a tool or wrote a file"
        );
    }
}
