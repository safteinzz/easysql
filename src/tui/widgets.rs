//! Pane furniture with no idea what it is drawing: titled blocks, centering,
//! the wrapped-row count a box needs to size itself, the house box every
//! overlay is built from, and rendering an argv the way a shell would read it.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

/// Rough count of how many rows `text` needs when word-wrapped to `width`,
/// matching how ratatui's `Wrap` breaks on spaces. Used to size modal boxes.
pub(super) fn wrapped_line_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let mut rows = 1usize;
    let mut col = 0usize;
    // Split on single spaces rather than runs of whitespace: the indent and the
    // gap after "runs" are real columns, and collapsing them undercounted the
    // rows enough to clip the line off the bottom of a wizard.
    for (i, word) in text.split(' ').enumerate() {
        let w = word.chars().count();
        // Only the very first token has no space in front of it. Keying this off
        // `col == 0` instead swallowed a column for every leading space, which
        // undercounted the rows and clipped the last line off a box.
        let need = if i == 0 { w } else { col + 1 + w };
        if need <= width {
            col = need;
        } else {
            rows += 1;
            col = w;
        }
        // A word longer than the whole line wraps across several rows.
        while col > width {
            rows += 1;
            col -= width;
        }
    }
    rows
}

/// A bordered block titled `Name (count)`.
pub(super) fn titled(name: &str, count: usize) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {name} ({count}) "))
}

/// Render a friendly empty-state message inside the body block.
pub(super) fn empty(f: &mut Frame, area: Rect, name: &str, msg: &str) {
    let para = Paragraph::new(msg)
        .block(titled(name, 0))
        .style(Style::default().add_modifier(Modifier::DIM))
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

/// An argv rendered the way you would type it into a shell, for a human to read.
/// Only an argument that actually needs quoting gets it, so `psql service=prod`
/// stays readable and a path with a space still pastes correctly.
///
/// `connect_argv` expands `~` because a child runs without a shell and would
/// otherwise be handed a directory literally called `~`, but an absolute home
/// path is noise on screen and it is what puts `/home/somebody/...` in a
/// screenshot. Display collapses it back; nothing that runs ever sees this
/// string.
pub(super) fn shell_join_display(argv: &[String]) -> String {
    let shown: Vec<String> = argv
        .iter()
        .map(|a| {
            let c = crate::ini::collapse_tilde(a);
            // A collapsed `~/x` must not come back quoted: `'~/x'` is a literal
            // directory called `~` to every shell, so the line on screen would
            // no longer be the line you could paste. Quote the rest of it the
            // normal way by handing the tail through unchanged.
            match c.strip_prefix("~/") {
                Some(rest) if !needs_quoting(rest) => c,
                _ => shell_join(std::slice::from_ref(&c)),
            }
        })
        .collect();
    shown.join(" ")
}

fn needs_quoting(s: &str) -> bool {
    s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || "\"'\\$`;&|<>()*?[]{}!#~".contains(c))
}

/// A connection's whole command as a shell would take it: its environment
/// first, then the already-joined argv. Without the environment a copied line
/// for a read-only Postgres connection opens a writable session in any client
/// that drops the service's own `options`.
pub(super) fn with_env(env: &[(String, String)], argv: String) -> String {
    let mut parts: Vec<String> = env
        .iter()
        .map(|(k, v)| format!("{k}={}", shell_join(std::slice::from_ref(v))))
        .collect();
    parts.push(argv);
    parts.join(" ")
}

pub(super) fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if needs_quoting(a) {
                format!("'{}'", a.replace('\'', r"'\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── the house box ───────────────────────────────────────────────────────────
// Every overlay is built from these, so only its colour and its buttons carry
// meaning: gate red, alert yellow, offer/picker/form/reader cyan.

/// Narrowest a box may be, so a two-word message still reads as a box.
pub(super) const BOX_MIN_W: u16 = 24;
/// Widest, so one long line does not stretch a box across a 200-column screen.
pub(super) const BOX_MAX_W: u16 = 88;
/// Rows the chrome costs: two borders plus the single top padding row.
pub(super) const BOX_CHROME_H: u16 = 3;
/// Columns the chrome costs: two borders plus two columns of padding a side.
pub(super) const BOX_CHROME_W: u16 = 6;

/// How wide a box is on a screen this wide.
pub(super) fn box_width(screen_w: u16) -> u16 {
    screen_w.saturating_sub(4).clamp(BOX_MIN_W, BOX_MAX_W)
}

/// The columns a body actually gets, which is what it must be wrapped to.
pub(super) fn box_inner_width(width: u16) -> usize {
    width.saturating_sub(BOX_CHROME_W).max(1) as usize
}

/// How tall a box holding `body_rows` *wrapped* rows is, capped at the screen.
/// Pass the wrapped count, never the line count: measuring the unwrapped text
/// is what clips a modal's last row off and makes it look unanswerable, which
/// is a bug this crate has already shipped once.
///
/// The cap is the whole screen and not some fraction of it. A box is drawn over
/// a `Clear`, so it owns the screen while it is up anyway, and a fraction only
/// decides in advance that a long one gets cut off.
pub(super) fn box_height(body_rows: u16, screen_h: u16) -> u16 {
    let floor = BOX_CHROME_H + 1;
    body_rows
        .saturating_add(BOX_CHROME_H)
        .clamp(floor, screen_h.max(floor))
}

/// A box of at most `w` by `h`, centered in `area`.
pub(super) fn box_area(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

/// The bordered block every box wears: a spaced title on the top border, and
/// otherwise an unbroken frame in the colour that says what kind it is.
///
/// Nothing else is written on the border. Keys go in the body, through
/// `box_hint`: a frame with a sentence along the bottom of it stops reading as
/// a frame, and the title then has to compete with it.
pub(super) fn box_block(colour: Color, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colour))
        // One blank row above the content and none below it: the key line is
        // the last thing in the box, and a blank under it is a wasted row that
        // makes the frame look loose.
        .padding(Padding::new(2, 2, 1, 0))
        .title(format!(" {} ", title.trim()))
}

/// The line of keys a box ends with, as the last row of its body. Every kind
/// puts it in the same place, so it is where the eye already is.
///
/// Quieter than anything else in the box, deliberately: an unfocused field
/// label is the default foreground dimmed, so this goes a step below that with
/// `DarkGray` dimmed again. Separation is the blank row above it and its fixed
/// place at the bottom, not brightness. Colouring it only made a guideline look
/// like something worth reading.
pub(super) fn box_hint(keys: &str) -> Line<'static> {
    Line::from(Span::styled(
        keys.to_string(),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ))
}

/// The Yes/No row a gate and an offer share. The labels carry their keys, so
/// the hint line does not have to teach them twice, and the picked one is
/// filled with the border colour rather than merely reversed: a reversed
/// button reads as "selected", a filled one reads as "this is what Enter does".
/// One answer of a two-option form toggle: the house button, filled when picked.
pub(super) fn chip(label: &str, picked: bool) -> Span<'static> {
    let style = if picked {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    Span::styled(format!(" {label} "), style)
}

pub(super) fn box_buttons(colour: Color, yes: bool) -> Line<'static> {
    let button = |label: &str, picked: bool| {
        let style = if picked {
            Style::default()
                .fg(Color::Black)
                .bg(colour)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        Span::styled(format!(" {label} "), style)
    };
    Line::from(vec![
        button("Yes (y)", yes),
        Span::raw("  "),
        button("No (n)", !yes),
    ])
}
