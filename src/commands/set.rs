//! `esql set`: the Settings tab without the terminal, through the same
//! `Settings::set`, `reset` and `rows()` the tab uses, so a key and its choices
//! are spelled the same in both.

use super::{DryRun, Json, fail, json_array, json_opt, json_str, said, shown, step};
use colored::Colorize;

/// Under `esql set --help`: what `--json` prints.
pub const JSON_FIELDS: &str = "\
--json prints an array with one object per setting, and these names are fixed:
  key      what `esql set <key> <value>` takes
  value    what it is now
  default  what `--default` puts back
  choices  the values it takes, or null when it is typed
  help     what it does";

#[derive(clap::Args)]
pub struct Args {
    /// The setting, as `esql set` lists it, shown alone when no value follows
    #[arg(value_name = "KEY",
          add = clap_complete::ArgValueCompleter::new(crate::completion::setting_keys))]
    key: Option<String>,
    /// Its new value, one of its choices where `esql set` lists some
    #[arg(value_name = "VALUE")]
    value: Option<String>,
    /// Put the setting back the way it ships
    #[arg(long, requires = "key", conflicts_with = "value")]
    default: bool,
    #[command(flatten)]
    json: Json,
    #[command(flatten)]
    dry: DryRun,
}

pub fn run(args: Args) {
    let mut settings = crate::settings::load();
    let rows = settings.rows();
    let row = args.key.as_deref().map(|key| {
        rows.iter().find(|r| r.key == key).unwrap_or_else(|| {
            let keys: Vec<&str> = rows.iter().map(|r| r.key).collect();
            fail(&format!(
                "there is no setting `{key}`; these are: {}",
                keys.join(", ")
            ))
        })
    });
    let path = shown(&crate::settings::path());

    match (row, args.value, args.default) {
        (Some(row), _, true) => {
            step(&format!(
                "set {} back to {:?} in {path}",
                row.key, row.default
            ));
            if args.dry.dry_run {
                return;
            }
            settings.reset(row.key);
            save(&settings);
        }
        (Some(row), Some(value), false) => {
            if let Some(choices) = row.choices
                && !choices.contains(&value.as_str())
            {
                fail(&format!("`{}` takes {}", row.key, choices.join("|")));
            }
            // What the file will hold, which is not always what was typed: an
            // empty command falls back to the client's own name.
            let mut after = crate::settings::load();
            after.set(row.key, &value);
            let written = after
                .rows()
                .into_iter()
                .find(|r| r.key == row.key)
                .map_or(value.clone(), |r| r.value);
            step(&format!("set {} = {written:?} in {path}", row.key));
            if args.dry.dry_run {
                return;
            }
            settings.set(row.key, &value);
            save(&settings);
        }
        (row, None, false) => {
            let shown: Vec<&crate::settings::Row> = match row {
                Some(r) => vec![r],
                None => rows.iter().collect(),
            };
            list(&shown, args.json.json);
        }
        (None, _, _) => fail("give the setting to change: `esql set <key> <value>`"),
    }
}

fn save(settings: &crate::settings::Settings) {
    if let Err(e) = settings.save() {
        fail(&format!("could not save the setting: {}", said(&e)));
    }
}

fn list(rows: &[&crate::settings::Row], json: bool) {
    if json {
        let out: Vec<String> = rows
            .iter()
            .map(|r| {
                let choices = r.choices.map(|c| {
                    let quoted: Vec<String> = c.iter().map(|v| json_str(v)).collect();
                    format!("[{}]", quoted.join(", "))
                });
                format!(
                    "{{\"key\": {}, \"value\": {}, \"default\": {}, \"choices\": {}, \"help\": {}}}",
                    json_str(r.key),
                    json_str(&r.value),
                    json_str(&r.default),
                    choices.unwrap_or_else(|| json_opt(None)),
                    json_str(r.help)
                )
            })
            .collect();
        println!("{}", json_array(&out));
        return;
    }
    let kw = rows.iter().map(|r| r.key.len()).max().unwrap_or(0);
    let vw = rows.iter().map(|r| r.value.len()).max().unwrap_or(0);
    for r in rows {
        let takes = match r.choices {
            Some(c) => c.join("|"),
            None => "typed".to_string(),
        };
        println!(
            "{:kw$}  {:vw$}  {}",
            r.key,
            r.value,
            format!("{} ({takes})", r.help).dimmed()
        );
    }
}
