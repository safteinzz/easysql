//! The panel beside the list: everything known about the selected row, so the
//! answers you would otherwise go looking for (is the server up, when was I
//! last in it, is there a password on file, which file does this even live in)
//! are on screen already.
//!
//! It only appears when the terminal is wide enough to keep a readable list
//! next to it; below that the list gets the whole width. It reads only what is
//! already loaded: never a fresh query, and never a connection of its own.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::widgets::shell_join_display;
use super::*;

/// Narrowest total width that still leaves a usable list beside the panel.
pub(super) const MIN_WIDTH_FOR_DETAIL: u16 = 96;
/// The panel's share of the frame when it is shown.
pub(super) const DETAIL_PCT: u16 = 38;

/// Does this view have a detail panel, and is there room for it? Every view
/// does; the width is what decides.
pub(super) fn detail_fits(_app: &App, width: u16) -> bool {
    width >= MIN_WIDTH_FOR_DETAIL
}

pub(super) fn render_detail(f: &mut Frame, area: Rect, app: &App) {
    let lines = match app.view {
        View::Connections => conn_lines(app),
        View::Passwords => cred_lines(app),
        View::Tunnels => tunnel_lines(app),
        View::Settings => setting_lines(app),
    };
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Details ")
                .padding(Padding::horizontal(1)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn conn_lines(app: &App) -> Vec<Line<'static>> {
    let Some(c) = app.selected_conn() else {
        return vec![Line::styled("nothing selected", dim())];
    };
    let mut lines = vec![
        Line::styled(c.name.clone(), heading()),
        Line::styled(c.engine.label().to_string(), dim()),
        Line::raw(""),
    ];

    // State first: the things that decide whether you press Enter at all.
    if c.engine.networked() {
        lines.push(reach_line(app.reach.get(&c.key()), sleeping_via(c)));
    }
    lines.push(match app.history.get(&c.key()) {
        Some(e) => Line::styled(
            format!(
                "last {} · opened {} time{}",
                history::ago(e.last, history::now()),
                e.count,
                if e.count == 1 { "" } else { "s" }
            ),
            dim(),
        ),
        None => Line::styled("never opened from here", dim()),
    });
    lines.push(Line::raw(""));

    if c.engine == Engine::Sqlite {
        lines.push(row("File", shorten(&c.database)));
        // The one thing worth knowing about a file connection, and it costs a
        // stat on a local path rather than any network call.
        let exists = crate::ini::expand_tilde(&c.database).exists();
        lines.push(Line::styled(
            if exists {
                "● the file is there".to_string()
            } else {
                "● no such file yet (sqlite3 would create it)".to_string()
            },
            if exists {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Yellow)
            },
        ));
    } else {
        for (label, value) in [
            ("Host", c.host.clone()),
            ("Port", c.port_or_default()),
            ("Database", c.database.clone()),
            ("User", c.user.clone()),
        ] {
            if !value.is_empty() {
                lines.push(row(label, value));
            }
        }
        // Keys somebody wrote by hand that we carry through but never ask about.
        for (k, v) in &c.extra {
            lines.push(row(k, v.clone()));
        }
        lines.push(Line::raw(""));
        lines.push(if !c.engine.stores_password() {
            Line::styled(
                format!("{} asks for the password itself", c.engine.default_client()),
                dim(),
            )
        } else if app.has_password(c) {
            Line::styled(
                "password on file (the client reads it itself)",
                Style::default().fg(Color::Green),
            )
        } else {
            Line::styled("no password saved (p saves one)", dim())
        });
    }

    // What this connection is currently doing, pulled from the Tunnels tab so
    // you do not have to go and look.
    let via: Vec<String> = app
        .tunnels
        .iter()
        // Host *and* port: every forward on this machine points at 127.0.0.1
        // somewhere, so matching the host alone lists every unrelated tunnel
        // under every local connection.
        .filter(|t| {
            t.ports()
                .map(|(_, target, port)| target == c.host && port == c.port_or_default())
                .unwrap_or(false)
        })
        .map(|t| {
            format!(
                "localhost:{} via {}",
                t.ports().map(|(o, _, _)| o).unwrap_or(""),
                t.host
            )
        })
        .collect();
    if !via.is_empty() {
        lines.push(Line::raw(""));
        lines.push(row("Tunnels", via.join(", ")));
    }

    // A remembered forward is the difference between this connection working
    // after a reboot and failing with a refusal that explains nothing, so it is
    // shown whether or not it happens to be up right now - and it says which,
    // because "needs a tunnel" and "has one" are different situations.
    if let Some(v) = crate::vias::get(&c.key()) {
        let live = crate::tunnels::carrying(&v.local).is_some();
        lines.push(Line::raw(""));
        lines.push(row("Needs tunnel", format!("through {}", v.host)));
        lines.push(Line::styled(
            if live {
                "  open now".to_string()
            } else {
                format!("  not open: Enter reopens it, {}", v.command())
            },
            dim(),
        ));
        // The two can disagree, and silently opening a forward nobody uses is
        // worse than saying so.
        if !crate::vias::points_at_it(c, &v) {
            lines.push(Line::styled(
                format!(
                    "  but this connects to {}:{}, not localhost:{} - e to repoint it",
                    c.host,
                    c.port_or_default(),
                    v.local
                ),
                Style::default().fg(Color::Yellow),
            ));
        }
    }

    lines.push(Line::raw(""));
    lines.push(row(
        "Defined in",
        shorten(&c.engine.store().to_string_lossy()),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        shell_join_display(&c.connect_argv(&app.settings)),
        Style::default().fg(Color::Green),
    ));
    lines.push(Line::styled(format!("esql {}", c.name), dim()));
    lines
}

