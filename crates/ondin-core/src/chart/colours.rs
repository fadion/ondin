//! A chart's colours as the generator assigns them: a series' or a named
//! mark's (a Sankey node, a pie's slice, a scatter's group) own style, else
//! the palette's colour at its place. The Chart card's swatches and the picker
//! read and write through these.

use super::{Chart, ColumnId, NodeStyle, SeriesStyle, palette, parse_number};
use crate::variable::BrushSource;

fn literal(c: [u8; 3]) -> BrushSource {
    BrushSource::Literal(crate::Brush::Solid(peniko::Color::from_rgb8(
        c[0], c[1], c[2],
    )))
}

/// The fill series `column` draws in: its style's, else the palette's colour at
/// its place among the mapped series.
pub fn series_fill(chart: &Chart, column: ColumnId) -> Option<BrushSource> {
    if let Some(s) = chart.series.iter().find(|s| s.column == column) {
        return Some(s.fill.clone());
    }
    let n = chart.mapping.series.iter().position(|c| *c == column)?;
    let colours = palette(chart.options.palette, chart.options.dark);
    Some(literal(colours[n % colours.len()]))
}

/// A Sankey's node names in the order the generator meets them (rows it draws
/// only), each with the fill it draws in.
pub fn sankey_nodes(chart: &Chart) -> Vec<(String, BrushSource)> {
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let (Some(fc), Some(tc), Some(vc)) = (at(m.from), at(m.to), at(m.value)) else {
        return Vec::new();
    };
    let colours = palette(chart.options.palette, chart.options.dark);
    let mut out: Vec<(String, BrushSource)> = Vec::new();
    for row in &t.rows {
        let (from, to) = (row[fc].trim(), row[tc].trim());
        if from.is_empty() || to.is_empty() || from == to {
            continue;
        }
        if parse_number(&row[vc]).is_none_or(|v| v <= 0.0) {
            continue;
        }
        for name in [from, to] {
            if out.iter().any(|(n, _)| n == name) {
                continue;
            }
            let i = out.len();
            let fill = chart
                .nodes
                .iter()
                .find(|s| s.name == name)
                .map_or_else(|| literal(colours[i % colours.len()]), |s| s.fill.clone());
            out.push((name.to_string(), fill));
        }
    }
    out
}

/// The marks a chart colours by name rather than by series, in the order the
/// generator meets them, each with the fill it draws in: a Sankey's nodes, a
/// pie's slices (by category) and a grouped scatter's groups. Each takes its
/// style from `Chart::nodes` by name, else the palette at its place. `None`
/// for a chart coloured by series.
pub fn named_marks(chart: &Chart) -> Option<Vec<(String, BrushSource)>> {
    let t = &chart.table;
    let m = &chart.mapping;
    let colours = palette(chart.options.palette, chart.options.dark);
    let fill = |name: &str, i: usize| {
        chart
            .nodes
            .iter()
            .find(|s| s.name == name)
            .map_or_else(|| literal(colours[i % colours.len()]), |s| s.fill.clone())
    };
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    // The rows the generator reads: those with a category.
    let cat = at(m.category);
    let rows = || {
        t.rows
            .iter()
            .filter(move |r| cat.is_none_or(|c| !r[c].trim().is_empty()))
    };
    let mut out: Vec<(String, BrushSource)> = Vec::new();
    match chart.kind {
        super::ChartKind::Sankey => return Some(sankey_nodes(chart)),
        super::ChartKind::Pie => {
            // The first visible series, as the generator reads it.
            let first = m
                .series
                .iter()
                .find(|c| !chart.series.iter().any(|s| s.column == **c && !s.visible));
            let Some(vc) = at(first.copied()) else {
                return Some(out);
            };
            for (i, r) in rows().enumerate() {
                let name = cat.map_or_else(|| (i + 1).to_string(), |c| r[c].trim().to_string());
                let drawn = parse_number(&r[vc]).is_some_and(|v| v > 0.0);
                if drawn && !out.iter().any(|(n, _)| *n == name) {
                    out.push((name.clone(), fill(&name, i)));
                }
            }
        }
        super::ChartKind::Scatter if m.group.is_some() => {
            let Some(gc) = at(m.group) else {
                return Some(out);
            };
            for r in rows() {
                let g = r[gc].trim();
                let name = if g.is_empty() {
                    super::generate::NO_GROUP
                } else {
                    g
                };
                if !out.iter().any(|(n, _)| n == name) {
                    let i = out.len();
                    out.push((name.to_string(), fill(name, i)));
                }
            }
        }
        _ => return None,
    }
    Some(out)
}

