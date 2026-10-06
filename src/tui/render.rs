//! Drawing the frame: the tab bar, the list body, the detail panel, the status
//! line and help.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};

use super::confirm::render_confirm;
use super::detail::{DETAIL_PCT, detail_fits};
use super::picker::render_picker;
use super::widgets::{BACK, READER_KEYS, SEP, key_footer, vscrollbar};
use super::*;

pub(super) fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_tabs(f, chunks[0], app);
    // The list keeps the whole width until there is room for a panel that does
    // not squeeze it; below that threshold the row itself has to say everything.
    if detail_fits(app, area.width) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(100 - DETAIL_PCT),
                Constraint::Percentage(DETAIL_PCT),
            ])
            .split(chunks[1]);
        render_body(f, cols[0], app);
        render_detail(f, cols[1], app);
    } else {
        render_body(f, chunks[1], app);
    }
    render_status(f, chunks[2], app);

    if app.show_help {
        render_help(f, area, app);
    }
    if let Some(p) = &app.prompt {
        render_prompt(f, area, p, &app.tunnels, &app.settings);
    }
    if let Some(p) = &app.picker {
        render_picker(f, area, p);
    }
    if let Some(c) = &app.confirm {
        render_confirm(f, area, c);
    }
    if let Some(t) = &app.typed {
        super::typed::render_typed(f, area, t);
    }
    // Last, so a failure is never drawn under the thing that caused it.
    if let Some(a) = &app.alert {
        super::alert::render_alert(f, area, a);
    }
}

pub(super) fn render_tabs(f: &mut Frame, area: Rect, app: &App) {
    let idx = VIEWS.iter().position(|v| *v == app.view).unwrap_or(0);
    let tabs = Tabs::new(VIEWS.iter().map(|v| v.title()).collect::<Vec<_>>())
        .select(idx)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" easysql · esql "),
        )
        .divider("│")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