fn cred_lines(app: &App) -> Vec<Line<'static>> {
    let Some(c) = app.selected_cred() else {
        return vec![Line::styled("nothing selected", dim())];
    };
    let mut lines = vec![
        Line::styled(c.user.clone(), heading()),
        Line::styled(c.engine.label().to_string(), dim()),
        Line::raw(""),
    ];
    lines.push(row("Host", c.host.clone()));
    lines.push(row("Port", c.port.clone()));
    lines.push(row("Database", c.database.clone()));
    lines.push(row("User", c.user.clone()));
    lines.push(Line::raw(""));
    lines.push(row("Stored in", c.where_stored()));
    lines.push(Line::raw(""));

    // Which connections this actually unlocks: a `.pgpass` line with a `*` in
    // it covers more than the one you made it for, and that is worth seeing.
    let covers: Vec<String> = app
        .conns
        .iter()
        .filter(|conn| c.covers(conn))
        .map(|conn| conn.name.clone())
        .collect();
    lines.push(match covers.is_empty() {
        true => Line::styled("covers no saved connection", dim()),
        false => row("Unlocks", covers.join(", ")),
    });

    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "the password itself is never read back, only written",
        dim(),
    ));
    lines
}

fn tunnel_lines(app: &App) -> Vec<Line<'static>> {
    let Some(t) = app.selected_tunnel() else {
        return vec![Line::styled("nothing selected", dim())];
    };
    let mut lines = vec![
        Line::styled(format!("-{} {}", t.kind, t.spec), heading()),
        Line::raw(""),
    ];

    // What it is for, before what it is made of.
    lines.push(Line::raw(t.explain()));
    lines.push(Line::raw(""));

    lines.push(row("Through", t.host.clone()));
    lines.push(row("Process", format!("pid {}", t.pid)));
    if let Some(started) = t.started_at() {
        // The log file is created as the tunnel is spawned, so its age is the
        // tunnel's age.
        let secs = started
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        lines.push(row("Opened", history::ago(secs, history::now())));
    }

    // A forward that half-died is otherwise silent: ssh's own words are the
    // only explanation there is.
    let stderr = t.stderr();
    if !stderr.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(stderr, Style::default().fg(Color::Yellow)));
    }

    if let Some((open, _, _)) = t.ports() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("point a connection at localhost:{open}"),
            dim(),
        ));
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(t.command(), Style::default().fg(Color::Green)));
    lines.push(Line::styled(format!("kill {}", t.pid), dim()));
    lines
}