/// Each of [`sankey_nodes`]' nodes' value, in its order: the larger of what flows
/// in and what flows out, as the generator sizes the node.
pub fn sankey_totals(chart: &Chart) -> Vec<f64> {
    let names: Vec<String> = sankey_nodes(chart).into_iter().map(|(n, _)| n).collect();
    let t = &chart.table;
    let m = &chart.mapping;
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let (Some(fc), Some(tc), Some(vc)) = (at(m.from), at(m.to), at(m.value)) else {
        return Vec::new();
    };
    let (mut ins, mut outs) = (vec![0.0; names.len()], vec![0.0; names.len()]);
    let index = |name: &str| names.iter().position(|n| n == name);
    for row in &t.rows {
        let (from, to) = (row[fc].trim(), row[tc].trim());
        let Some(v) = parse_number(&row[vc]).filter(|v| *v > 0.0) else {
            continue;
        };
        if from == to {
            continue;
        }
        if let (Some(a), Some(b)) = (index(from), index(to)) {
            outs[a] += v;
            ins[b] += v;
        }
    }
    ins.iter().zip(&outs).map(|(i, o)| i.max(*o)).collect()
}

/// `chart` with series `column` drawn in `fill`, its style made if it had none.
pub fn with_series_fill(chart: &Chart, column: ColumnId, fill: BrushSource) -> Chart {
    let mut next = chart.clone();
    match next.series.iter_mut().find(|s| s.column == column) {
        Some(s) => s.fill = fill,
        None => next.series.push(SeriesStyle {
            column,
            fill,
            visible: true,
            dash: Default::default(),
            point: Default::default(),
        }),
    }
    next
}

/// `chart` with the named mark `name` (a Sankey node, a pie's slice, a
/// scatter's group: [`named_marks`]) drawn in `fill`.
pub fn with_node_fill(chart: &Chart, name: &str, fill: BrushSource) -> Chart {
    let mut next = chart.clone();
    match next.nodes.iter_mut().find(|s| s.name == name) {
        Some(s) => s.fill = fill,
        None => next.nodes.push(NodeStyle {
            name: name.to_string(),
            fill,
        }),
    }
    next
}

/// `chart` on palette `p`, every series and node back on the palette's colour
/// at its place (for the ground the chart is drawn for), so picking a palette
/// recolours the whole chart.
pub fn with_palette(chart: &Chart, p: super::Palette) -> Chart {
    let mut next = chart.clone();
    next.options.palette = p;
    let colours = palette(p, next.options.dark);
    for s in &mut next.series {
        if let Some(n) = next.mapping.series.iter().position(|c| *c == s.column) {
            s.fill = literal(colours[n % colours.len()]);
        }
    }
    next.nodes.clear();
    next
}

/// `chart` drawn for a light ground or (`dark`) a dark one. Every colour still
/// at the other ground's value moves to this one's: a series' or a named
/// mark's palette colour at its place, and the ink, axes and gridlines at
/// their starting colours ([`super::chrome`]). A colour the user picked, and a
/// bound one, which its variable's modes already serve, stay.
pub fn with_dark(chart: &Chart, dark: bool) -> Chart {
    if chart.options.dark == dark {
        return chart.clone();
    }
    let (from, to) = (
        palette(chart.options.palette, !dark),
        palette(chart.options.palette, dark),
    );
    let swap = |fill: &mut BrushSource, i: usize| {
        if *fill == literal(from[i % from.len()]) {
            *fill = literal(to[i % to.len()]);
        }
    };
    let mut next = chart.clone();
    next.options.dark = dark;
    for s in &mut next.series {
        if let Some(n) = chart.mapping.series.iter().position(|c| *c == s.column) {
            swap(&mut s.fill, n);
        }
    }
    for (i, (name, _)) in named_marks(chart).into_iter().flatten().enumerate() {
        if let Some(s) = next.nodes.iter_mut().find(|s| s.name == name) {
            swap(&mut s.fill, i);
        }
    }
    let (old, new) = (super::chrome(!dark), super::chrome(dark));
    let solid = |c| BrushSource::Literal(crate::Brush::Solid(c));
    let o = &mut next.options;
    for (fill, was, now) in [
        (&mut o.text.ink, old.ink, new.ink),
        (&mut o.value_axis.colour, old.axis, new.axis),
        (&mut o.category_axis.colour, old.axis, new.axis),
        (&mut o.value_axis.grid.colour, old.grid, new.grid),
        (&mut o.category_axis.grid.colour, old.grid, new.grid),
    ] {
        if *fill == solid(was) {
            *fill = solid(now);
        }
    }
    next
}

/// `chart` with its series (its [`named_marks`], where it colours by name)
/// bound to `vars` in order, round again past the last: *From variables*, so a
/// mode switch recolours the chart. The palette stays what it was, for the
/// series a later edit adds.
pub fn with_variables(chart: &Chart, vars: &[crate::VarId]) -> Chart {
    if vars.is_empty() {
        return chart.clone();
    }
    let bound = |i: usize| BrushSource::Bound {
        var: vars[i % vars.len()],
        alpha: 1.0,
    };
    let mut next = chart.clone();
    match named_marks(chart) {
        Some(marks) => {
            for (i, (name, _)) in marks.into_iter().enumerate() {
                next = with_node_fill(&next, &name, bound(i));
            }
        }
        None => {
            for (i, col) in chart.mapping.series.iter().enumerate() {
                next = with_series_fill(&next, *col, bound(i));
            }
        }
    }
    next
}

