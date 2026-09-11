//! A run easysql refuses before the client starts leaves nothing behind: no
//! history stamp, and no client run. Each case is the real binary in a HOME of
//! its own under the temp dir, with only stand-in clients on its PATH.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// A HOME, config and state dir, and a PATH of stand-ins, all in one temp
/// directory that is deleted when the case is done.
struct Stage(PathBuf);

impl Stage {
    fn new(settings: &str) -> Stage {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("easysql-refused-{}-{stamp}", std::process::id()));
        fs::create_dir_all(root.join(".config/easysql")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join(".pg_service.conf"),
            "[prod]\nhost=127.0.0.1\nport=1\n\n\
             [ro]\nhost=127.0.0.1\nport=1\noptions=-c default_transaction_read_only=on\n",
        )
        .unwrap();
        fs::write(root.join(".config/easysql/settings"), settings).unwrap();
        Stage(root)
    }

    /// A client on the stage's PATH that records what it was handed and prints
    /// `answer`, so a case can tell whether it ran and for what.
    fn client(&self, name: &str, answer: &str) {
        let path = self.0.join("bin").join(name);
        let log = self.0.join(format!("{name}.log"));
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\necho {answer}\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn ran(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.0.join(format!("{name}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
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
        fs::read_to_string(history_in(&self.0)).ok()
    }
}

fn history_in(home: &Path) -> PathBuf {
    home.join(".local/state/easysql/history")
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_run_refused_before_the_client_starts_leaves_no_history() {
    // The configured client is not installed at all.
    let missing = Stage::new("psql_command=nosuchclient\n");
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
    let foreign = Stage::new("psql_command=pgcli\n");
    foreign.client("pgcli", "");
    let out = foreign.esql(&["prod", "select 1"]);
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    assert!(
        foreign.ran("pgcli").is_empty(),
        "pgcli must not be handed a -c"
    );
    assert_eq!(foreign.history(), None);

    // The connection is marked read-only but the server says it could write,
    // which is what a pooler that drops `options` looks like.
    let writable = Stage::new("");
    writable.client("psql", "off");
    let out = writable.esql(&["ro", "truncate orders"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let ran = writable.ran("psql");
    assert_eq!(
        ran.len(),
        1,
        "psql is asked the question and never handed the query: {ran:?}"
    );
    assert!(ran[0].contains("default_transaction_read_only"), "{ran:?}");
    assert_eq!(writable.history(), None);
}
