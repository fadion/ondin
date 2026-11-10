//! A chart on the clipboard: what the copy carries of a linked chart.

use ondin_core::chart::DataLink;
use ondin_core::verbs::{self, Lent};
use ondin_core::{Document, IdSource, NodeId, Operation, Transaction, io};
use serde_json::json;

/// A linked chart's file stays behind in the variables' detached twins as in
/// the subtrees, on write and on read.
/// flip: `write_carrying` writing the twins without `Chart::copied` → fails at
/// "left behind on write"; `parse` reading them without it → "dropped on read".
#[test]
fn a_detached_twin_of_a_linked_chart_leaves_its_file_behind() {
    let mut ids = IdSource::new(0xC11);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let script = json!([{ "verb": "insert_chart",
        "args": { "parent": page.to_wire(), "kind": "column" } }]);
    let made = verbs::run_script(&mut doc, &mut ids, Lent::default(), &script)
        .unwrap_or_else(|e| panic!("{e}"));
    let chart = NodeId::from_wire(&made[0][0]).unwrap();
    let mut linked = doc.get(chart).unwrap().chart().unwrap().clone();
    linked.source = Some(DataLink {
        path: "C:\\data\\sales.csv".into(),
        size: 10,
        modified: 1,
        header: None,
    });
    doc.apply(&Transaction(vec![Operation::SetChart {
        id: chart,
        chart: Some(Box::new(linked)),
    }]))
    .unwrap();
    let nodes = doc.capture_subtree(chart).unwrap();
    let vars = ondin_core::variable::Carried {
        detached: vec![nodes[0].clone()],
        ..Default::default()
    };
    let text =
        io::clip::write_carrying(std::slice::from_ref(&nodes), &[], &[], &vars, None, "Chart")
            .unwrap();
    assert!(!text.contains("sales.csv"), "left behind on write");

    // A source planted in the twin, the last chart in the payload, is dropped on read.
    let key = "\"chart\":{";
    let at = text.rfind(key).expect("the twin's chart") + key.len();
    let forged = format!(
        "{}\"source\":{{\"path\":\"C:\\\\x.csv\"}},{}",
        &text[..at],
        &text[at..]
    );
    let pasted = io::clip::read(&forged).unwrap().unwrap();
    assert_eq!(
        pasted.vars.detached.len(),
        1,
        "the fixture: the twin travels"
    );
    assert!(
        pasted.vars.detached[0].chart().unwrap().source.is_none(),
        "dropped on read"
    );
}
