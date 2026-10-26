//! `ondin render` — data-driven renders (§9): a component once per row of a CSV,
//! or every frame once per mode.
//!
//! Every edit goes through an [`EditorSession`]'s commit, so a row's variant switch
//! and property values settle exactly as the inspector's would; each render is then
//! undone back to the placed instance or the untouched frame. Files come from
//! `ondin_export::plan::render`, the bytes `export --all` writes.

use crate::session::EditorSession;
use ondin_core::variant::{self, PropKind, PropValue, Property};
use ondin_core::{Document, ExportFormat, ExportScale, ExportSpec, NodeId, NodeKind, Operation};

const USAGE: &str = "usage: ondin render <file.ondin> --component <name> --data <rows.csv> \
     [--name-by <column>] [--png|--svg] [--scale <n>|--width <px>|--height <px>] [-o <dir>]\n       \
     ondin render <file.ondin> --all-modes [--page <name|n>] [--png|--svg] \
     [--scale <n>|--width <px>|--height <px>] [-o <dir>]";

/// A parsed `ondin render` command line.
#[derive(Debug, PartialEq)]
pub(crate) struct Render {
    pub file: String,
    pub what: What,
    pub format: ExportFormat,
    pub scale: ExportScale,
    /// The folder the files go to; the document's own folder when unset.
    pub out: Option<String>,
}

/// What `ondin render` renders.
#[derive(Debug, PartialEq)]
pub(crate) enum What {
    /// The component named `component`, once per row of the CSV at `data`.
    Rows {
        component: String,
        data: String,
        /// The column whose cell names each row's file, instead of its number.
        name_by: Option<String>,
    },
    /// Every visible top-level frame of the pages that are not drafts, or of the
    /// one `--page` names, once per mode of the document.
    Modes { page: Option<String> },
}

/// Parse the arguments after `render`. The flags are `export`'s where the two
/// share a question; PNG is the default, since a render is a picture.
pub(crate) fn parse(args: &[String]) -> Result<Render, String> {
    let mut file = None;
    let mut format = None;
    let mut out = None;
    let mut page = None;
    let mut component = None;
    let mut data = None;
    let mut name_by = None;
    let mut modes = false;
    let mut scale: Option<(&str, ExportScale)> = None;
    let mut i = 0;
    while i < args.len() {
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            let v = args.get(*i).ok_or_else(|| USAGE.to_string())?;
            // A flag is not a value: `-o --png` would write into a folder `--png`.
            if v.starts_with('-') {
                return Err(format!(
                    "{} wants a value, not the option {v}\n{USAGE}",
                    args[*i - 1]
                ));
            }
            Ok(v.clone())
        };
        match args[i].as_str() {
            "--png" => format = Some(ExportFormat::Png),
            "--svg" => format = Some(ExportFormat::Svg),
            "--all-modes" => modes = true,
            "--component" => component = Some(value(&mut i)?),
            "--data" => data = Some(value(&mut i)?),
            "--name-by" => name_by = Some(value(&mut i)?),
            "--page" => page = Some(value(&mut i)?),
            "-o" | "--out" => out = Some(value(&mut i)?),
            flag @ ("--scale" | "--width" | "--height") => {
                let text = value(&mut i)?;
                let asked = crate::size_flag(flag, &text, USAGE)?;
                if let Some((had, _)) = scale {
                    return Err(format!(
                        "{had} and {flag} are two answers to one question — pick one\n{USAGE}"
                    ));
                }
                scale = Some((flag, asked));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other if file.is_some() => {
                return Err(format!(
                    "render takes one document; got {} and {other}\n{USAGE}",
                    file.as_deref().unwrap_or_default()
                ));
            }
            other => file = Some(other.to_string()),
        }
        i += 1;
    }
    let format = format.unwrap_or(ExportFormat::Png);
    if let Some((had, _)) = scale
        && format != ExportFormat::Png
    {
        return Err(format!(
            "{had} is a raster size, so it applies only to --png\n{USAGE}"
        ));
    }
    let what = match (modes, component, data) {
        (true, None, None) if name_by.is_none() => What::Modes { page },
        (true, ..) => {
            return Err(format!(
                "--all-modes renders the document's frames; it takes no --component, \
                 --data or --name-by\n{USAGE}"
            ));
        }
        (false, Some(component), Some(data)) => {
            if page.is_some() {
                return Err(format!(
                    "--page applies only to --all-modes; a component is found on any page\n{USAGE}"
                ));
            }
            What::Rows {
                component,
                data,
                name_by,
            }
        }
        (false, Some(_), None) => return Err(format!("--component needs --data\n{USAGE}")),
        (false, None, Some(_)) => return Err(format!("--data needs --component\n{USAGE}")),
        (false, None, None) => return Err(USAGE.to_string()),
    };
    Ok(Render {
        file: file.ok_or_else(|| USAGE.to_string())?,
        what,
        format,
        scale: scale.map_or(ExportScale::Times(1.0), |(_, s)| s),
        out,
    })
}

