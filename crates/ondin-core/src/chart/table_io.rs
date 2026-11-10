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
    let count = |d: u8| {
        let mut quoted = false;
        line.bytes()
            .filter(|b| {
                if *b == b'"' {
                    quoted = !quoted;
                }
                !quoted && *b == d
            })
            .count()
    };
    let (comma, semi, tab) = (count(b','), count(b';'), count(b'\t'));
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
    let mut quoted = false;
    for (i, ch) in text.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '\n' | '\r' if !quoted => return &text[..i],
            _ => {}
        }
    }
    text
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
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
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
    Ok(build(names, rows, header))
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
                let mut out = Vec::new();
                while let Some((k, v)) = m.next_entry::<String, serde_json::Value>()? {
                    out.retain(|(o, _): &(String, serde_json::Value)| *o != k);
                    out.push((k, v));
                }
                Ok(Ordered(out))
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
    let mut names: Vec<String> = Vec::new();
    for item in &items {
        for (k, _) in &item.0 {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }
    let mut nested: Vec<String> = Vec::new();
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let get = |k: &str| item.0.iter().find(|(o, _)| o == k).map(|(_, v)| v);
            names
                .iter()
                .map(|k| match get(k) {
                    None | Some(serde_json::Value::Null) => String::new(),
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(serde_json::Value::Number(n)) => n.to_string(),
                    Some(serde_json::Value::Bool(b)) => b.to_string(),
                    Some(serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
                        if !nested.contains(k) {
                            nested.push(k.clone());
                        }
                        String::new()
                    }
                })
                .collect()
        })
        .collect();
    let mut read = build(names, rows, true);
    for k in nested {
        read.notes
            .push(format!("“{k}” held a list or an object, left empty"));
    }
    Ok(read)
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
fn build(mut names: Vec<String>, mut rows: Vec<Vec<String>>, header: bool) -> Read {
    let mut notes = Vec::new();
    if names.len() > MAX_COLUMNS {
        notes.push(format!(
            "{} columns past {MAX_COLUMNS} left out",
            names.len() - MAX_COLUMNS
        ));
        names.truncate(MAX_COLUMNS);
    }
    if rows.len() > MAX_ROWS {
        notes.push(format!(
            "{} rows past {} left out",
            rows.len() - MAX_ROWS,
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
