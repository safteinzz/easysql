//! Keeps the README intro honest.
//!
//! Cargo.toml's `description` is the single source: crates.io reads it, clap's
//! `about` derives from it, and the README block between the markers below has
//! to match. This test fails if they drift.
//!
//! To fix a failure, don't edit the README by hand:
//!
//!     UPDATE_README=1 cargo test
//!
//! which rewrites the block, same convention as snapshot testing.

use std::fs;
use std::path::Path;

const START: &str = "<!-- desc:start -->";
const END: &str = "<!-- desc:end -->";

#[test]
fn readme_description_matches_cargo_toml() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(&path).expect("README.md should exist");
    let want = env!("CARGO_PKG_DESCRIPTION");

    let (head, rest) = readme
        .split_once(START)
        .unwrap_or_else(|| panic!("README.md is missing {START}"));
    let (block, tail) = rest
        .split_once(END)
        .unwrap_or_else(|| panic!("README.md is missing {END}"));

    if block.trim() == want {
        return;
    }

    if std::env::var_os("UPDATE_README").is_some() {
        fs::write(&path, format!("{head}{START}\n{want}\n{END}{tail}"))
            .expect("failed to write README.md");
        return;
    }

    panic!(
        "README.md description is out of date.\n  \
         cargo.toml: {want}\n  \
         readme:     {}\n\n\
         run `UPDATE_README=1 cargo test` to fix it.",
        block.trim()
    );
}