/// Run a parsed `ondin render`. Every file is rendered before any is written, as
/// `export --all` does, and a row that cannot be applied fails the whole run
/// before anything is rendered.
pub(crate) fn run(r: &Render) -> Result<(), String> {
    let bytes = std::fs::read(&r.file).map_err(|e| format!("cannot read {}: {e}", r.file))?;
    let doc = ondin_core::io::load(&bytes).map_err(|e| format!("cannot load {}: {e}", r.file))?;
    let mut session = EditorSession::new();
    session.adopt_document(doc, None);
    let spec = ExportSpec::new(r.format, r.scale);
    let files = match &r.what {
        What::Rows {
            component,
            data,
            name_by,
        } => {
            let text =
                std::fs::read_to_string(data).map_err(|e| format!("cannot read {data}: {e}"))?;
            let table = csv(&text).map_err(|e| format!("{data}: {e}"))?;
            render_rows(&mut session, component, &table, name_by.as_deref(), &spec)?
        }
        What::Modes { page } => render_modes(&mut session, page.as_deref(), &spec)?,
    };
    let dir = r
        .out
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| std::path::Path::new(&r.file).parent().map(Into::into))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut written = Vec::new();
    let mut skipped = 0usize;
    for (path, data) in files {
        match data {
            Some(data) => written.push((dir.join(path), data)),
            None => {
                eprintln!("skipped {path} — it has no size");
                skipped += 1;
            }
        }
    }
    if written.is_empty() {
        return Err(format!(
            "{}: every render was skipped — nothing had a size ({skipped} skipped)",
            r.file
        ));
    }
    let replaced = crate::write_planned(&written)?;
    println!(
        "{}",
        crate::export_all_report(written.len(), replaced, skipped, &dir)
    );
    Ok(())
}

/// A rendered file: its path under the destination and its bytes, `None` when
/// the layer had nothing to frame.
type Rendered = (String, Option<Vec<u8>>);

/// One row's edits, checked against the document before anything renders.
#[derive(Debug, Default)]
struct RowEdits {
    /// The variant to switch to, when the row names one other than the default.
    variant: Option<NodeId>,
    props: Vec<(usize, Property, PropValue)>,
}

/// What a column of the table drives.
#[derive(Clone, Debug)]
enum Column {
    /// The set's variant property at this index.
    Variant(usize),
    Prop(Property),
    /// Nothing: unmatched, or only `--name-by`'s.
    None,
}

/// Render `component` once per row of `table`, its first row the header.
fn render_rows(
    s: &mut EditorSession,
    component: &str,
    table: &[Vec<String>],
    name_by: Option<&str>,
    spec: &ExportSpec,
) -> Result<Vec<Rendered>, String> {
    let owner = find_component(&s.doc, component)?;
    let default = match s.doc.get(owner).and_then(|n| n.set()) {
        Some(_) => *variant::variants(&s.doc, owner)
            .first()
            .ok_or_else(|| format!("component set {component:?} has no variants"))?,
        None => owner,
    };
    let inst = place_instance(s, default)?;
    let (header, rows) = table
        .split_first()
        .ok_or_else(|| "the CSV is empty; its first row names the properties".to_string())?;
    let name_col = match name_by {
        Some(name) => Some(
            header
                .iter()
                .position(|h| h.trim() == name)
                .ok_or_else(|| format!("--name-by {name:?}: the CSV has no such column"))?,
        ),
        None => None,
    };
    let columns = columns(&s.doc, owner, inst, header, name_col, component)?;
    let edits = rows
        .iter()
        .enumerate()
        .map(|(r, row)| row_edits(&s.doc, owner, default, &columns, header, r + 1, row))
        .collect::<Result<Vec<_>, _>>()?;

    let stem = ondin_export::plan::sanitize_segment(component);
    let stem = if stem.is_empty() {
        "Component".into()
    } else {
        stem
    };
    let width = rows.len().to_string().len();
    let mut taken = Vec::new();
    let base = s.history.undo_depth();
    let mut out = Vec::with_capacity(rows.len());
    for (r, (row, edit)) in rows.iter().zip(&edits).enumerate() {
        let n = r + 1;
        apply_row(s, inst, edit, header, n)?;
        let named = name_col
            .and_then(|c| row.get(c))
            .map(|cell| ondin_export::plan::sanitize_segment(cell.trim()))
            .filter(|cell| !cell.is_empty());
        let name = named.unwrap_or_else(|| format!("{stem}-{n:0width$}"));
        let path = ondin_export::plan::de_collide(
            format!("{name}.{}", spec.format.extension()),
            &mut taken,
        );
        out.push(render_one(s, inst, spec, path));
        undo_to(s, base);
    }
    Ok(out)
}

