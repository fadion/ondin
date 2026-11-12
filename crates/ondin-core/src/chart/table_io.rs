//! A chart's table read from text (`charts.md`, *Import and the linked file*):
//! CSV (RFC 4180, its delimiter detected among comma, semicolon and tab), TSV,
//! and JSON as an array of objects. Each reader holds the table's limits and
//! says what it left out.

use super::{Column, ColumnId, MAX_CELL_CHARS, MAX_COLUMNS, MAX_ROWS, Table, parse_number};

/// A table as read, with a note for each thing left out of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub table: Table,
    /// Plain sentences: *12 rows past 10,000 left out*, *“address” held an
    /// object, left empty*.
    pub notes: Vec<String>,
    /// Whether the first row became the header (CSV and TSV only; JSON's keys
    /// always are).
    pub header: bool,
}

/// Why a file is not a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    #[error("The file is empty")]
    Empty,
    #[error("Not an array of objects")]
    NotObjects,
    #[error("The file is not valid JSON")]
    BadJson,
}

/// The file kinds a table is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
}

impl Format {
    /// The kind a file name's extension names, case folded.
    pub fn of_name(name: &str) -> Option<Format> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "csv" => Some(Format::Csv),
            "tsv" | "tab" => Some(Format::Tsv),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

/// `text` read as `format`. `header` forces whether a CSV's or TSV's first row
/// is the header; `None` decides: it is, unless every cell in it parses as a
/// number.
pub fn read(text: &str, format: Format, header: Option<bool>) -> Result<Read, Unreadable> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format {
        Format::Json => read_json(text),
        Format::Tsv => read_delimited(text, b'\t', header),
        Format::Csv => read_delimited(text, delimiter(text), header),
    }
}

/// The delimiter a CSV's first line uses most, outside quotes: comma, semicolon
/// or tab, comma on a tie.
fn delimiter(text: &str) -> u8 {
    let line = first_record(text);
    let count = |d: char| {
        let mut n = 0;
        unquoted(
            line,
            |c| c == d,
            |c| {
                n += usize::from(c == d);
                false
            },
        );
        n
    };
    let (comma, semi, tab) = (count(','), count(';'), count('\t'));
    if semi > comma && semi >= tab {
        b';'
    } else if tab > comma && tab > semi {
        b'\t'
    } else {
        b','
    }
}

/// The text up to the first line break outside quotes.
fn first_record(text: &str) -> &str {
    let at = unquoted(
        text,
        |c| matches!(c, ',' | ';' | '\t'),
        |c| matches!(c, '\n' | '\r'),
    );
    &text[..at.unwrap_or(text.len())]
}

/// Each character of `text` outside quotes, with its byte offset, to `each`
/// until it answers `true`; that offset, `None` when it never does. A quote
/// opens only at the start of a line or after a character `sep` matches, and
/// `""` inside one is a quote, as [`records`] reads them, so an inch mark
/// mid-field quotes nothing.
fn unquoted(
    text: &str,
    sep: impl Fn(char) -> bool,
    mut each: impl FnMut(char) -> bool,
) -> Option<usize> {
    let mut quoted = false;
    let mut at_start = true;
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek().is_some_and(|(_, c)| *c == '"') {
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            continue;
        }
        if ch == '"' && at_start {
            quoted = true;
            at_start = false;
            continue;
        }
        if each(ch) {
            return Some(i);
        }
        at_start = sep(ch) || matches!(ch, '\n' | '\r');
    }
    None
}

/// RFC 4180 records: quoted fields may hold the delimiter, line breaks and
/// doubled quotes; a line break is `\n`, `\r\n` or `\r`.
fn records(text: &str, d: u8) -> Vec<Vec<String>> {
    let d = d as char;
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(ch) = chars.next() {
        any = true;
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                c => field.push(c),
            }
            continue;
        }
        match ch {
            '"' if field.is_empty() => quoted = true,
            c if c == d => row.push(std::mem::take(&mut field)),
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                out.push(std::mem::take(&mut row));
                any = false;
            }
            c => field.push(c),
        }
    }
    if any {
        row.push(field);
        out.push(row);
    }
    // Blank lines are no rows.
    out.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    out
}

