//! What an import takes (`ondin_core::take`): the census it offers and the trim
//! that leaves out what was not chosen, each leaving the layers drawn as they were.

use ondin_core::image::{ImageEntry, ImageFormat, ImageId, ImageSource, image_brush};
use ondin_core::item::Keyed;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::style::StyleSlot;
use ondin_core::take::{Keep, census, take};
use ondin_core::{
    Brush, BrushSource, Cell, Document, Fill, IdSource, Mode, NodeId, NodeKind, Operation,
    Transaction, VarKind, Variable,
};

const RED: Color = Color::from_rgba8(200, 30, 30, 255);
const BLUE: Color = Color::from_rgba8(20, 40, 220, 255);

/// Two pages. *Flows* holds a main `m` (a frame around rect `ma`), an instance
/// `i` of it, and rect `bound`, filled by colour variable `var` in two modes and
/// styled with fill style `style`. *Archive* holds a frame whose rect shows
/// picture `old`; *Flows*' `m` shows picture `hero`.
struct F {
    doc: Document,
    flows: NodeId,
    archive: NodeId,
    m: NodeId,
    i: NodeId,
    ia: NodeId,
    bound: NodeId,
}

fn frame(id: NodeId, parent: NodeId, index: usize) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(100.0, 100.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }
}

fn rect(id: NodeId, parent: NodeId) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }
}

fn picture(id: &str, bytes: usize) -> Operation {
    Operation::AddImage {
        id: ImageId(id.into()),
        entry: ImageEntry {
            source: ImageSource::Embedded(vec![0u8; bytes].into()),
            format: ImageFormat::Png,
            width: 4,
            height: 4,
        },
    }
}

fn fills(ids: &mut IdSource, id: NodeId, brush: BrushSource) -> Operation {
    Operation::SetFills {
        id,
        fills: vec![Keyed::new(
            ids.mint_item(),
            Fill {
                brush,
                visible: true,
            },
        )],
    }
}

fn commit(doc: &mut Document, ids: &mut IdSource, ops: Vec<Operation>) {
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(doc, &mut tx, ids);
    doc.apply(&tx).expect("the fixture commits");
}

