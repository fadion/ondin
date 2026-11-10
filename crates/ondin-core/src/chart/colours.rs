//! A chart's colours as the generator assigns them: a series' or a Sankey
//! node's own style, else the palette's colour at its place. The Chart card's
//! swatches and the picker read and write through these.

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
    let colours = palette(chart.options.palette);
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
    let colours = palette(chart.options.palette);
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

/// `chart` with Sankey node `name` drawn in `fill`.
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
/// at its place, so picking a palette recolours the whole chart.
pub fn with_palette(chart: &Chart, p: super::Palette) -> Chart {
    let mut next = chart.clone();
    next.options.palette = p;
    let colours = palette(p);
    for s in &mut next.series {
        if let Some(n) = next.mapping.series.iter().position(|c| *c == s.column) {
            s.fill = literal(colours[n % colours.len()]);
        }
    }
    next.nodes.clear();
    next
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
        let colours = palette(chart.options.palette);
        assert_eq!(series_fill(&chart, second), Some(literal(colours[1])));
        let red = literal([255, 0, 0]);
        let painted = with_series_fill(&chart, second, red.clone());
        assert_eq!(series_fill(&painted, second), Some(red));
        let warm = with_palette(&painted, super::super::Palette::Warm);
        assert_eq!(
            series_fill(&warm, second),
            Some(literal(palette(super::super::Palette::Warm)[1]))
        );
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