fn read_delimited(text: &str, d: u8, header: Option<bool>) -> Result<Read, Unreadable> {
    let mut rows = records(text, d);
    if rows.is_empty() {
        return Err(Unreadable::Empty);
    }
    let header = header.unwrap_or_else(|| {
        !rows[0]
            .iter()
            .all(|c| !c.trim().is_empty() && parse_number(c).is_some())
    });
    // Named only as far as a table holds, so one long line names no more.
    let widest = rows.iter().map(Vec::len).max().unwrap_or(0);
    let width = widest.min(MAX_COLUMNS);
    let names: Vec<String> = if header {
        let head = rows.remove(0);
        (0..width)
            .map(|i| {
                head.get(i)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("Column {}", i + 1))
            })
            .collect()
    } else {
        (0..width).map(|i| format!("Column {}", i + 1)).collect()
    };
    Ok(build(names, rows, header, widest - width, 0))
}

/// One JSON object with its keys in the file's order, which `serde_json`'s map
/// (sorted, without its `preserve_order` feature) does not keep.
struct Ordered(Vec<(String, serde_json::Value)>);

impl<'de> serde::Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut m: A,
            ) -> Result<Ordered, A::Error> {
                // A repeated key keeps its last value, at its last place.
                let mut out: Vec<Option<(String, serde_json::Value)>> = Vec::new();
                let mut at: rustc_hash::FxHashMap<String, usize> = Default::default();
                while let Some((k, v)) = m.next_entry::<String, serde_json::Value>()? {
                    if let Some(i) = at.insert(k.clone(), out.len()) {
                        out[i] = None;
                    }
                    out.push(Some((k, v)));
                }
                Ok(Ordered(out.into_iter().flatten().collect()))
            }
        }
        d.deserialize_map(Visit)
    }
}

fn read_json(text: &str) -> Result<Read, Unreadable> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| Unreadable::BadJson)?;
    let serde_json::Value::Array(items) = value else {
        return Err(Unreadable::NotObjects);
    };
    if items.is_empty() {
        return Err(Unreadable::Empty);
    }
    if items.iter().any(|i| !i.is_object()) {
        return Err(Unreadable::NotObjects);
    }
    // Read again in order: the first pass only says it is an array of objects.
    let items: Vec<Ordered> = serde_json::from_str(text).map_err(|_| Unreadable::BadJson)?;
    let (names, past) = json_columns(&items);
    let column: rustc_hash::FxHashMap<&str, usize> =
        (0..).zip(&names).map(|(i, k)| (k.as_str(), i)).collect();
    let mut nested: Vec<usize> = Vec::new();
    let mut unwritten: Vec<usize> = Vec::new();
    let note = |list: &mut Vec<usize>, i: usize| {
        if !list.contains(&i) {
            list.push(i);
        }
    };
    let rows: Vec<Vec<String>> = items
        .iter()
        .take(MAX_ROWS)
        .map(|item| {
            let mut row = vec![String::new(); names.len()];
            for (k, v) in &item.0 {
                let Some(&i) = column.get(k.as_str()) else {
                    continue;
                };
                row[i] = match v {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => match n.as_f64() {
                        // Written without an exponent, which `parse_number`
                        // refuses: `serde_json` writes 1e21 as `1e21`, `f64`'s
                        // `Display` never. One too long for a cell that way
                        // keeps the file's spelling, as text, and is said.
                        Some(f) if n.is_f64() => {
                            let s = f.to_string();
                            if s.chars().count() > MAX_CELL_CHARS {
                                note(&mut unwritten, i);
                                n.to_string()
                            } else {
                                s
                            }
                        }
                        _ => n.to_string(),
                    },
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                        note(&mut nested, i);
                        String::new()
                    }
                };
            }
            row
        })
        .collect();
    let more_rows = items.len().saturating_sub(MAX_ROWS);
    let mut read = build(names, rows, true, past, more_rows);
    for i in nested {
        let k = &read.table.columns[i].name;
        read.notes
            .push(format!("“{k}” held a list or an object, left empty"));
    }
    for i in unwritten {
        let k = &read.table.columns[i].name;
        read.notes.push(format!(
            "“{k}” held a number too large or too small to write out, kept as text"
        ));
    }
    Ok(read)
}

/// The columns of a JSON table: the first [`MAX_COLUMNS`] keys met, in that
/// order, and how many other distinct keys there were. The rest are only
/// counted, so a file of many keys never builds a cell for each.
fn json_columns(items: &[Ordered]) -> (Vec<String>, usize) {
    let mut names: Vec<String> = Vec::new();
    let mut seen: rustc_hash::FxHashSet<&str> = Default::default();
    for item in items {
        for (k, _) in &item.0 {
            if seen.insert(k) && names.len() < MAX_COLUMNS {
                names.push(k.clone());
            }
        }
    }
    let past = seen.len() - names.len();
    (names, past)
}

