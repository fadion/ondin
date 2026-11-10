use super::*;

/// The numbers a cell parses as, and the text it does not.
#[test]
fn a_cell_parses_with_a_thousands_separator_and_not_a_decimal_comma() {
    for (cell, want) in [
        ("42", Some(42.0)),
        (" -3.5 ", Some(-3.5)),
        ("+7", Some(7.0)),
        ("1,234.5", Some(1234.5)),
        ("12,345,678", Some(12_345_678.0)),
        (".5", Some(0.5)),
        ("1,23", None),
        ("1234,567", None),
        ("3,5", None),
        ("1.", None),
        ("n/a", None),
        ("", None),
        ("-", None),
        ("inf", None),
        ("NaN", None),
        ("1e3", None),
    ] {
        assert_eq!(parse_number(cell), want, "{cell:?}");
    }
}

/// Every kind's sample table is a valid description that draws with nothing
/// left out, a Sankey's in three columns of nodes.
#[test]
fn every_sample_validates_and_draws_whole() {
    let size = kurbo::Size::new(480.0, 280.0);
    for kind in ChartKind::ALL {
        let chart = Chart::sample(kind);
        assert_eq!(chart.validate(), Ok(()), "{kind:?}");
        let got = generate(&chart, size, "Inter").expect("draws");
        assert!(got.left_out.is_empty(), "{kind:?}: {:?}", got.left_out);
        assert!(!got.parts.is_empty(), "{kind:?}");
    }
    let got = generate(&Chart::sample(ChartKind::Sankey), size, "Inter").unwrap();
    let mut lefts: Vec<i64> = got
        .parts
        .iter()
        .filter(|p| p.key.role == PartRole::SankeyNode)
        .map(|p| p.transform.translation().x.round() as i64)
        .collect();
    lefts.sort_unstable();
    lefts.dedup();
    assert_eq!(lefts.len(), 3, "node columns: {lefts:?}");
}