/// Render every visible top-level frame of the chosen pages once per mode of the
/// document, each frame set to that mode for its render.
fn render_modes(
    s: &mut EditorSession,
    page: Option<&str>,
    spec: &ExportSpec,
) -> Result<Vec<Rendered>, String> {
    let modes: Vec<(ondin_core::variable::ModeId, String)> = s
        .doc
        .variables()
        .modes()
        .iter()
        .map(|m| (m.id, m.name.clone()))
        .collect();
    if modes.is_empty() {
        return Err(
            "the document has no modes; --all-modes renders each frame once per mode".to_string(),
        );
    }
    let pages: Vec<NodeId> = match page {
        Some(_) => vec![crate::chosen_page(&s.doc, page)?],
        None => s.doc.gathered_pages().collect(),
    };
    let frames: Vec<NodeId> = pages
        .iter()
        .filter_map(|p| s.doc.get(*p))
        .flat_map(|p| p.children().iter().copied())
        .filter(|id| {
            s.doc
                .get(*id)
                .is_some_and(|n| n.visible() && matches!(n.kind(), NodeKind::Artboard { .. }))
        })
        .collect();
    if frames.is_empty() {
        return Err("no visible top-level frame to render".to_string());
    }
    let base = s.history.undo_depth();
    let mut taken = Vec::new();
    let mut out = Vec::with_capacity(frames.len() * modes.len());
    for frame in frames {
        let name = s
            .doc
            .get(frame)
            .map(|n| n.name().to_string())
            .unwrap_or_default();
        for (mode, mode_name) in &modes {
            s.try_commit(ondin_core::Transaction(vec![Operation::SetMode {
                id: frame,
                mode: Some(*mode),
            }]))
            .map_err(|e| format!("cannot set {name:?} to mode {mode_name:?}: {e}"))?;
            let stem = format!(
                "{}-{}",
                ondin_export::plan::sanitize_segment(&name),
                ondin_export::plan::sanitize_segment(mode_name)
            );
            let path = ondin_export::plan::de_collide(
                format!("{stem}.{}", spec.format.extension()),
                &mut taken,
            );
            out.push(render_one(s, frame, spec, path));
            undo_to(s, base);
        }
    }
    Ok(out)
}

/// `node` rendered to `spec` from the session's committed state, saying so when
/// the raster limit took the size down.
fn render_one(s: &EditorSession, node: NodeId, spec: &ExportSpec, path: String) -> Rendered {
    let (pixels, clamped) = match spec.format.is_raster() {
        true => ondin_export::plan::raster_size(&s.resolved, node, spec),
        false => (None, false),
    };
    if clamped && let Some((w, h)) = pixels {
        eprintln!("{path}: past the raster limit, written at {w} × {h}");
    }
    let file = ondin_export::PlannedFile {
        node,
        spec: spec.clone(),
        path,
        pixels,
        clamped,
    };
    let data = ondin_export::plan::render(&s.doc, &s.resolved, &file);
    (file.path, data)
}

/// Undo back to the history depth `base`, where the render started from.
fn undo_to(s: &mut EditorSession, base: usize) {
    while s.history.undo_depth() > base {
        let before = s.history.undo_depth();
        s.undo();
        // An undo that fails leaves the step in place; stop rather than spin.
        if s.history.undo_depth() >= before {
            break;
        }
    }
}

