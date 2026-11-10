//! A chart in code export: written as the frames it is, its layout as CSS, so
//! it resizes in a browser as it does on the canvas, and read as one picture.

use ondin_core::chart::{Chart, ChartKind};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction};
use ondin_export::code::{CodeExport, Target};

/// A page holding one settled sample chart of `kind`, 480 × 280, named *Sales*.
fn chart_doc(kind: ChartKind) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0xC7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let chart = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: chart,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(480.0, 280.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some("Sales".into()),
        },
        Operation::SetChart {
            id: chart,
            chart: Some(Box::new(Chart::sample(kind))),
        },
    ]))
    .unwrap();
    let res = Resolved::settled(&mut doc);
    let parts = doc.get(chart).unwrap().chart().unwrap().parts.len();
    assert!(parts > 10, "{kind:?} settled into its parts: {parts}");
    (doc, res, chart)
}

fn file(out: &CodeExport, path: &str) -> String {
    String::from_utf8(out.file(path).unwrap().to_vec()).unwrap()
}

/// The declarations of `.class`'s rule in `css`.
fn rule<'a>(css: &'a str, class: &str) -> &'a str {
    let head = format!("\n.{class} {{\n");
    let at = css
        .find(&head)
        .unwrap_or_else(|| panic!("no rule for .{class}\n{css}"));
    let body = &css[at + head.len()..];
    &body[..body.find('}').unwrap()]
}

/// The chart frame is `role="img"` named by its layer and its parts are hidden;
/// the body is the grid with the plot's `minmax(0px, 1fr)` tracks, and its cells
/// are 100% of their areas with no px floor, so a narrower page shrinks the plot.
///
/// flip: `Walk::sizes` writing a layout-less frame's own box → fails at "the
/// plot fills its cell"; dropping the percentage test of the replaced minimum →
/// fails at "nothing holds the plot at its size".
#[test]
fn a_chart_writes_its_layout_and_reads_as_one_picture() {
    let (doc, res, chart) = chart_doc(ChartKind::Column);
    let out = Target::Html.export(&doc, &res, &[chart]);
    let html = file(&out, "index.html");
    let css = file(&out, "styles.css");
    assert!(
        html.contains(r#"<div class="sales" role="img" aria-label="Sales">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="legend" aria-hidden="true">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="body" aria-hidden="true">"#),
        "{html}"
    );
    assert_eq!(
        html.matches("aria-hidden").count(),
        2,
        "the frame's two parts hide what is under them"
    );

    let sales = rule(&css, "sales");
    assert!(sales.contains("display: flex;"), "{sales}");
    assert!(sales.contains("flex-direction: column;"), "{sales}");
    let body = rule(&css, "body");
    assert!(body.contains("display: grid;"), "{body}");
    assert!(body.contains("minmax(0px, 1fr)"), "{body}");
    let plot = rule(&css, "plot");
    assert!(
        plot.contains("width: 100%;") && plot.contains("height: 100%;"),
        "the plot fills its cell: {plot}"
    );
    assert!(
        !plot.contains("min-width") && !plot.contains("min-height"),
        "nothing holds the plot at its size: {plot}"
    );
}

/// A column is pinned by percentage insets of the plot, a line is an inline
/// `<svg>` stretching in a box pinned the same way, and no kind is drawn
/// differently from the canvas.
#[test]
fn marks_are_pinned_by_percentages_and_paths_stretch() {
    let (doc, res, chart) = chart_doc(ChartKind::Column);
    let out = Target::Html.export(&doc, &res, &[chart]);
    let css = file(&out, "styles.css");
    let col = rule(&css, "revenue-jan");
    for side in ["left", "right", "top", "bottom"] {
        assert!(
            col.lines()
                .any(|l| l.trim().starts_with(&format!("{side}: ")) && l.contains('%')),
            "the column's {side} is a percentage: {col}"
        );
    }

    let (doc, res, chart) = chart_doc(ChartKind::Line);
    let out = Target::Html.export(&doc, &res, &[chart]);
    let html = file(&out, "index.html");
    let css = file(&out, "styles.css");
    assert!(
        html.contains(r#"<svg class="line-revenue""#)
            && html.contains("preserveAspectRatio=\"none\""),
        "{html}"
    );
    assert!(
        html.contains("vector-effect=\"non-scaling-stroke\""),
        "{html}"
    );
    let held = rule(&css, "line-revenue-box");
    assert!(held.contains("left: 8.3333%;"), "{held}");
    let line = rule(&css, "line-revenue");
    assert!(
        line.contains("width: 100%;") && line.contains("height: 100%;"),
        "{line}"
    );

    for kind in ChartKind::ALL {
        let (doc, res, chart) = chart_doc(kind);
        let out = Target::Html.export(&doc, &res, &[chart]);
        assert!(
            out.approximated.is_empty(),
            "{kind:?}: {:?}",
            out.approximated
        );
    }
}