fn fixture() -> F {
    let mut ids = IdSource::new(0x7A);
    let flows = ids.mint();
    let mut doc = Document::new(flows);
    let [archive, m, ma, i, ia, bound, old_frame, old] = [(); 8].map(|_| ids.mint());
    let (light, dark, var) = (ids.mint_mode(), ids.mint_mode(), ids.mint_var());
    let hero = image_brush(ImageId("hero".into()));
    let old_pic = image_brush(ImageId("old".into()));
    let mut ops = vec![
        Operation::SetName {
            id: flows,
            name: "Flows".into(),
        },
        Operation::AddPage {
            id: archive,
            index: 1,
            name: "Archive".into(),
            ground: Color::WHITE,
            draft: false,
            mode: None,
        },
        picture("hero", 10),
        picture("old", 20),
        frame(m, flows, 0),
        rect(ma, m),
        Operation::SetComponent {
            id: m,
            component: true,
        },
        frame(i, flows, 1),
        rect(ia, i),
        Operation::SetLink {
            id: i,
            link: Some(m),
        },
        Operation::SetLink {
            id: ia,
            link: Some(ma),
        },
        rect(bound, flows),
        frame(old_frame, archive, 0),
        rect(old, old_frame),
        Operation::AddMode {
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: var,
                name: "accent".into(),
                kind: VarKind::Color,
                cells: [(light, Cell::Color(RED)), (dark, Cell::Color(BLUE))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ];
    ops.push(fills(&mut ids, m, hero.clone().into()));
    ops.push(fills(&mut ids, i, hero.into()));
    ops.push(fills(&mut ids, ma, Brush::Solid(RED).into()));
    ops.push(fills(&mut ids, ia, Brush::Solid(RED).into()));
    ops.push(fills(&mut ids, old, old_pic.into()));
    ops.push(fills(
        &mut ids,
        bound,
        BrushSource::Bound { var, alpha: 1.0 },
    ));
    commit(&mut doc, &mut ids, ops);
    let style = ids.mint_style();
    let ops = ondin_core::style::create_ops(&doc, style, "surface", ia, StyleSlot::Fills, &[ia])
        .expect("a fill style from the instance's rect");
    commit(&mut doc, &mut ids, ops);
    F {
        doc,
        flows,
        archive,
        m,
        i,
        ia,
        bound,
    }
}

/// Trim the fixture to everything but what `edit` turns off.
fn trim(f: &mut F, edit: impl FnOnce(&mut Keep)) {
    let mut keep = Keep::all(&f.doc);
    edit(&mut keep);
    take(&mut f.doc, &keep).expect("the trim goes through");
}

fn fill(doc: &Document, id: NodeId) -> BrushSource {
    doc.get(id).unwrap().paint().fills[0].brush.clone()
}

/// The census counts pages, the frames on each, mains, styles, variables, modes
/// and pictures with their bytes.
#[test]
fn the_census_counts_what_the_import_offers() {
    let f = fixture();
    let c = census(&f.doc);
    let pages: Vec<(&str, usize)> = c
        .pages
        .iter()
        .map(|p| (p.name.as_str(), p.frames))
        .collect();
    assert_eq!(pages, [("Flows", 2), ("Archive", 1)]);
    assert_eq!(
        (c.frames, c.components, c.styles, c.variables, c.modes),
        (3, 1, 1, 1, 2)
    );
    assert_eq!((c.pictures, c.picture_bytes), (2, 30));
}

/// Keeping everything changes nothing a census can see.
#[test]
fn keeping_everything_keeps_everything() {
    let mut f = fixture();
    let before = census(&f.doc);
    trim(&mut f, |_| {});
    assert_eq!(census(&f.doc), before);
}

/// A page left out goes with its layers and the picture only it showed.
/// flip: drop the unshown-picture removal in `take` → fails at the picture count.
#[test]
fn a_page_left_out_takes_its_pictures_along() {
    let mut f = fixture();
    let keep = Keep {
        pages: vec![f.flows],
        ..Keep::all(&f.doc)
    };
    take(&mut f.doc, &keep).unwrap();
    assert!(!f.doc.is_page(f.archive));
    assert_eq!(f.doc.pages().len(), 1);
    let ids: Vec<&str> = f.doc.images().map(|(id, _)| id.0.as_str()).collect();
    assert_eq!(ids, ["hero"]);
}

/// Without components, the instance is plain layers still showing the main's
/// picture, and the main is a plain frame.
#[test]
fn without_components_instances_become_plain_layers() {
    let mut f = fixture();
    let shown = fill(&f.doc, f.i);
    trim(&mut f, |k| k.components = false);
    assert!(!f.doc.get(f.m).unwrap().component());
    assert_eq!(f.doc.get(f.i).unwrap().link(), None);
    assert_eq!(f.doc.get(f.ia).unwrap().link(), None);
    assert_eq!(fill(&f.doc, f.i), shown);
    assert_eq!(census(&f.doc).components, 0);
}

/// Without variables, a bound fill becomes the colour it drew, and the table
/// and its modes are empty.
#[test]
fn without_variables_a_bound_value_becomes_the_value_it_drew() {
    let mut f = fixture();
    trim(&mut f, |k| k.variables = false);
    assert_eq!(
        fill(&f.doc, f.bound),
        BrushSource::Literal(Brush::Solid(RED))
    );
    let c = census(&f.doc);
    assert_eq!((c.variables, c.modes), (0, 0));
}

/// Without styles, the styled layer keeps its fill and is bound to nothing.
#[test]
fn without_styles_each_use_keeps_its_values() {
    let mut f = fixture();
    let shown = fill(&f.doc, f.ia);
    trim(&mut f, |k| k.styles = false);
    assert_eq!(census(&f.doc).styles, 0);
    assert_eq!(f.doc.get(f.ia).unwrap().styles().fills, None);
    assert_eq!(fill(&f.doc, f.ia), shown);
}

/// Without pictures, the table is empty and the image fills stay.
#[test]
fn without_pictures_the_fills_stay_and_the_table_empties() {
    let mut f = fixture();
    let shown = fill(&f.doc, f.m);
    let keep = Keep::all_but_pictures(&f.doc);
    take(&mut f.doc, &keep).unwrap();
    assert_eq!(f.doc.image_count(), 0);
    assert_eq!(fill(&f.doc, f.m), shown);
}

/// Everything off at once, and the result still saves and loads.
#[test]
fn a_document_trimmed_of_everything_still_loads() {
    let mut f = fixture();
    let keep = Keep {
        pages: vec![f.flows],
        components: false,
        styles: false,
        variables: false,
        pictures: false,
    };
    take(&mut f.doc, &keep).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).expect("a trimmed document loads");
    let c = census(&back);
    assert_eq!(
        (
            c.pages.len(),
            c.components,
            c.styles,
            c.variables,
            c.pictures
        ),
        (1, 0, 0, 0, 0)
    );
}

/// No page kept is refused, and the document is left alone.
#[test]
fn taking_no_page_is_refused() {
    let mut f = fixture();
    let before = census(&f.doc);
    let keep = Keep {
        pages: vec![],
        ..Keep::all(&f.doc)
    };
    assert!(take(&mut f.doc, &keep).is_err());
    assert_eq!(census(&f.doc), before);
}