/// The component a name finds: a main that is not a variant, or a component set,
/// on any page or in the kit vault. Refused when none or several carry the name.
fn find_component(doc: &Document, name: &str) -> Result<NodeId, String> {
    let roots: Vec<NodeId> = doc.page_ids().chain(doc.vault()).collect();
    let mut found = Vec::new();
    let mut names = Vec::new();
    for id in ondin_core::build::subtree_nodes(doc, &roots) {
        let Some(n) = doc.get(id) else { continue };
        let is_component =
            n.set().is_some() || (n.component() && variant::set_of(doc, id).is_none());
        if !is_component {
            continue;
        }
        if n.name() == name {
            found.push(id);
        }
        names.push(n.name().to_string());
    }
    match found.as_slice() {
        [one] => Ok(*one),
        [] => {
            names.sort();
            names.dedup();
            Err(match names.is_empty() {
                true => format!("no component called {name:?}; the document has none"),
                false => format!(
                    "no component called {name:?}; the components are {}",
                    names.join(", ")
                ),
            })
        }
        many => Err(format!(
            "{} components are called {name:?}; rename all but one",
            many.len()
        )),
    }
}

/// Place an instance of `main` on its page — the first page for a kit main — and
/// answer its root.
fn place_instance(s: &mut EditorSession, main: NodeId) -> Result<NodeId, String> {
    let doc = &s.doc;
    let page = doc.page_of(main).unwrap_or(doc.first_page());
    let template = doc
        .capture_subtree(main)
        .ok_or_else(|| "the component is not in the document".to_string())?;
    let (tx, made) = ondin_core::insert_subtrees(
        doc,
        &mut s.ids,
        &[ondin_core::build::Placement {
            nodes: template,
            parent: page,
            index: None,
        }],
        Default::default(),
    );
    s.try_commit(tx)
        .map_err(|e| format!("cannot place an instance: {e}"))?;
    made.first()
        .copied()
        .ok_or_else(|| "cannot place an instance".to_string())
}

