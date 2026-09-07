//! The toolbox - bare `esql`. Four tabs (Connections / Passwords / Tunnels /
//! Settings) and a set of wizards that stand in for the things nobody
//! remembers: a `~/.pg_service.conf` block, a `~/.pgpass` line, a `[clientX]`
//! group, and `ssh -L port:host:port bastion`.
//!
//! Opening a connection runs *suspended*: we drop out of the alternate screen,
//! hand the real terminal to psql/mysql/sqlite3 so it owns its own prompt,
//! pager and readline, then restore the UI when you `\q`. Tunnels are spawned
//! detached and tracked, so they don't need that.

use crate::creds::{self, Cred};
use crate::engines::{self, Conn, Engine};
use crate::history;
use crate::reach::{self, Reach};
use crate::settings::{self, ConnOrder, Settings};
use crate::tunnels;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::{ListState, Padding};
use std::collections::HashMap;
use std::io::{self, Stdout};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

type Term = Terminal<CrosstermBackend<Stdout>>;

mod alert;
mod confirm;
mod detail;
mod filter;
mod input;
mod picker;
mod probe;
mod prompt;
mod render;
mod widgets;
mod wizard;

use confirm::Confirm;
use detail::render_detail;
use picker::Picker;
use prompt::{Action, Kind, Prompt, render_prompt};
use render::ui;
use widgets::{
    box_area, box_block, box_buttons, box_height, box_hint, box_inner_width, box_width, empty,
    titled, wrapped_line_count,
};

/// The tabs. Order here is the left-to-right / Tab-cycle order.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum View {
    Connections,
    Passwords,
    Tunnels,
    Snippets,
    Settings,
}

impl View {
    /// The tab's label. Lives on the view itself so the strip is built from
    /// `VIEWS` rather than from a second list beside it: a parallel array is how
    /// a new tab ends up rendering under the previous one's name.
    pub(super) fn title(self) -> &'static str {
        match self {
            View::Connections => "Connections",
            View::Passwords => "Passwords",
            View::Tunnels => "Tunnels (ssh -L)",
            View::Snippets => "Snippets",
            View::Settings => "Settings",
        }
    }
}

const VIEWS: [View; 5] = [
    View::Connections,
    View::Passwords,
    View::Tunnels,
    View::Snippets,
    View::Settings,
];

// The bottom bar is a terse reminder of this view's actions only; `?` opens the
// full cheat-sheet (navigation keys and the real command behind each action), so
// the bar stays short instead of restating everything and overflowing.
const CONN_HINTS: &str = "↵ open · c new · e edit · d del · p password · t tunnel · y yank · Y url · r refresh · / find · ? help";
const SNIP_HINTS: &str = "c new · e edit · o open the file · d delete · r reload · / find · ? help";
const PASS_HINTS: &str =
    "c new · e edit · o open the file · d forget · r refresh · / find · ? help";
const TUNNELS_HINTS: &str = "↵ on/off · d stop · r refresh · / find · ? help";
const SETTINGS_HINTS: &str = "↵ change · d back to default · r reload · ? help";

/// How long a status message stays on screen before the hints return.
const STATUS_TTL: Duration = Duration::from_secs(3);

/// An external command the event loop must run *suspended* (outside the TUI) so
/// it can own the terminal - which here is only ever a database client.
pub(super) struct PendingRun {
    pub(super) argv: Vec<String>,
    pub(super) label: String,
    /// The connection, when this run is an interactive session. Carried whole
    /// rather than by name because a failed open is diagnosed against it.
    pub(super) connect: Option<Conn>,
}