/// The variables `chart`'s series (its [`named_marks`], where it colours by
/// name) are bound to in order, when every one is bound; what *From variables*
/// lights against.
pub fn bound_variables(chart: &Chart) -> Option<Vec<crate::VarId>> {
    let fills: Vec<BrushSource> = match named_marks(chart) {
        Some(marks) => marks.into_iter().map(|(_, f)| f).collect(),
        None => chart
            .mapping
            .series
            .iter()
            .filter_map(|c| series_fill(chart, *c))
            .collect(),
    };
    let vars: Option<Vec<crate::VarId>> = fills.iter().map(|f| f.var()).collect();
    vars.filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::ChartKind;

    /// A series with no style reads the palette at its place; a written one reads
    /// back; a palette pick puts it back on the palette.
    #[test]
    fn a_series_colour_reads_its_style_else_the_palette() {
        let chart = Chart::sample(ChartKind::Column);
        let second = chart.mapping.series[1];
        let colours = palette(chart.options.palette, chart.options.dark);
        assert_eq!(series_fill(&chart, second), Some(literal(colours[1])));
        let red = literal([255, 0, 0]);
        let painted = with_series_fill(&chart, second, red.clone());
        assert_eq!(series_fill(&painted, second), Some(red));
        let warm = with_palette(&painted, super::super::Palette::Warm);
        assert_eq!(
            series_fill(&warm, second),
            Some(literal(palette(super::super::Palette::Warm, false)[1]))
        );
    }

    /// A chart turned dark moves what is still on the palette and the starting
    /// chrome to the dark colours, keeps a picked colour, and turns back.
    /// flip: `with_dark` leaving the series styles alone → fails at "a palette
    /// series follows".
    #[test]
    fn a_chart_turned_dark_keeps_its_own_colours() {
        use crate::chart::Palette;
        let mut chart = with_palette(&Chart::sample(ChartKind::Column), Palette::Ondin);
        let [first, second, ..] = chart.mapping.series[..] else {
            panic!("the sample maps three series");
        };
        // Stored as a column mapped for the first time stores it.
        let day = palette(Palette::Ondin, false);
        chart = with_series_fill(&chart, first, literal(day[0]));
        let red = literal([255, 0, 0]);
        chart = with_series_fill(&chart, second, red.clone());
        let dark = with_dark(&chart, true);
        let night = palette(Palette::Ondin, true);
        assert_eq!(
            series_fill(&dark, first),
            Some(literal(night[0])),
            "a palette series follows"
        );
        assert_eq!(series_fill(&dark, second), Some(red), "a picked one stays");
        let solid = |c| BrushSource::Literal(crate::Brush::Solid(c));
        let o = &dark.options;
        assert_eq!(o.text.ink, solid(crate::chart::chrome(true).ink));
        assert_eq!(
            o.value_axis.grid.colour,
            solid(crate::chart::chrome(true).grid)
        );
        assert_eq!(with_dark(&dark, false), chart, "back to light, as it was");
    }

    /// A Sankey's styled node on the palette follows the ground too.
    #[test]
    fn a_styled_node_follows_the_ground() {
        let chart = Chart::sample(ChartKind::Sankey);
        let (name, fill) = named_marks(&chart).unwrap().remove(0);
        let chart = with_node_fill(&chart, &name, fill);
        let dark = with_dark(&chart, true);
        let night = palette(dark.options.palette, true);
        assert_eq!(named_marks(&dark).unwrap()[0].1, literal(night[0]));
    }

    /// A Sankey's nodes come in the order the rows first name them.
    #[test]
    fn sankey_nodes_are_in_first_seen_order() {
        let chart = Chart::sample(ChartKind::Sankey);
        let names: Vec<String> = sankey_nodes(&chart).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            [
                "Search", "Signup", "Social", "Direct", "Bounce", "Trial", "Leave"
            ]
        );
    }

    /// *From variables* binds the series in order, round again past the last,
    /// and reads back as those variables; a palette pick unbinds them.
    #[test]
    fn from_variables_binds_the_series_in_order() {
        let chart = Chart::sample(ChartKind::Column);
        let mut ids = crate::IdSource::new(1);
        let vars = [ids.mint_var(), ids.mint_var()];
        let bound = with_variables(&chart, &vars);
        assert_eq!(
            bound_variables(&bound),
            Some(vec![vars[0], vars[1], vars[0]])
        );
        let back = with_palette(&bound, super::super::Palette::Ondin);
        assert_eq!(bound_variables(&back), None);
    }

    /// A node's total is the larger of what flows in and what flows out.
    #[test]
    fn a_sankey_nodes_total_is_its_larger_side() {
        let mut chart = Chart::sample(ChartKind::Sankey);
        chart.table.rows = [["A", "B", "10"], ["B", "C", "4"], ["A", "C", "1,000"]]
            .iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect();
        assert_eq!(sankey_totals(&chart), vec![1010.0, 10.0, 1004.0]);
    }
}
