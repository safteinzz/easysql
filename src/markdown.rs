//! `esql --md`: a client's machine-readable rows turned into markdown tables.
//!
//! The client still runs the query and prints the rows; this only re-draws what
//! it printed, so there is no dialect here and nothing that speaks to a server.

/// The shape a client prints its rows in when `Engine::table_output` asks.
#[derive(Clone, Copy)]
pub enum Rows {
    /// RFC 4180, as psql's and sqlite3's CSV modes write it.
    Csv,
    /// mysql's `--batch`: tabs between fields, with `\n`, `\t` and `\\`
    /// escaped inside a value.
    Tsv,
}

/// Every result in `out` as a markdown table, a blank line between them.
///
/// A client prints consecutive results back to back with nothing between them,
/// so a change in the number of fields is read as the next result's header;
/// two results of the same width print as one table.
pub fn render(out: &str, rows: Rows) -> String {
    let records = match rows {
        Rows::Csv => csv(out),
        Rows::Tsv => tsv(out),
    };
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    for r in records {
        match tables.last_mut() {
            Some(t) if t[0].len() == r.len() => t.push(r),
            _ => tables.push(vec![r]),
        }
    }
    tables
        .iter()
        .map(|t| table(t))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One table, padded so the raw text reads as well in a terminal as rendered.
fn table(records: &[Vec<String>]) -> String {
    let cells: Vec<Vec<String>> = records
        .iter()
        .map(|r| r.iter().map(|v| cell(v)).collect())
        .collect();
    let mut widths = vec![3; cells[0].len()];
    for r in &cells {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |r: &[String]| {
        let padded: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c}{}", " ".repeat(w - c.chars().count())))
            .collect();
        format!("| {} |\n", padded.join(" | "))
    };
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    let mut s = line(&cells[0]);
    s.push_str(&line(&rule));
    for r in &cells[1..] {
        s.push_str(&line(r));
    }
    s
}

/// A value as one markdown cell: a row cannot hold a newline, so each line is
/// joined with `<br>`, trimmed because a renderer collapses the spaces anyway.
fn cell(v: &str) -> String {
    v.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("<br>")
        .replace('|', "\\|")
}

fn csv(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => record.push(std::mem::take(&mut field)),
            // sqlite3 ends a record with `\r\n`.
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

fn tsv(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(|l| l.split('\t').map(unescape).collect())
        .collect()
}

fn unescape(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            s.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => s.push('\n'),
            Some('t') => s.push('\t'),
            Some('0') => {}
            Some(other) => s.push(other),
            None => s.push('\\'),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cells on one drawn row, counting only the `|` that are not escaped.
    fn cells(row: &str) -> usize {
        let mut prev = ' ';
        let mut n = 0;
        for c in row.chars() {
            if c == '|' && prev != '\\' {
                n += 1;
            }
            prev = c;
        }
        n - 1
    }

    #[test]
    fn a_quoted_comma_or_newline_stays_in_one_cell() {
        let csv = "a,b\r\n\"x,y\",\"one\n  two | three\"\r\n";
        let tsv = "a\tb\nx,y\tone\\n  two | three\n";
        for (out, rows) in [(csv, Rows::Csv), (tsv, Rows::Tsv)] {
            let md = render(out, rows);
            let lines: Vec<&str> = md.lines().collect();
            assert_eq!(lines.len(), 3, "a header, a rule and one row, got:\n{md}");
            assert_eq!(
                cells(lines[2]),
                2,
                "the row should still be two cells, got:\n{md}"
            );
        }
    }

    #[test]
    fn a_change_in_field_count_starts_a_new_table() {
        let md = render("a,b\n1,2\nc\n3\n", Rows::Csv);
        let rules = md.lines().filter(|l| l.starts_with("| -")).count();
        assert_eq!(rules, 2, "two results should draw two tables, got:\n{md}");
    }
}