pub(super) struct App {
    pub(super) view: View,
    pub(super) conns: Vec<Conn>,
    pub(super) creds: Vec<Cred>,
    pub(super) tunnels: Vec<tunnels::Entry>,
    pub(super) snippets: Vec<crate::snippets::Snippet>,
    pub(super) conn_state: ListState,
    pub(super) cred_state: ListState,
    pub(super) tunnel_state: ListState,
    pub(super) prompt: Option<Prompt>,
    pub(super) picker: Option<Picker>,
    pub(super) confirm: Option<Confirm>,
    pub(super) status: String,
    /// Whether the message on screen is a failure, which is all that decides
    /// its colour.
    pub(super) status_failed: bool,
    /// When `status` was set; it stops showing after `STATUS_TTL` so an old
    /// message never sits there looking like it is still current.
    pub(super) status_at: Option<Instant>,
    pub(super) show_help: bool,
    pub(super) should_quit: bool,
    /// What `/` is filtering the current list by. Empty means "show all".
    pub(super) query: String,
    /// True while the query is being typed, so keys go into it instead of
    /// triggering actions.
    pub(super) searching: bool,
    /// When you last opened each connection, and how often.
    pub(super) history: history::History,
    /// Last known state of each server's port, keyed by `Conn::key`.
    pub(super) reach: HashMap<String, Reach>,
    /// Which probe round we are showing; answers from an older one are dropped.
    pub(super) reach_gen: u64,
    pub(super) reach_tx: Sender<reach::Msg>,
    pub(super) reach_rx: Receiver<reach::Msg>,
    /// Whether each engine's client is on PATH, worked out once per refresh
    /// rather than per row per frame.
    pub(super) installed: [bool; 4],
    /// The choices that are yours rather than the clients'.
    /// A failure that has to be read. It owns every key until dismissed.
    pub(super) alert: Option<alert::Alert>,
    pub(super) settings: Settings,
    pub(super) snippet_state: ListState,
    pub(super) settings_state: ListState,
}

impl App {
    /// An app with no data loaded - the base for both `new()` and the tests
    /// (which set the lists directly, so they never touch a real config).
    pub(super) fn empty() -> Self {
        let (tx, rx) = channel();
        App {
            view: View::Connections,
            conns: Vec::new(),
            creds: Vec::new(),
            tunnels: Vec::new(),
            snippets: Vec::new(),
            conn_state: ListState::default().with_selected(Some(0)),
            cred_state: ListState::default().with_selected(Some(0)),
            tunnel_state: ListState::default().with_selected(Some(0)),
            prompt: None,
            picker: None,
            confirm: None,
            alert: None,
            status: String::new(),
            status_failed: false,
            status_at: None,
            show_help: false,
            should_quit: false,
            query: String::new(),
            searching: false,
            history: history::History::default(),
            reach: HashMap::new(),
            reach_gen: 0,
            reach_tx: tx,
            reach_rx: rx,
            installed: [true; 4],
            settings: Settings::default(),
            // Selected from the start: the detail panel beside a list that has
            // never been touched would otherwise read "nothing selected", which
            // looks like an empty tab rather than one you have not moved in.
            snippet_state: ListState::default().with_selected(Some(0)),
            settings_state: ListState::default().with_selected(Some(0)),
        }
    }