fn setting_lines(app: &App) -> Vec<Line<'static>> {
    let Some(r) = app.selected_setting() else {
        return vec![Line::styled("nothing selected", dim())];
    };
    let mut lines = vec![Line::styled(r.label.to_string(), heading()), Line::raw("")];
    lines.push(Line::styled(r.help.to_string(), dim()));
    lines.push(Line::raw(""));
    lines.push(row("Now", r.value.clone()));
    lines.push(row("Default", r.default.clone()));
    if let Some(choices) = r.choices {
        lines.push(row("Choices", choices.join(" · ")));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        match r.choices {
            Some(_) => "↵ cycles it · d puts it back".to_string(),
            None => "↵ types it · d puts it back".to_string(),
        },
        dim(),
    ));
    lines.push(Line::raw(""));
    // The file is the real store; say where it is so it can be edited or
    // version-controlled like anything else.
    lines.push(Line::styled(
        shorten(&settings::path().to_string_lossy()),
        dim(),
    ));
    lines.push(row("Key", format!("{} = {}", r.key, r.value)));
    lines
}

/// One `label   value` row, with the label dim so the value reads first.
fn row(name: &str, value: String) -> Line<'static> {
    Line::from(vec![label(name), Span::raw(value)])
}

/// The dim, fixed-width label column that keeps every value aligned. A label
/// that fills the column still gets one space after it, or a long one runs
/// straight into its value ("Needs tunnelthrough bastion").
fn label(name: &str) -> Span<'static> {
    const W: usize = 11;
    let pad = if name.len() >= W { 1 } else { W - name.len() };
    Span::styled(format!("{name}{:pad$}", "", pad = pad), dim())
}

/// A connection whose forward is recorded but not running: the port really is
/// closed, so the probe is not wrong, but "down" would blame the database for a
/// tunnel that simply is not open yet - and one keypress fixes it.
pub(super) fn sleeping_via(c: &crate::engines::Conn) -> Option<String> {
    let v = crate::vias::get(&c.key())?;
    crate::tunnels::carrying(&v.local)
        .is_none()
        .then_some(v.host)
}

fn reach_line(reach: Option<&Reach>, sleeping: Option<String>) -> Line<'static> {
    if let (Some(Reach::Down), Some(host)) = (reach, sleeping.as_ref()) {
        return Line::styled(
            format!("● tunnel to {host} is not open · Enter reopens it and connects"),
            Style::default().fg(Color::Yellow),
        );
    }
    match reach {
        // A loopback or LAN answer rounds to zero, and "answered in 0 ms" reads
        // like a missing number rather than a fast one.
        Some(Reach::Up(0)) => Line::styled(
            "● up · answered instantly".to_string(),
            Style::default().fg(Color::Green),
        ),
        Some(Reach::Up(ms)) => Line::styled(
            format!("● up · answered in {ms} ms"),
            Style::default().fg(Color::Green),
        ),
        Some(Reach::Down) => Line::styled(
            "● down · nothing listening on that port",
            Style::default().fg(Color::Red),
        ),
        Some(Reach::Probing) => Line::styled("○ checking the port…", dim()),
        None => Line::styled("○ not checked (r to check)", dim()),
    }
}

/// `/home/you/db.sqlite` -> `~/db.sqlite`, which is how you wrote it.
fn shorten(path: &str) -> String {
    crate::ini::collapse_tilde(path)
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn heading() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}