pub(super) fn render_body(f: &mut Frame, area: Rect, app: &mut App) {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let sel = Style::default().add_modifier(Modifier::REVERSED);
    let now = history::now();

    match app.view {
        View::Connections => {
            let rows = app.conn_rows();
            // On a fresh machine there is a second thing missing, and finding
            // that out by pressing Enter is exactly the experience to avoid.
            let no_clients = if app.installed.iter().any(|&i| i) {
                ""
            } else {
                "\n\nNo database client is installed here yet either. easysql is a front end,\nso cargo could not bring one: press Enter on a connection and it offers\nto install the right one for this machine."
            };
            if let Some(msg) = nothing_here(
                app,
                app.conns.len(),
                rows.len(),
                &format!(
                    "No saved connections yet.\nPress `c` to add one: postgres, mysql or sqlite.{no_clients}"
                ),
            ) {
                empty(f, area, "Connections", &msg);
                return;
            }
            // Columns are sized to what is actually on screen, so a filtered
            // list tightens up instead of keeping a hidden row's width.
            let nw = rows
                .iter()
                .map(|&i| app.conns[i].name.len())
                .max()
                .unwrap_or(0);
            let ew = rows
                .iter()
                .map(|&i| app.conns[i].engine.label().len())
                .max()
                .unwrap_or(0);
            let tw = rows
                .iter()
                .map(|&i| app.conns[i].target().len())
                .max()
                .unwrap_or(0)
                .min(38)
                // Never wider than what is left once every column that must survive has had
                // its share, or the age clips to `3m a` and reads as a rendering fault. The
                // block's borders and the highlight symbol eat width that `area` still counts.
                .min({
                    let usable = (area.width as usize).saturating_sub(2 + 2);
                    let fixed = 1 + 1 + nw + 2 + ew + 2 + 2 + 9 + 9;
                    usable.saturating_sub(fixed).max(12)
                });
            let items: Vec<ListItem> = rows
                .iter()
                .map(|&i| {
                    let c = &app.conns[i];
                    let (mark, style) = reach_mark(
                        app.reach.get(&c.key()),
                        c.engine.networked(),
                        super::detail::sleeping_via(c).is_some(),
                    );
                    let age = match app.history.get(&c.key()) {
                        Some(e) => history::ago(e.last, now),
                        None => String::new(),
                    };
                    // The two things that decide whether Enter just works: is
                    // the client here, and is the password already on file.
                    let (flag, flag_style) = if !app.installed[c.engine.idx()] {
                        ("no client", Style::default().fg(Color::Red))
                    } else if app.has_password(c) {
                        ("pw", Style::default().fg(Color::Green))
                    } else {
                        ("", Style::default())
                    };
                    // The detail panel says in words what this colour means.
                    let name_style = if c.read_only() {
                        bold.fg(READ_ONLY_COLOR)
                    } else {
                        bold
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(mark, style),
                        Span::raw(" "),
                        Span::styled(format!("{:nw$}", c.name), name_style),
                        Span::raw("  "),
                        Span::styled(
                            format!("{:ew$}", c.engine.label()),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw("  "),
                        Span::styled(fit(&c.target(), tw), dim),
                        Span::raw("  "),
                        Span::styled(format!("{flag:9}"), flag_style),
                        Span::styled(age, dim),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(counted("Connections", rows.len(), app.conns.len()))
                .highlight_style(sel)
                .highlight_symbol("▸ ");
            f.render_stateful_widget(list, area, &mut app.conn_state);
            list_bar(f, area, rows.len(), app.conn_state.offset());
        }

        View::Passwords => {
            let rows = app.cred_rows();
            if let Some(msg) = nothing_here(
                app,
                app.creds.len(),
                rows.len(),
                "No saved passwords.\nPress `c`, or `p` on a connection, to save one where its client already looks:\n~/.pgpass for postgres, ~/.my.cnf for mysql.",
            ) {
                empty(f, area, "Passwords", &msg);
                return;
            }
            let items: Vec<ListItem> = rows
                .iter()
                .map(|&i| {
                    let c = &app.creds[i];
                    ListItem::new(Line::from(vec![
                        Span::styled(c.describe(), bold),
                        Span::raw("  "),
                        // Never the secret, only the fact that there is one.
                        Span::styled("••••••", Style::default().fg(Color::Green)),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(counted("Passwords", rows.len(), app.creds.len()))
                .highlight_style(sel)
                .highlight_symbol("▸ ");
            f.render_stateful_widget(list, area, &mut app.cred_state);
            list_bar(f, area, rows.len(), app.cred_state.offset());
        }

        View::Tunnels => {
            let rows = app.tunnel_rows();
            if let Some(msg) = nothing_here(
                app,
                app.tunnels.len(),
                rows.len(),
                "No tunnels yet.\nOpen one from the Connections tab with `t`, for a database\nyou can only reach from inside the network.",
            ) {
                empty(f, area, "Tunnels", &msg);
                return;
            }
            let items: Vec<ListItem> = rows
                .iter()
                .map(|&i| {
                    let t = &app.tunnels[i];
                    // Name the connection this was dug for, resolved back to a real one rather
                    // than shown as a raw `pg:name` key. An orphan is called out: it means the
                    // connection was deleted or renamed outside easysql.
                    let owner = match t.owner.clone() {
                        Some(key) => match app.conns.iter().find(|c| c.key() == key) {
                            Some(c) => format!("   for {} ({})", c.name, c.engine.label()),
                            None => "   for a connection that no longer exists".to_string(),
                        },
                        None => String::new(),
                    };
                    // Cut to the pane: a row that runs long is silently
                    // truncated by the terminal mid-word, which reads as a
                    // rendering fault.
                    let room = (area.width as usize).saturating_sub(2 + 2);
                    let body = t.describe();
                    let owner = fit_end(&owner, room.saturating_sub(body.chars().count()));
                    ListItem::new(Line::from(vec![
                        Span::raw(body),
                        Span::styled(owner, Style::default().add_modifier(Modifier::DIM)),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(counted("Tunnels", rows.len(), app.tunnels.len()))
                .highlight_style(sel)
                .highlight_symbol("▸ ");
            f.render_stateful_widget(list, area, &mut app.tunnel_state);
            list_bar(f, area, rows.len(), app.tunnel_state.offset());
        }

        View::Snippets => {
            let rows = app.snippet_rows();
            if let Some(msg) = nothing_here(
                app,
                app.snippets.len(),
                rows.len(),
                "No saved queries.\nPress `c` to write one, then run it against any connection\nwith `esql <name> :<snippet>`.",
            ) {
                empty(f, area, "Snippets", &msg);
                return;
            }
            let nw = rows
                .iter()
                .map(|&i| app.snippets[i].name.len())
                .max()
                .unwrap_or(0)
                .min(20);
            let items: Vec<ListItem> = rows
                .iter()
                .map(|&i| {
                    let s = &app.snippets[i];
                    let room = (area.width as usize).saturating_sub(2 + 2 + nw + 2 + 12 + 2);
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:nw$}", s.name), bold),
                        Span::raw("  "),
                        Span::styled(
                            format!("{:12}", format!(":{}", s.name)),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw("  "),
                        Span::styled(fit_end(&s.summary(), room), dim),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(counted("Snippets", rows.len(), app.snippets.len()))
                .highlight_style(sel)
                .highlight_symbol("▸ ");
            f.render_stateful_widget(list, area, &mut app.snippet_state);
            list_bar(f, area, rows.len(), app.snippet_state.offset());
        }

        View::Settings => {
            let rows = app.settings_rows();
            let total = app.settings.rows().len();
            if let Some(msg) = nothing_here(app, total, rows.len(), "No settings.") {
                empty(f, area, "Settings", &msg);
                return;
            }
            let w = rows.iter().map(|r| r.label.len()).max().unwrap_or(0);
            // Sized to the widest value on screen: a long command must not run
            // into the help text beside it.
            let vw = rows.iter().map(|r| r.value.len()).max().unwrap_or(0);
            let gw = rows
                .iter()
                .map(|r| r.group.label().len())
                .max()
                .unwrap_or(0);
            let mut last_group = None;
            let items: Vec<ListItem> = rows
                .iter()
                .map(|r| {
                    // The group is named once, on its first row, so the two
                    // kinds read as two blocks without a header you can land on.
                    let group = if last_group == Some(r.group) {
                        String::new()
                    } else {
                        r.group.label().to_string()
                    };
                    last_group = Some(r.group);
                    // A value you chose is worth picking out from one that just
                    // came with the program.
                    let value_style = if r.is_default() {
                        Style::default()
                    } else {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{group:gw$}"), dim),
                        Span::raw("  "),
                        Span::styled(format!("{:w$}", r.label), bold),
                        Span::raw("  "),
                        Span::styled(format!("{:vw$}", r.value), value_style),
                        Span::raw("  "),
                        // Cut to what is left of the pane: a help line that
                        // overruns is chopped mid-word by the terminal, which
                        // looks like a bug rather than a long sentence.
                        Span::styled(
                            fit_end(
                                r.help,
                                (area.width as usize)
                                    .saturating_sub(2 + 2 + gw + 2 + w + 2 + vw + 2),
                            ),
                            dim,
                        ),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(counted("Settings", rows.len(), total))
                .highlight_style(sel)
                .highlight_symbol("▸ ");
            f.render_stateful_widget(list, area, &mut app.settings_state);
            list_bar(f, area, rows.len(), app.settings_state.offset());
        }
    }
}

/// The scrollbar of a bordered list pane, after the list is drawn so its
/// `offset` is the one on screen.
fn list_bar(f: &mut Frame, area: Rect, total: usize, offset: usize) {
    vscrollbar(
        f,
        area,
        total,
        offset,
        area.height.saturating_sub(2) as usize,
    );
}

/// The message for an empty pane, or `None` when there are rows to draw. An
/// empty list and a filter that matched nothing are different problems, so they
/// get different words.
fn nothing_here(app: &App, total: usize, shown: usize, when_empty: &str) -> Option<String> {
    if total == 0 {
        Some(when_empty.to_string())
    } else if shown == 0 {
        Some(format!(
            "Nothing matches '{}'.\nEsc clears the filter.",
            app.query
        ))
    } else {
        None
    }
}

/// `Name (shown/total)` while a filter is on, plain `Name (n)` otherwise.
fn counted(name: &str, shown: usize, total: usize) -> Block<'static> {
    if shown == total {
        titled(name, total)
    } else {
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {name} ({shown}/{total}) "))
    }
}

/// The dot in front of a connection: filled and coloured once we know, hollow
/// while we are still asking. A SQLite file has no port and gets no dot.
/// Cut to at most `w` without padding, for a trailing column that should simply
/// disappear when there is no room rather than push the row over.
fn fit_end(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    if w < 2 {
        return String::new();
    }
    let mut out: String = s.chars().take(w - 1).collect();
    out.push('…');
    out
}

/// Pad to exactly `w`, and cut with an ellipsis rather than overflowing: a
/// column that is allowed to run long pushes the ones after it off the row.
fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return format!("{s:w$}");
    }
    let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn reach_mark(reach: Option<&Reach>, networked: bool, sleeping_via: bool) -> (&'static str, Style) {
    // A closed port in front of a tunnel we can reopen is not a server that is
    // gone. Yellow on the same glyph rather than a rarer codepoint, which is one
    // font fallback from rendering as a stray `‹`. Up counts too, and matters
    // more: a port answering while the forward is down is answering for something
    // else.
    if sleeping_via && matches!(reach, Some(Reach::Down) | Some(Reach::Up(_))) {
        return ("●", Style::default().fg(Color::Yellow));
    }
    if !networked {
        return (" ", Style::default());
    }
    match reach {
        Some(Reach::Up(_)) => ("●", Style::default().fg(Color::Green)),
        Some(Reach::Down) => ("●", Style::default().fg(Color::Red)),
        _ => ("○", Style::default().add_modifier(Modifier::DIM)),
    }
}

pub(super) fn render_status(f: &mut Frame, area: Rect, app: &App) {
    // While `/` is being typed the line belongs to the query: it is the only
    // place what you typed is visible.
    if app.searching {
        let mut spans = vec![Span::styled(
            " /",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )];
        spans.extend(line_edit::with_cursor(
            &app.query,
            app.query_back,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        // Every letter goes into the query here, so only keys that are not
        // letters are offered.
        spans.push(Span::styled(
            format!("   {} match   ↵ keep{SEP}{BACK}", app.row_count()),
            Style::default().add_modifier(Modifier::DIM),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    // Show the last action's result while it is fresh; otherwise the keys, so a
    // stale message never masquerades as the current state.
    let line = match app.live_status() {
        // Green for what worked, yellow for what did not, and never red: red
        // means a gate in front of something you are about to lose.
        Some(msg) => Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(if app.status_failed {
                Color::Yellow
            } else {
                Color::Green
            }),
        )),
        None => {
            let keys = match app.view {
                View::Connections => CONN_KEYS,
                View::Passwords => PASS_KEYS,
                View::Tunnels => TUNNELS_KEYS,
                View::Snippets => SNIP_KEYS,
                View::Settings => SETTINGS_KEYS,
            };
            // A committed filter stays visible in front of the keys: rows are
            // hidden, and nothing else on screen would say why.
            let lead = match app.query.is_empty() {
                true => Vec::new(),
                false => vec![format!("/{}", app.query), BACK.to_string()],
            };
            key_footer(&lead, keys, area.width)
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

/// One group of the help panel: a heading, then `(keys, what they do)` rows,
/// where a row with no keys is a note about the group.
type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);

/// Every key the app answers to, grouped by where it works. The panel scrolls,
/// so a new row costs nothing but its line.
const HELP: &[HelpSection] = &[
    (
        "moving",
        &[
            ("j/k ↑↓", "move in the list"),
            ("h/l ←→", "the previous, next tab"),
            ("tab shift-tab", "the next, previous tab"),
            ("ctrl-j/k/h/l", "the same, from anywhere"),
        ],
    ),
    (
        "every tab",
        &[
            ("/", "find in the list, esc drops it"),
            ("r", "refresh what the tab shows"),
            ("?", "this help"),
            ("q ctrl-c", "quit"),
        ],
    ),
    (
        "connections",
        &[
            ("↵", "open it in its own client (\\q comes back here)"),
            ("c", "create a connection"),
            ("e", "edit it"),
            ("d", "delete it"),
            ("y", "yank the command"),
            ("Y", "yank the url"),
            ("p", "save its password"),
            ("t", "reach it through ssh (ssh -L)"),
            (
                "",
                "● up · ● down · ● tunnel closed · ○ checking · pw saved",
            ),
        ],
    ),
    (
        "passwords",
        &[
            ("c", "create a password"),
            ("e", "edit it"),
            ("d", "delete it"),
            ("o", "open the file in $EDITOR"),
        ],
    ),
    (
        "tunnels",
        &[
            ("↵", "on/off (ssh -N)"),
            ("d", "delete it, stopping it first"),
        ],
    ),
    (
        "snippets",
        &[
            ("c", "create a snippet"),
            ("e", "edit it"),
            ("d", "delete it"),
            ("o", "open the file in $EDITOR"),
        ],
    ),
    (
        "settings",
        &[("↵ e", "change it"), ("d", "put it back to its default")],
    ),
    (
        "in a form",
        &[
            ("type", "fill the field, h/j/k/l included"),
            ("ctrl-j/k ↑↓", "the previous, next field"),
            ("tab shift-tab", "the next, previous field"),
            ("h/l ←→", "step a ‹ choice ›"),
            ("ctrl-o", "pick the ssh host for a tunnel"),
            ("↵", "the next field, and submit on the last"),
            ("esc", "cancel"),
        ],
    ),
    (
        "in a box",
        &[
            ("y n", "answer"),
            ("h/l ←→ tab", "move between the buttons"),
            ("↵", "select, or pick from a list"),
            ("j/k ↑↓", "move in a list, scroll an alert"),
            ("esc", "cancel or close"),
        ],
    ),
    (
        "in this help",
        &[
            ("j/k ↑↓", "scroll"),
            ("ctrl-d ctrl-u", "half a page down, up"),
            ("g G", "the top, the bottom"),
            ("esc q ?", "close"),
        ],
    ),
];

/// The width of the key column, so every description starts in one place.
const HELP_KEYS: usize = 16;

pub(super) fn help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (section, entries) in HELP {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            *section,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        for (keys, what) in *entries {
            if keys.is_empty() {
                lines.push(Line::styled(
                    format!("  {what}"),
                    Style::default().add_modifier(Modifier::DIM),
                ));
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {keys:<HELP_KEYS$}"),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(*what),
            ]));
        }
    }
    lines
}

/// The help reader: the body scrolls under a key row that never moves, with a
/// scrollbar on the right border once it is taller than the box.
pub(super) fn render_help(f: &mut Frame, area: Rect, app: &mut App) {
    let lines = help_lines();
    let width = box_width(area.width);
    // The body, then a blank and the key row.
    let rect = box_area(area, width, box_height(lines.len() as u16 + 2, area.height));
    f.render_widget(Clear, rect);
    let block = box_block(Color::Cyan, "help");
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let shown = inner.height.saturating_sub(2) as usize;
    // Clamped here, where the height is known, so scrolling past the end never
    // piles up presses that then take as many to undo.
    app.help_scroll = app.help_scroll.min(lines.len().saturating_sub(shown));
    let top = app.help_scroll;
    let body = Rect {
        height: shown as u16,
        ..inner
    };
    f.render_widget(Paragraph::new(lines[top..].to_vec()), body);
    let keys = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    f.render_widget(Paragraph::new(box_hint(READER_KEYS)), keys);
    if lines.len() > shown {
        vscrollbar(f, rect, lines.len(), top, shown);
    }
}