    /// Set the transient status line for something that worked. It fades on its
    /// own after `STATUS_TTL`.
    pub(super) fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_at = Some(Instant::now());
        self.status_failed = false;
    }

    /// Report a failure where it fits: the status line when the message is
    /// short, an alert box when it is long or spans several lines.
    ///
    /// A line is enough for `Address already in use`, because the list behind
    /// it still shows the state. A box is for what you have to read twice, and
    /// it carries the command too, which a line has no room for.
    pub(super) fn report_failure(&mut self, title: &str, said: &str, cmd: &str, err: &str) {
        let err = err.trim();
        // Our sentence, a colon, then the program's own words.
        let line = format!("{said}: {err}");
        // Too tall or too wide for the bar, so it goes in the box.
        if err.lines().count() > 1 || line.chars().count() > 90 {
            self.alert(title, format!("{cmd}\n\n{err}"));
        } else {
            self.set_failed(line);
        }
    }

    /// The same line for something that did not. Yellow, the same yellow an
    /// alert uses: red is reserved for a gate in front of something about to be
    /// lost, and this has already happened. A failure worth acting on is an
    /// alert box instead - this is for the ones with nothing to do about them.
    pub(super) fn set_failed(&mut self, msg: impl Into<String>) {
        self.set_status(msg);
        self.status_failed = true;
    }

    /// The status message while it is still fresh; `None` once it has expired.
    pub(super) fn live_status(&self) -> Option<&str> {
        let at = self.status_at?;
        (at.elapsed() < STATUS_TTL && !self.status.is_empty()).then_some(self.status.as_str())
    }

    pub(super) fn new() -> Self {
        let mut app = Self::empty();
        app.settings = settings::load();
        app.history = history::load();
        app.refresh_all();
        app.start_probes();
        app
    }

    // --- data loading -----------------------------------------------------

    pub(super) fn refresh_all(&mut self) {
        self.refresh_conns();
        self.refresh_creds();
        self.refresh_tunnels();
        self.refresh_snippets();
    }

    pub(super) fn refresh_conns(&mut self) {
        self.conns = engines::list();
        self.sort_conns();
        for e in engines::ENGINES {
            self.installed[e.idx()] = engines::client_installed(e, &self.settings);
        }
        let n = self.conn_rows().len();
        Self::clamp(&mut self.conn_state, n);
    }

    /// Most recently opened first, then everything you have never opened,
    /// alphabetically. The database you were in ten minutes ago is the one you
    /// are most likely reaching for, and it should not be a scroll away - unless
    /// you asked for plain alphabetical in Settings.
    pub(super) fn sort_conns(&mut self) {
        match self.settings.conn_order {
            ConnOrder::Recent => {
                let history = &self.history;
                self.conns.sort_by_key(|c| history.rank(&c.key()));
            }
            // Engine first, then name inside it. `Engine::idx()` is the order
            // `ENGINES` declares, not the slug: `lite/my/pg` is an abbreviation
            // nobody asked to read.
            ConnOrder::Engine => self
                .conns
                .sort_by(|a, b| (a.engine.idx(), &a.name).cmp(&(b.engine.idx(), &b.name))),
            // And by name alone, for somebody who thinks in connection names
            // rather than in engines. Ties fall back to the engine so the order
            // is still total.
            ConnOrder::Name => self
                .conns
                .sort_by(|a, b| (&a.name, a.engine.idx()).cmp(&(&b.name, b.engine.idx()))),
        }
    }

    pub(super) fn refresh_creds(&mut self) {
        self.creds = creds::list();
        let n = self.cred_rows().len();
        Self::clamp(&mut self.cred_state, n);
    }

    /// The local port of a live forward whose far end is exactly this
    /// connection's host and port, if one is open. This is what stops easysql
    /// offering to dig a tunnel it has already dug.
    pub(super) fn tunnel_carrying(&self, c: &Conn) -> Option<String> {
        let port = c.port_or_default();
        self.tunnels.iter().find_map(|t| {
            // Only a forward that is actually running carries anything: a
            // sleeping row is a row, not a tunnel, and offering to reuse it
            // would point the connection at a port with nothing behind it.
            if !t.on() {
                return None;
            }
            let (open, target, onward) = t.ports()?;
            let same_host = target.eq_ignore_ascii_case(&c.host)
                || crate::sshhosts::is_same_machine(&t.host, &c.host)
                    && matches!(target, "127.0.0.1" | "localhost");
            (t.kind == 'L' && same_host && onward == port).then(|| open.to_string())
        })
    }

    pub(super) fn refresh_snippets(&mut self) {
        self.snippets = crate::snippets::list();
        // Editing a `.sql` with `o` would otherwise leave `:name` expanding to the old
        // query. Writes nothing when nothing changed.
        let _ = crate::snippets::sync_psqlrc();
        let n = self.snippet_rows().len();
        Self::clamp(&mut self.snippet_state, n);
    }

    pub(super) fn selected_snippet(&self) -> Option<&crate::snippets::Snippet> {
        let row = *self.snippet_rows().get(self.snippet_state.selected()?)?;
        self.snippets.get(row)
    }

    pub(super) fn refresh_tunnels(&mut self) {
        self.tunnels = crate::vias::rows();
        let n = self.tunnel_rows().len();
        Self::clamp(&mut self.tunnel_state, n);
    }

    /// Is there a saved password that answers for this connection? Only asked
    /// of the engines whose client reads a password file of its own: SQLite has
    /// nothing to unlock, and sqlcmd prompts for itself.
    pub(super) fn has_password(&self, c: &Conn) -> bool {
        c.engine.stores_password() && self.creds.iter().any(|cred| cred.covers(c))
    }

    // --- reachability -----------------------------------------------------

    /// Start a fresh round of port probes. Older rounds are invalidated by the
    /// generation bump, so a slow answer from the last round cannot repaint a
    /// server we have since re-probed.
    pub(super) fn start_probes(&mut self) {
        self.reach_gen += 1;
        // Turned off in Settings: forget what we knew rather than leave dots
        // that stop being true the moment anything moves.
        if !self.settings.probe {
            self.reach.clear();
            return;
        }
        let targets: Vec<reach::Target> = self
            .conns
            .iter()
            // A SQLite file has no port and nothing to ask.
            .filter(|c| c.engine.networked())
            .map(|c| reach::Target {
                key: c.key(),
                host: if c.host.is_empty() {
                    "localhost".to_string()
                } else {
                    c.host.clone()
                },
                port: c.port_or_default().parse().unwrap_or(0),
            })
            .filter(|t| t.port != 0)
            .collect();
        for t in &targets {
            self.reach.insert(t.key.clone(), Reach::Probing);
        }
        reach::probe_all(
            targets,
            self.reach_gen,
            self.reach_tx.clone(),
            self.settings.probe_timeout,
        );
    }

    /// Take whatever the probe threads have reported since the last frame.
    pub(super) fn drain_probes(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.reach_rx.try_recv() {
            if msg.generation != self.reach_gen {
                continue; // an answer to a question we no longer ask
            }
            self.reach.insert(msg.key, msg.reach);
            changed = true;
        }
        changed
    }

    /// True while any probe is still outstanding, so the event loop knows to
    /// keep waking up instead of blocking on a keypress.
    pub(super) fn probing(&self) -> bool {
        self.reach.values().any(|r| *r == Reach::Probing)
    }

    // --- filtering --------------------------------------------------------

    /// Indices into `conns` that survive the current query.
    pub(super) fn conn_rows(&self) -> Vec<usize> {
        self.conns
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                let target = c.target();
                filter::matches(&self.query, &[&c.name, &target, c.engine.label()])
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn cred_rows(&self) -> Vec<usize> {
        self.creds
            .iter()
            .enumerate()
            .filter(|(_, c)| filter::matches(&self.query, &[&c.describe()]))
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn tunnel_rows(&self) -> Vec<usize> {
        self.tunnels
            .iter()
            .enumerate()
            .filter(|(_, t)| filter::matches(&self.query, &[&t.describe()]))
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn snippet_rows(&self) -> Vec<usize> {
        self.snippets
            .iter()
            .enumerate()
            .filter(|(_, s)| filter::matches(&self.query, &[&s.name, &s.summary()]))
            .map(|(i, _)| i)
            .collect()
    }

    /// The settings the current query keeps. Every one is always relevant, so
    /// this filters the same way the other tabs do rather than specially.
    pub(super) fn settings_rows(&self) -> Vec<settings::Row> {
        self.settings
            .rows()
            .into_iter()
            .filter(|r| filter::matches(&self.query, &[r.label, r.key, &r.value, r.help]))
            .collect()
    }

    pub(super) fn selected_setting(&self) -> Option<settings::Row> {
        let i = self.settings_state.selected()?;
        self.settings_rows().into_iter().nth(i)
    }

    /// How many rows the current view is showing, after filtering.
    pub(super) fn row_count(&self) -> usize {
        match self.view {
            View::Connections => self.conn_rows().len(),
            View::Passwords => self.cred_rows().len(),
            View::Tunnels => self.tunnel_rows().len(),
            View::Snippets => self.snippet_rows().len(),
            View::Settings => self.settings_rows().len(),
        }
    }

    /// Jump to another view the way a finished action does. The filter belongs
    /// to the list it was typed over, so it does not come along - and a tunnel
    /// made from a filtered Connections list would otherwise land on a Tunnels
    /// tab that hides it.
    pub(super) fn goto_view(&mut self, view: View) {
        self.view = view;
        if !self.query.is_empty() || self.searching {
            self.query.clear();
            self.searching = false;
            self.requery();
        }
    }

    /// Put the cursor on the tunnel with this pid: the one you just made is the
    /// one you are looking for, and on a busy list it is not row one.
    pub(super) fn select_tunnel(&mut self, spec: &str, host: &str) {
        let at = self.tunnel_rows().iter().position(|&r| {
            let t = &self.tunnels[r];
            t.spec == spec && t.host == host
        });
        if let Some(i) = at {
            self.tunnel_state.select(Some(i));
        }
    }

    /// The same for a connection, after adding or renaming one.
    pub(super) fn select_conn(&mut self, key: &str) {
        let at = self
            .conn_rows()
            .iter()
            .position(|&r| self.conns[r].key() == key);
        if let Some(i) = at {
            self.conn_state.select(Some(i));
        }
    }

    /// Re-clamp every list after the query changed, and put the selection back
    /// at the top so the first match is the one under the cursor.
    pub(super) fn requery(&mut self) {
        self.conn_state.select(Some(0));
        self.cred_state.select(Some(0));
        self.tunnel_state.select(Some(0));
        self.snippet_state.select(Some(0));
        self.settings_state.select(Some(0));
        let n = self.conn_rows().len();
        Self::clamp(&mut self.conn_state, n);
        let n = self.cred_rows().len();
        Self::clamp(&mut self.cred_state, n);
        let n = self.tunnel_rows().len();
        Self::clamp(&mut self.tunnel_state, n);
        let n = self.snippet_rows().len();
        Self::clamp(&mut self.snippet_state, n);
        let n = self.settings_rows().len();
        Self::clamp(&mut self.settings_state, n);
    }

    /// Keep a list's selection in range (and set/clear it as the list fills/empties).
    pub(super) fn clamp(state: &mut ListState, len: usize) {
        if len == 0 {
            state.select(None);
        } else {
            let sel = state.selected().unwrap_or(0).min(len - 1);
            state.select(Some(sel));
        }
    }

    // A selection indexes the *filtered* rows, so every lookup goes through the
    // row list; indexing the raw vec would pick the wrong entry the moment `/`
    // hides anything.
    pub(super) fn selected_conn(&self) -> Option<&Conn> {
        let row = *self.conn_rows().get(self.conn_state.selected()?)?;
        self.conns.get(row)
    }

    pub(super) fn selected_cred(&self) -> Option<&Cred> {
        let row = *self.cred_rows().get(self.cred_state.selected()?)?;
        self.creds.get(row)
    }

    pub(super) fn selected_tunnel(&self) -> Option<&tunnels::Entry> {
        let row = *self.tunnel_rows().get(self.tunnel_state.selected()?)?;
        self.tunnels.get(row)
    }

    // --- input ------------------------------------------------------------

    pub(super) fn cycle_view(&mut self, delta: i32) {
        let i = VIEWS.iter().position(|v| *v == self.view).unwrap_or(0) as i32;
        let n = (i + delta).rem_euclid(VIEWS.len() as i32) as usize;
        self.view = VIEWS[n];
        // A `/` filter belongs to the list you typed it over; carrying it into
        // the next tab would silently hide rows you never searched.
        if !self.query.is_empty() || self.searching {
            self.query.clear();
            self.searching = false;
            self.requery();
        }
    }

    pub(super) fn move_sel(&mut self, delta: i32) {
        let len = self.row_count();
        match self.view {
            View::Connections => Self::move_state(&mut self.conn_state, len, delta),
            View::Passwords => Self::move_state(&mut self.cred_state, len, delta),
            View::Tunnels => Self::move_state(&mut self.tunnel_state, len, delta),
            View::Snippets => Self::move_state(&mut self.snippet_state, len, delta),
            View::Settings => Self::move_state(&mut self.settings_state, len, delta),
        }
    }

    pub(super) fn move_state(state: &mut ListState, len: usize, delta: i32) {
        if len == 0 {
            return;
        }
        let cur = state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(len as i32) as usize;
        state.select(Some(next));
    }
}

// ==========================================================================
// Terminal lifecycle + event loop
// ==========================================================================

/// Entry point for bare `esql`.
pub fn run() -> Result<()> {
    install_panic_hook();
    let mut terminal = setup()?;
    let mut app = App::new();
    let res = event_loop(&mut terminal, &mut app);
    teardown(&mut terminal)?;
    res
}

fn event_loop(terminal: &mut Term, app: &mut App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|f| ui(f, app))?;

        // While a status message is showing, or port probes are still landing,
        // we wake up periodically so the screen can catch up on its own;
        // otherwise block until the user actually presses a key, so an idle TUI
        // costs nothing.
        let timeout = if app.live_status().is_some() || app.probing() {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(3600)
        };
        if !event::poll(timeout)? {
            app.drain_probes();
            continue; // nothing pressed: redraw so the stale status drops off
        }
        app.drain_probes();

        let Event::Key(key) = event::read()? else {
            continue;
        };
        // crossterm emits Press+Release on some platforms; act on Press only.
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if let Some(run) = app.on_key(key) {
            // The same orientation line the CLI prints. It belongs here too, and
            // more so: Enter in the list is the usual way in, and the whole gap
            // it closes is "I am at a prompt and cannot remember this client's
            // word for `list the tables`".
            let hint = run
                .connect
                .as_ref()
                .filter(|_| app.settings.hints)
                .map(|c| crate::engines::hint_line(c.engine, &app.settings));
            let status = run_suspended(terminal, &run.argv, hint)?;

            if let Some(conn) = run.connect.clone() {
                match status {
                    // A session you actually got into is what makes a connection
                    // "recent"; a failed one leaves the ordering alone.
                    Some(s) if s.success() => {
                        history::record(&conn.key());
                        app.history = history::load();
                    }
                    // The client printed why and then we redrew over it. Ask the
                    // server the same question non-interactively and offer the
                    // fix in a modal instead of a status that flashes past.
                    Some(_) => app.offer_fix(&conn),
                    None => {}
                }
            }

            match status {
                Some(s) if s.success() => app.set_status(format!("{} ✓", run.label)),
                // The client ran and failed. `offer_fix` above already put up an
                // offer when the server's answer could be classified; when it
                // could not, the exit code is all we have and it belongs in the
                // status line, not in a box with nothing to say.
                Some(s) => app.set_failed(format!(
                    "{} failed (exit {})",
                    run.label,
                    s.code().unwrap_or(-1)
                )),
                // Nothing ran at all: the program is missing or not executable,
                // and this is the one line explaining why the screen came back
                // unchanged. A status line would clear it after STATUS_TTL.
                None => {
                    let program = run.argv[0].clone();
                    // Name the one command this machine needs, never a list of
                    // distros' guesses, and never an errno in place of advice.
                    let advice = run
                        .connect
                        .as_ref()
                        .map(|c| c.engine)
                        .and_then(|e| match engines::install_argv(e) {
                            Some(argv) => Some(format!("Install it with:\n\n    {}", argv.join(" "))),
                            None => engines::install_note(e).map(str::to_string),
                        })
                        .unwrap_or_else(|| {
                            "Check that it is installed and on your PATH, or point the setting for this engine at the program you do have.".to_string()
                        });
                    app.alert(
                        "could not run it",
                        format!(
                            "`{program}` could not be started, so nothing was opened.\n\n{advice}"
                        ),
                    )
                }
            }
            app.refresh_all();
            // A dot learned before the session is a claim about a world we
            // stopped watching: the tunnel it went through may have died while
            // the client owned the terminal, and a `\c` inside psql can leave
            // the connection pointing somewhere else entirely.
            app.start_probes();
        }
    }
    Ok(())
}