/// What each header cell drives, warning once for a column that matches nothing.
/// A name matches exactly, else ignoring case when that finds exactly one.
fn columns(
    doc: &Document,
    owner: NodeId,
    inst: NodeId,
    header: &[String],
    name_col: Option<usize>,
    component: &str,
) -> Result<Vec<Column>, String> {
    let set_props: Vec<String> = doc
        .get(owner)
        .and_then(|n| n.set())
        .map(|s| s.props.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default();
    let props: Vec<Property> = variant::instance_properties(doc, inst)
        .into_iter()
        .map(|k| k.value)
        .filter(|p| p.kind != PropKind::Nested)
        .collect();
    let names: Vec<&str> = set_props
        .iter()
        .map(String::as_str)
        .chain(props.iter().map(|p| p.name.as_str()))
        .collect();
    let mut out = Vec::with_capacity(header.len());
    let mut seen: Vec<usize> = Vec::new();
    for (c, cell) in header.iter().enumerate() {
        let h = cell.trim();
        let exact = names.iter().position(|n| *n == h);
        let loose = || {
            let hits: Vec<usize> = (0..names.len())
                .filter(|i| names[*i].eq_ignore_ascii_case(h))
                .collect();
            (hits.len() == 1).then(|| hits[0])
        };
        let Some(at) = exact.or_else(loose) else {
            if Some(c) != name_col {
                eprintln!("column {h:?} matches no property of {component:?}; skipped");
            }
            out.push(Column::None);
            continue;
        };
        if seen.contains(&at) {
            return Err(format!(
                "two columns name the property {:?}; keep one",
                names[at]
            ));
        }
        seen.push(at);
        out.push(match at < set_props.len() {
            true => Column::Variant(at),
            false => Column::Prop(props[at - set_props.len()].clone()),
        });
    }
    Ok(out)
}

/// Row `n`'s cells read against the columns. An empty cell keeps the
/// component's default.
fn row_edits(
    doc: &Document,
    owner: NodeId,
    default: NodeId,
    columns: &[Column],
    header: &[String],
    n: usize,
    row: &[String],
) -> Result<RowEdits, String> {
    if row.len() > header.len() {
        return Err(format!(
            "row {n} has {} cells; the header has {}",
            row.len(),
            header.len()
        ));
    }
    let at = |c: usize| format!("row {n}, column {:?}", header[c].trim());
    let set = doc.get(owner).and_then(|o| o.set()).cloned();
    let mut values: Vec<String> = doc
        .get(default)
        .map(|d| d.variant().to_vec())
        .unwrap_or_default();
    let mut switched = false;
    let mut edits = RowEdits::default();
    for (c, cell) in row.iter().enumerate() {
        if cell.is_empty() {
            continue;
        }
        match &columns[c] {
            Column::None => {}
            Column::Variant(pi) => {
                let Some(prop) = set.as_ref().and_then(|s| s.props.get(*pi)) else {
                    continue;
                };
                let value = prop
                    .values
                    .iter()
                    .find(|v| *v == cell)
                    .or_else(|| prop.values.iter().find(|v| v.eq_ignore_ascii_case(cell)))
                    .ok_or_else(|| {
                        format!(
                            "{}: {cell:?} is not a value of {} ({})",
                            at(c),
                            prop.name,
                            prop.values.join(", ")
                        )
                    })?;
                if let Some(slot) = values.get_mut(*pi) {
                    *slot = value.clone();
                    switched = true;
                }
            }
            Column::Prop(p) => {
                let value = match p.kind {
                    PropKind::Boolean => PropValue::Boolean(boolean(cell).ok_or_else(|| {
                        format!(
                            "{}: {cell:?} is not a boolean (true/false, yes/no, on/off, 1/0)",
                            at(c)
                        )
                    })?),
                    PropKind::Text => PropValue::Text(cell.clone()),
                    // A set shows its first variant, as a placed instance would.
                    PropKind::Swap => {
                        let found =
                            find_component(doc, cell).map_err(|e| format!("{}: {e}", at(c)))?;
                        let main = match doc.get(found).and_then(|n| n.set()) {
                            Some(_) => variant::variants(doc, found)
                                .first()
                                .copied()
                                .ok_or_else(|| format!("{}: {cell:?} has no variants", at(c)))?,
                            None => found,
                        };
                        PropValue::Swap(main)
                    }
                    PropKind::Nested => continue,
                };
                edits.props.push((c, p.clone(), value));
            }
        }
    }
    if switched {
        let to = variant::find_variant(doc, owner, &values).ok_or_else(|| {
            let named: Vec<String> = set
                .iter()
                .flat_map(|s| s.props.iter())
                .zip(&values)
                .map(|(p, v)| format!("{}={v}", p.name))
                .collect();
            format!("row {n}: no variant has {}", named.join(", "))
        })?;
        edits.variant = (to != default).then_some(to);
    }
    Ok(edits)
}

/// Commit row `n`'s edits on the instance `inst`: the variant switch first, then
/// every property in one transaction.
fn apply_row(
    s: &mut EditorSession,
    inst: NodeId,
    edit: &RowEdits,
    header: &[String],
    n: usize,
) -> Result<(), String> {
    if let Some(to) = edit.variant {
        let tx = variant::switch(&s.doc, inst, to, &mut s.ids)
            .ok_or_else(|| format!("row {n}: cannot switch to that variant"))?;
        s.try_commit(tx)
            .map_err(|e| format!("row {n}: the variant switch failed: {e}"))?;
    }
    let mut ops = Vec::new();
    for (c, p, value) in &edit.props {
        if let PropValue::Swap(main) = value {
            let doc = &s.doc;
            let fits = variant::counterparts(doc, inst, p).iter().all(|cp| {
                ondin_core::swap::slot_main(doc, *cp) == Some(*main)
                    || ondin_core::swap::can_swap_to(doc, *cp, *main)
            });
            if !fits {
                return Err(format!(
                    "row {n}, column {:?}: {:?} cannot be shown there",
                    header[*c].trim(),
                    doc.get(*main).map(|m| m.name()).unwrap_or_default()
                ));
            }
        }
        ops.extend(variant::set_property(&s.doc, &[inst], p, value));
    }
    s.try_commit(ondin_core::Transaction(ops))
        .map_err(|e| format!("row {n}: the properties failed: {e}"))
}

/// A boolean cell, read the ways a spreadsheet writes one.
fn boolean(cell: &str) -> Option<bool> {
    match cell.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// A CSV's rows: RFC 4180 quoting (`""` inside quotes, line breaks inside quoted
/// cells), CRLF or LF, a leading byte-order mark dropped and blank lines skipped.
/// The separator is a comma, or a semicolon when the first line has semicolons
/// and no commas — what a spreadsheet writes in a locale with a decimal comma.
fn csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first = text.lines().next().unwrap_or("");
    let sep = match !first.contains(',') && first.contains(';') {
        true => ';',
        false => ',',
    };
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut line = 1usize;
    let mut chars = text.chars().peekable();
    let end_row = |row: &mut Vec<String>, cell: &mut String, rows: &mut Vec<Vec<String>>| {
        row.push(std::mem::take(cell));
        let row = std::mem::take(row);
        if !(row.len() == 1 && row[0].is_empty()) {
            rows.push(row);
        }
    };
    while let Some(ch) = chars.next() {
        match (quoted, ch) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            (true, '"') => quoted = false,
            (true, ch) => {
                if ch == '\n' {
                    line += 1;
                }
                cell.push(ch);
            }
            (false, '"') if cell.is_empty() => quoted = true,
            (false, c) if c == sep => row.push(std::mem::take(&mut cell)),
            (false, '\r') if chars.peek() == Some(&'\n') => {}
            (false, '\n') => {
                line += 1;
                end_row(&mut row, &mut cell, &mut rows);
            }
            (false, ch) => cell.push(ch),
        }
    }
    if quoted {
        return Err(format!(
            "a quoted cell opened before line {line} never closes"
        ));
    }
    if !cell.is_empty() || !row.is_empty() {
        end_row(&mut row, &mut cell, &mut rows);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::Size;
    use ondin_core::peniko::Color;
    use ondin_core::variant::{VariantProp, VariantSet};
    use ondin_core::{Brush, Fill, IdSource, Keyed, Transaction, keyed_by_position};

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    fn text_kind(content: &str) -> NodeKind {
        NodeKind::Text {
            content: content.into(),
            style: Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 12.0,
                ..Default::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: ondin_core::TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// The badge's fill, looked for in the SVG to tell whether it drew.
    const BADGE: &str = "#ff00ff";

    /// A set `Button` (`Size: Small, Large`; 80 and 120 wide), each variant a
    /// text `Label` bound to *Label text* and a magenta rect `Badge` bound to the
    /// boolean *Badge*, adopted into a session.
    fn button() -> EditorSession {
        let mut ids = IdSource::new(0x5E7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [set, small, sl, sb, large, ll, lb] = [(); 7].map(|_| ids.mint());
        let frame = |w| NodeKind::Artboard {
            size: Size::new(w, 30.0),
        };
        let badge = || NodeKind::Rect {
            size: Size::new(8.0, 8.0),
            corner_radii: Default::default(),
        };
        let create = |id, parent, index, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        let magenta = |id| Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(255, 0, 255, 255)).into(),
                visible: true,
            }]),
        };
        let mut prop = |name: &str, kind, bound| {
            Keyed::new(
                ids.mint_item(),
                Property {
                    name: name.into(),
                    kind,
                    bound,
                    filter: String::new(),
                    preferred: Vec::new(),
                },
            )
        };
        let props = vec![
            prop("Label text", PropKind::Text, vec![sl, ll]),
            prop("Badge", PropKind::Boolean, vec![sb, lb]),
        ];
        doc.apply(&Transaction(vec![
            create(set, root, 0, frame(300.0), "Button"),
            create(small, set, 0, frame(80.0), "Small"),
            create(sl, small, 0, text_kind("Go"), "Label"),
            create(sb, small, 1, badge(), "Badge"),
            create(large, set, 1, frame(120.0), "Large"),
            create(ll, large, 0, text_kind("Go"), "Label"),
            create(lb, large, 1, badge(), "Badge"),
            magenta(sb),
            magenta(lb),
            Operation::SetComponent {
                id: small,
                component: true,
            },
            Operation::SetComponent {
                id: large,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Small".into(), "Large".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: small,
                values: vec!["Small".into()],
            },
            Operation::SetVariant {
                id: large,
                values: vec!["Large".into()],
            },
            Operation::SetProperties { id: set, props },
        ]))
        .expect("a set of two variants");
        let mut s = EditorSession::new();
        s.adopt_document(doc, None);
        s
    }

    fn svg_spec() -> ExportSpec {
        ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0))
    }

    fn table(text: &str) -> Vec<Vec<String>> {
        csv(text).expect("a well-formed CSV")
    }

    fn svg(r: &Rendered) -> String {
        String::from_utf8(r.1.clone().expect("rendered")).expect("SVG is UTF-8")
    }

    /// Quoting, a doubled quote, a line break and a separator inside quotes, CRLF,
    /// a byte-order mark and blank lines all read as a spreadsheet wrote them; a
    /// semicolon header switches the separator.
    #[test]
    fn csv_reads_what_a_spreadsheet_writes() {
        let rows =
            table("\u{feff}Name,Note\r\n\"Ada, Countess\",\"says \"\"hi\"\"\nthen\"\r\n\r\nBob,\n");
        assert_eq!(
            rows,
            vec![
                vec!["Name".to_string(), "Note".into()],
                vec!["Ada, Countess".into(), "says \"hi\"\nthen".into()],
                vec!["Bob".into(), String::new()],
            ]
        );
        assert_eq!(
            table("a;b\n1,5;2\n"),
            vec![
                vec!["a".to_string(), "b".into()],
                vec!["1,5".into(), "2".into()]
            ]
        );
        assert!(csv("a\n\"open").unwrap_err().contains("never closes"));
    }

    /// Both forms parse, PNG at 1× by default, and the mixed or incomplete ones are
    /// refused with a sentence rather than half-run.
    #[test]
    fn render_parses_its_two_forms_and_refuses_the_mixed_ones() {
        assert_eq!(
            parse(&args(&[
                "d.ondin",
                "--component",
                "Card",
                "--data",
                "rows.csv",
                "--name-by",
                "Slug",
                "--width",
                "512",
                "-o",
                "out",
            ])),
            Ok(Render {
                file: "d.ondin".into(),
                what: What::Rows {
                    component: "Card".into(),
                    data: "rows.csv".into(),
                    name_by: Some("Slug".into()),
                },
                format: ExportFormat::Png,
                scale: ExportScale::Width(512),
                out: Some("out".into()),
            })
        );
        assert_eq!(
            parse(&args(&["d.ondin", "--all-modes", "--svg", "--page", "2"])),
            Ok(Render {
                file: "d.ondin".into(),
                what: What::Modes {
                    page: Some("2".into())
                },
                format: ExportFormat::Svg,
                scale: ExportScale::Times(1.0),
                out: None,
            })
        );
        assert!(matches!(
            crate::parse(&args(&["render", "d.ondin", "--all-modes"])),
            Ok(crate::Command::Render(_))
        ));
        for (bad, says) in [
            (&["d.ondin"][..], "usage"),
            (&["d.ondin", "--component", "C"], "needs --data"),
            (
                &["d.ondin", "--all-modes", "--component", "C"],
                "takes no --component",
            ),
            (
                &["d.ondin", "--component", "C", "--data", "r", "--page", "1"],
                "--page applies only",
            ),
            (
                &["d.ondin", "--all-modes", "--svg", "--scale", "2"],
                "only to --png",
            ),
            (&["d.ondin", "--all-modes", "-o", "--png"], "wants a value"),
            (
                &["d.ondin", "--all-modes", "--width", "1", "--scale", "2"],
                "two answers",
            ),
            (&["d.ondin", "e.ondin", "--all-modes"], "one document"),
        ] {
            let err = parse(&args(bad)).unwrap_err();
            assert!(err.contains(says), "{bad:?}: {err}");
        }
    }

    /// Each row renders its own variant and property values, named by `--name-by`
    /// or by component and row number, and the session is back at the placed
    /// instance afterwards.
    ///
    /// flip: `render_rows` without its `undo_to` → fails at row 2's width, which
    /// keeps row 1's Large variant.
    #[test]
    fn each_row_renders_its_variant_and_properties() {
        let mut s = button();
        let rows = table("Size,Label text,Badge,Slug\nLarge,Sign up,no,cta\n,\"Hi, there\",,\n");
        let out = render_rows(&mut s, "Button", &rows, Some("Slug"), &svg_spec())
            .expect("both rows render");
        let names: Vec<&str> = out.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["cta.svg", "Button-2.svg"]);
        let (one, two) = (svg(&out[0]), svg(&out[1]));
        assert!(one.contains("width=\"120\""), "row 1 is Large: {one}");
        assert!(one.contains("Sign up") && !one.contains(BADGE), "{one}");
        assert!(two.contains("width=\"80\""), "row 2 is Small: {two}");
        assert!(two.contains("Hi, there") && two.contains(BADGE), "{two}");
        assert_eq!(
            s.history.undo_depth(),
            1,
            "only the instance's placement is left"
        );
    }

    /// The command end to end from files on disk: one PNG per row into `-o`, at
    /// the asked scale, read back out of each PNG's header.
    #[test]
    fn run_writes_one_png_per_row() {
        let dir = std::env::temp_dir().join(format!("ondin-render-rows-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.ondin");
        std::fs::write(&src, ondin_core::io::save(&button().doc).unwrap()).unwrap();
        let data = dir.join("rows.csv");
        std::fs::write(&data, "Size\nLarge\nSmall\n").unwrap();
        let out = dir.join("out");
        run(&Render {
            file: src.to_string_lossy().into_owned(),
            what: What::Rows {
                component: "Button".into(),
                data: data.to_string_lossy().into_owned(),
                name_by: None,
            },
            format: ExportFormat::Png,
            scale: ExportScale::Times(2.0),
            out: Some(out.to_string_lossy().into_owned()),
        })
        .expect("the render runs");
        let size = |name: &str| {
            let png = std::fs::read(out.join(name)).expect("written");
            let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
            (be(16), be(20))
        };
        assert_eq!(size("Button-1.png"), (240, 60), "Large at 2×");
        assert_eq!(size("Button-2.png"), (160, 60), "Small at 2×");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cell that does not read for its property is refused naming its row and
    /// column, before anything renders; a column that matches nothing is skipped.
    #[test]
    fn a_bad_cell_is_refused_by_row_and_column() {
        for (csv_text, says) in [
            (
                "Badge\nyes\nmaybe\n",
                "row 2, column \"Badge\": \"maybe\" is not a boolean",
            ),
            (
                "Size\nHuge\n",
                "row 1, column \"Size\": \"Huge\" is not a value of Size (Small, Large)",
            ),
            ("Size\nSmall,extra\n", "row 1 has 2 cells; the header has 1"),
            (
                "Badge,badge\n1,0\n",
                "two columns name the property \"Badge\"",
            ),
        ] {
            let mut s = button();
            let err =
                render_rows(&mut s, "Button", &table(csv_text), None, &svg_spec()).unwrap_err();
            assert!(err.contains(says), "{csv_text:?}: {err}");
        }
        let mut s = button();
        let out = render_rows(
            &mut s,
            "Button",
            &table("Colour,size\nRed,large\n"),
            None,
            &svg_spec(),
        )
        .expect("an unknown column is skipped, and a name matches without case");
        assert!(svg(&out[0]).contains("width=\"120\""));
        let err = render_rows(&mut s, "Buton", &table("a\n1\n"), None, &svg_spec()).unwrap_err();
        assert!(err.contains("the components are Button"), "{err}");
    }

    /// Every visible top-level frame renders once per mode, in that mode, named
    /// frame then mode, and the frame inherits again afterwards.
    ///
    /// flip: `render_modes` without its `SetMode` commit → fails at the Dark
    /// render's colour.
    #[test]
    fn every_frame_renders_once_per_mode() {
        use ondin_core::BrushSource;
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let mut ids = IdSource::new(0x30D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (light, dark) = (ids.mint_mode(), ids.mint_mode());
        let (var, card) = (ids.mint_var(), ids.mint());
        let mode = |id, name: &str, index| Operation::AddMode {
            mode: Mode {
                id,
                name: name.into(),
            },
            index,
            cells: vec![],
        };
        doc.apply(&Transaction(vec![
            mode(light, "Light", 0),
            mode(dark, "Dark", 1),
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "surface".into(),
                    kind: VarKind::Color,
                    cells: [
                        (light, Cell::Color(Color::from_rgba8(250, 250, 250, 255))),
                        (dark, Cell::Color(Color::from_rgba8(20, 20, 20, 255))),
                    ]
                    .into_iter()
                    .collect(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::CreateNode {
                id: card,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 40.0),
                },
                transform: None,
                name: Some("Card".into()),
            },
            Operation::SetFills {
                id: card,
                fills: keyed_by_position([Fill {
                    brush: BrushSource::Bound { var, alpha: 1.0 },
                    visible: true,
                }]),
            },
        ]))
        .expect("a frame filled from a variable in two modes");
        let mut s = EditorSession::new();
        s.adopt_document(doc, None);
        let out = render_modes(&mut s, None, &svg_spec()).expect("both modes render");
        let names: Vec<&str> = out.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["Card-Light.svg", "Card-Dark.svg"]);
        assert!(svg(&out[0]).contains("#fafafa"), "{}", svg(&out[0]));
        assert!(svg(&out[1]).contains("#141414"), "{}", svg(&out[1]));
        assert_eq!(s.doc.get(card).unwrap().mode(), None, "inherits again");
    }
}