/// A count with thousands separators, as the chart's sentences write one
/// (*Too many marks: 3,400 bars*).
pub fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The table from named columns and rows, held to the limits, each cut noted.
/// `more_columns` and `more_rows` count what a reader left out before building,
/// beside whatever `names` and `rows` hold past the limits.
fn build(
    mut names: Vec<String>,
    mut rows: Vec<Vec<String>>,
    header: bool,
    more_columns: usize,
    more_rows: usize,
) -> Read {
    let mut notes = Vec::new();
    let columns_past = names.len().saturating_sub(MAX_COLUMNS) + more_columns;
    if columns_past > 0 {
        notes.push(format!(
            "{columns_past} columns past {MAX_COLUMNS} left out"
        ));
        names.truncate(MAX_COLUMNS);
    }
    let rows_past = rows.len().saturating_sub(MAX_ROWS) + more_rows;
    if rows_past > 0 {
        notes.push(format!(
            "{rows_past} rows past {} left out",
            grouped(MAX_ROWS)
        ));
        rows.truncate(MAX_ROWS);
    }
    let mut cut = 0;
    for r in &mut rows {
        r.resize(names.len(), String::new());
        for c in r.iter_mut() {
            if c.chars().count() > MAX_CELL_CHARS {
                *c = c.chars().take(MAX_CELL_CHARS).collect();
                cut += 1;
            }
        }
    }
    if cut > 0 {
        notes.push(format!(
            "{cut} {} cut to {MAX_CELL_CHARS} characters",
            if cut == 1 { "cell" } else { "cells" }
        ));
    }
    let columns = (0u32..)
        .zip(names)
        .map(|(id, name)| Column {
            id: ColumnId(id),
            name: name.chars().take(MAX_CELL_CHARS).collect(),
        })
        .collect();
    Read {
        table: Table { columns, rows },
        notes,
        header,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(r: &Read) -> Vec<Vec<&str>> {
        r.table
            .rows
            .iter()
            .map(|row| row.iter().map(String::as_str).collect())
            .collect()
    }

    /// Quoted fields keep their commas, line breaks and doubled quotes; CRLF and
    /// a BOM are read through.
    #[test]
    fn a_csv_reads_as_rfc_4180_says() {
        let text = "\u{feff}Name,Note,Value\r\n\"Smith, J\",\"said \"\"hi\"\"\nthen left\",3\r\nLee,,4\r\n";
        let r = read(text, Format::Csv, None).unwrap();
        assert!(r.header);
        let names: Vec<&str> = r.table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Name", "Note", "Value"]);
        assert_eq!(
            cells(&r),
            [
                vec!["Smith, J", "said \"hi\"\nthen left", "3"],
                vec!["Lee", "", "4"]
            ]
        );
    }

    /// A semicolon file is read as one; a first row of numbers is data, not a
    /// header.
    #[test]
    fn the_delimiter_and_the_header_are_detected() {
        let r = read("a;b\n1;2\n", Format::Csv, None).unwrap();
        assert_eq!(cells(&r), [vec!["1", "2"]]);
        let r = read("1,2\n3,4\n", Format::Csv, None).unwrap();
        assert!(!r.header);
        assert_eq!(r.table.columns[0].name, "Column 1");
        assert_eq!(r.table.rows.len(), 2);
        let r = read("x\ty\n1\t2", Format::Tsv, None).unwrap();
        assert_eq!(cells(&r), [vec!["1", "2"]]);
    }

    /// JSON's keys become columns in first-seen order; numbers, strings and
    /// booleans are text, `null` is empty, and a nested value is left empty and
    /// named in a note.
    #[test]
    fn json_reads_an_array_of_objects() {
        let text = r#"[{"m":"Jan","v":1.5,"x":null},{"v":2,"ok":true,"m":"Feb","n":{"a":1}}]"#;
        let r = read(text, Format::Json, None).unwrap();
        let names: Vec<&str> = r.table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["m", "v", "x", "ok", "n"]);
        assert_eq!(
            cells(&r),
            [
                vec!["Jan", "1.5", "", "", ""],
                vec!["Feb", "2", "", "true", ""]
            ]
        );
        assert_eq!(r.notes.len(), 1, "{:?}", r.notes);
        assert_eq!(read("{}", Format::Json, None), Err(Unreadable::NotObjects));
        assert_eq!(read("[1]", Format::Json, None), Err(Unreadable::NotObjects));
        assert_eq!(read("[", Format::Json, None), Err(Unreadable::BadJson));
    }

    /// A JSON number written with an exponent lands as a cell `parse_number`
    /// reads, at its value.
    /// flip: `read_json` writing a float with `Number::to_string` → fails at "1e21".
    #[test]
    fn a_json_exponent_reads_as_a_number() {
        let text = r#"[{"a":0.00001,"b":1e21,"c":-2.5e-3,"d":1e-7}]"#;
        let r = read(text, Format::Json, None).unwrap();
        let row = &r.table.rows[0];
        for (cell, want, what) in [
            (&row[0], 0.00001, "1e-5"),
            (&row[1], 1e21, "1e21"),
            (&row[2], -2.5e-3, "-2.5e-3"),
            (&row[3], 1e-7, "1e-7"),
        ] {
            assert_eq!(
                crate::chart::parse_number(cell),
                Some(want),
                "{what}: {cell}"
            );
        }
    }

    /// JSON with more distinct keys than a table holds keeps the first
    /// `MAX_COLUMNS` and says how many it left, without a cell per key per row;
    /// a key repeated in an object keeps its last value.
    /// flip: `json_columns` naming every key, for `build` to cut → fails at
    /// "named only as far as kept" (every row then holds a cell per key).
    #[test]
    fn json_of_many_keys_reads_only_the_columns_kept() {
        let n = 5_000;
        let mut text = String::from("[");
        for i in 0..n {
            text.push_str(&format!("{}{{\"k{i}\":{i}}}", if i > 0 { "," } else { "" }));
        }
        text.push(']');
        let items: Vec<Ordered> = serde_json::from_str(&text).unwrap();
        let (names, past) = json_columns(&items);
        assert_eq!(
            (names.len(), past),
            (MAX_COLUMNS, n - MAX_COLUMNS),
            "named only as far as kept"
        );
        let r = read(&text, Format::Json, None).unwrap();
        assert_eq!(r.table.columns.len(), MAX_COLUMNS);
        assert_eq!(r.table.rows.len(), n);
        assert_eq!(
            r.notes[0],
            format!("{} columns past {MAX_COLUMNS} left out", n - MAX_COLUMNS)
        );
        let r = read(r#"[{"a":1,"b":2,"a":3}]"#, Format::Json, None).unwrap();
        let names: Vec<&str> = r.table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
        assert_eq!(cells(&r), [vec!["2", "3"]]);
    }

    /// A JSON number too long to write out without an exponent keeps the
    /// file's spelling as text, and is said, rather than being cut to a cell
    /// and read at another value.
    /// flip: `read_json` writing every float by `Display` → fails at "1e300".
    #[test]
    fn a_json_number_past_a_cell_stays_as_written() {
        let r = read(
            r#"[{"v":1e300},{"v":1e-300},{"v":2.5}]"#,
            Format::Json,
            None,
        )
        .unwrap();
        assert_eq!(
            cells(&r),
            [vec!["1e+300"], vec!["1e-300"], vec!["2.5"]],
            "1e300"
        );
        assert!(
            r.notes.iter().any(|n| n.contains("too large or too small")),
            "{:?}",
            r.notes
        );
    }

    /// A `"` inside a field (an inch mark) opens no quote, so the delimiter
    /// sniffer reads past it as the reader does.
    /// flip: `unquoted` opening a quote at any `"` → fails at "two columns".
    #[test]
    fn an_inch_mark_quotes_nothing() {
        let r = read("Screen 5\";Units\nA;3\nB;4\n", Format::Csv, None).unwrap();
        assert_eq!(cells(&r), [vec!["A", "3"], vec!["B", "4"]], "two columns");
        let r = read("\"a;b\",c,d\n1,2,3\n", Format::Csv, None).unwrap();
        assert_eq!(r.table.columns.len(), 3, "a quoted field still hides its ;");
    }

    /// Past the row limit the rest is left out and said.
    #[test]
    fn the_limits_hold_and_are_said() {
        let mut text = String::from("a\n");
        for i in 0..MAX_ROWS + 3 {
            text.push_str(&format!("{i}\n"));
        }
        let r = read(&text, Format::Csv, None).unwrap();
        assert_eq!(r.table.rows.len(), MAX_ROWS);
        assert!(r.notes[0].starts_with("3 rows past"), "{:?}", r.notes);
    }
}