/// Leave the TUI, run an interactive command with the real terminal, then
/// restore the TUI. This is what lets psql own its prompt, its pager and its
/// readline instead of us pretending to be a database client.
fn run_suspended(
    terminal: &mut Term,
    argv: &[String],
    hint: Option<String>,
) -> Result<Option<ExitStatus>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    // ratatui hides the cursor while drawing and leaving the alt-screen does not
    // restore it, so without this the client runs with an invisible cursor and
    // you cannot see where you are typing.
    terminal.show_cursor()?;

    // After the alt-screen is gone, so it lands on the shell's screen above the
    // client's own banner rather than being wiped with the TUI.
    if let Some(hint) = hint {
        // Dim, so it reads as a footnote under the client's own banner.
        println!("\x1b[2m  {hint}\x1b[0m");
    }

    // Ctrl-C reaches every process in the foreground group, so without this it
    // kills easysql mid-suspend and the shell comes back in raw mode. Do what
    // `system(3)` does and ignore it while the child runs; SIG_IGN survives exec,
    // so the child restores SIG_DFL itself.
    let status;
    unsafe {
        let prev_int = libc::signal(libc::SIGINT, libc::SIG_IGN);
        let prev_quit = libc::signal(libc::SIGQUIT, libc::SIG_IGN);

        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
        status = cmd.status().ok();

        libc::signal(libc::SIGINT, prev_int);
        libc::signal(libc::SIGQUIT, prev_quit);
    }

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.hide_cursor()?;
    terminal.clear()?;
    Ok(status)
}

fn setup() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn teardown(terminal: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Make sure a panic doesn't leave the user's terminal in raw/alt-screen mode.
fn install_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_goes_in_the_container_its_length_earns() {
        // A short one fits the bar, where the list behind it still shows the
        // state; a long one has to be readable twice, which only a box allows.
        let mut app = App::empty();
        app.report_failure(
            "tunnel failed",
            "could not reopen it",
            "ssh -N",
            "port taken",
        );
        assert!(app.alert.is_none(), "a one-liner needs no box");
        assert!(app.status_failed, "and it is yellow, not green");
        assert!(app.live_status().is_some_and(|s| s.contains("port taken")));

        app.report_failure(
            "tunnel failed",
            "could not reopen it",
            "ssh -N",
            "line one\nline two",
        );
        assert!(app.alert.is_some(), "more than one line has to be read");
    }
}
