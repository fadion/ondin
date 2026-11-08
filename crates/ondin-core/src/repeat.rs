//! *Repeat around origin*: copies of a selection turned step by step about one
//! point, grouped with the original as one drawing.
//!
//! The copies are plain copies of the originals as they stand, so a main's copies
//! land as its instances (`component::settle_copy`), which is all *Linked* asks.

use crate::build;
use crate::kurbo::{Affine, Point, Rect};
use crate::{Document, IdSource, NodeId, OpError, Operation, Resolved, Transaction};

/// The most copies a ring makes, the original counted: past this a ring is a
/// texture, and the preview would stall.
pub const MAX_COUNT: usize = 360;

/// How the copies are laid round the point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    /// How many there are, the original counted: 12 is a full clock face.
    pub count: usize,
    /// The turn from one to the next, in radians, clockwise on screen.
    pub step: f64,
    /// Whether each copy is turned with its step, as rays are; off, the copies
    /// keep the original's orientation and only travel round.
    pub turn: bool,
}

impl Ring {
    /// A full turn split evenly between `count`.
    pub fn full(count: usize, turn: bool) -> Self {
        Self {
            count,
            step: std::f64::consts::TAU / count.max(1) as f64,
            turn,
        }
    }

    /// The world-space affine that carries an original to copy `k` (1 is the next
    /// one round), about `pivot`. `centre` is what travels when the copies are not
    /// turned: the middle of the selection's box.
    pub fn carry(&self, k: usize, pivot: Point, centre: Point) -> Affine {
        let turn = Affine::translate(pivot.to_vec2())
            * Affine::rotate(self.step * k as f64)
            * Affine::translate(-pivot.to_vec2());
        match self.turn {
            true => turn,
            false => Affine::translate((turn * centre) - centre),
        }
    }
}

/// Where the copies go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A new group that takes the originals, named for the first and the count:
    /// the commit.
    Group,
    /// Beside each original, under it, in its own parent — and at `fade` of its
    /// opacity: the preview, which must not reparent what is on the canvas.
    Beside { fade: u8 },
}

/// The middle of the box `members` occupy, in world space.
pub fn centre(res: &Resolved, members: &[NodeId]) -> Option<Point> {
    members
        .iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|a, b| a.union(b))
        .map(|b: Rect| b.center())
}

/// Why a ring is refused for a layer in an auto layout.
pub const IN_LAYOUT: &str = "Repeat can't work inside an auto layout";

/// Whether any of `members` sits in a parent that lays its children out. There the
/// ring's group, or the copies beside the original, would be laid out as items:
/// the original and its siblings would move and the ring would not stand round
/// its point. Refused, as the shape builder and the knife refuse such a layer.
pub fn in_layout(doc: &Document, members: &[NodeId]) -> bool {
    let view = crate::resolve::DocView(doc);
    members
        .iter()
        .any(|id| crate::container::parent_lays_out(&view, *id))
}

/// The name the group takes: *Tick × 12*.
pub fn group_name(first: &str, count: usize) -> String {
    format!("{first} × {count}")
}

/// Repeat `selection` round `pivot` (world space) as `ring` says.
///
/// Answers the edit and, under [`Place::Group`], the group made. The members must
/// share a parent to be grouped, one that lays nothing out ([`in_layout`]); the
/// originals stay first in the group, the copies below them in order round the ring.
pub fn repeat(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    selection: &[NodeId],
    pivot: Point,
    ring: Ring,
    into: Place,
) -> Result<(Transaction, Option<NodeId>), OpError> {
    let members = build::in_document_order(doc, selection);
    if members.is_empty() || ring.count < 2 || ring.count > MAX_COUNT {
        return Err(OpError::MalformedSubtree);
    }
    if in_layout(doc, &members) {
        return Err(OpError::InvalidParent);
    }
    let centre = centre(res, &members).ok_or(OpError::EmptyGeometry)?;
    let (mut ops, group) = match into {
        Place::Group => {
            let (tx, group) = build::group(doc, res, ids, &members)?;
            let top = *members.last().expect("checked non-empty");
            let first = doc
                .get(top)
                .map(|n| n.name().to_string())
                .unwrap_or_default();
            let mut ops = tx.0;
            ops.push(Operation::SetName {
                id: group,
                name: group_name(&first, ring.count),
            });
            (ops, Some(group))
        }
        Place::Beside { .. } => (Vec::new(), None),
    };
    // Each step's block goes in under the one before it, so the layers read the
    // original, then the copies in order round the ring.
    for k in 1..ring.count {
        let carry = ring.carry(k, pivot, centre);
        for (i, &m) in members.iter().enumerate() {
            let Some(template) = doc.capture_subtree(m) else {
                continue;
            };
            let Some((mut nodes, root)) = crate::document::remap_subtree(&template, ids) else {
                continue;
            };
            crate::component::settle_copy(
                doc,
                &template,
                &mut nodes,
                crate::component::MainCopy::Instance,
            );
            let world = res.world_transform(m).ok_or(OpError::NoSuchNode(m))?;
            let transform = build::local_for_world(doc, res, m, carry * world);
            let (parent, index) = match (into, group) {
                (Place::Group, Some(g)) => (g, i),
                _ => {
                    let node = doc.get(m).ok_or(OpError::NoSuchNode(m))?;
                    let parent = node.parent().ok_or(OpError::CannotModifyRoot)?;
                    let at = doc
                        .get(parent)
                        .and_then(|p| p.children().iter().position(|c| *c == m))
                        .unwrap_or(0);
                    (parent, at)
                }
            };
            if let (Place::Beside { fade }, Some(n)) =
                (into, nodes.iter_mut().find(|n| n.id == root))
            {
                n.opacity = doc.drawn_opacity(m) * f32::from(fade) / 255.0;
                n.opacity_var = None;
            }
            ops.push(Operation::InsertSubtree {
                nodes,
                parent,
                index,
            });
            ops.push(Operation::SetTransform {
                id: root,
                transform,
            });
        }
    }
    Ok((Transaction(ops), group))
}

/// Why a *Linked* ring cannot make its original a component first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotLinked {
    /// Linked copies repeat one layer at a time.
    Several,
    /// It is a main or an instance: its copies are linked already.
    Linked,
    /// *Create component*'s rule (`component::why_not_make`).
    Not(crate::component::NotMain),
}

/// Why a linked ring of the outermost layers `roots` would refuse, or `None`.
pub fn why_not_linked(doc: &Document, roots: &[NodeId]) -> Option<NotLinked> {
    let [one] = roots else {
        return Some(NotLinked::Several);
    };
    let node = doc.get(*one)?;
    if node.component() || crate::component::instance_root(doc, *one) == Some(*one) {
        return Some(NotLinked::Linked);
    }
    crate::component::why_not_make(doc, roots).map(NotLinked::Not)
}

/// A *Linked* ring of `id`: the layer made a component where it stands (a frame
/// or a group), or wrapped in a group made the component, then repeated, so its
/// copies land as instances. A group rather than *Create component*'s frame: the
/// wrapper only carries the link, and a frame would add a clip and a fill nobody
/// asked for. The wrapper takes the layer's name, so the main, its instances and
/// the ring read as the layer. Worked on a scratch copy so both halves are one edit.
pub fn linked(
    doc: &Document,
    res: &Resolved,
    ids: &mut IdSource,
    id: NodeId,
    pivot: Point,
    ring: Ring,
) -> Result<(Transaction, Option<NodeId>), OpError> {
    let (mut tx, main) = match crate::component::makes_in_place(doc, id) {
        true => (Transaction(Vec::new()), id),
        false => {
            let (mut tx, group) = build::group(doc, res, ids, &[id])?;
            if let Some(node) = doc.get(id) {
                tx.0.push(Operation::SetName {
                    id: group,
                    name: node.name().to_string(),
                });
            }
            (tx, group)
        }
    };
    tx.0.push(Operation::SetComponent {
        id: main,
        component: true,
    });
    let mut after = doc.clone();
    let outcome = after.apply(&tx)?;
    let mut laid = res.clone();
    laid.update(&after, &outcome.dirty);
    let (more, group) = repeat(&after, &laid, ids, &[main], pivot, ring, Place::Group)?;
    tx.0.extend(more.0);
    Ok((tx, group))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;
    use crate::container::{Display, Flex};
    use crate::kurbo::Size;

    /// A layer in a row frame is refused, as a ring, a preview or a linked ring,
    /// so neither it nor its sibling moves; out of the row it repeats.
    /// flip: `repeat` without its `in_layout` refusal → fails at "the ring is refused".
    #[test]
    fn a_layer_in_an_auto_layout_is_refused() {
        let mut ids = IdSource::new(0x2E9);
        let root = ids.mint();
        let (frame, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let mut doc = Document::new(root);
        let rect = |id, index| Operation::CreateNode {
            id,
            parent: frame,
            index,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 30.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 300.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            rect(a, 0),
            rect(b, 1),
            Operation::SetDisplay {
                id: frame,
                display: Some(Display::Flex(Flex::default())),
            },
        ]))
        .expect("the fixture");
        let res = Resolved::rebuild(&doc);
        assert!(in_layout(&doc, &[a]));
        let ring = Ring::full(4, true);
        let pivot = Point::new(40.0, 120.0);
        for into in [Place::Group, Place::Beside { fade: 115 }] {
            let made = repeat(&doc, &res, &mut ids, &[a], pivot, ring, into);
            assert!(
                matches!(made, Err(OpError::InvalidParent)),
                "the ring is refused"
            );
        }
        assert!(linked(&doc, &res, &mut ids, a, pivot, ring).is_err());

        doc.apply(&Transaction(vec![Operation::SetDisplay {
            id: frame,
            display: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        assert!(repeat(&doc, &res, &mut ids, &[a], pivot, ring, Place::Group).is_ok());
    }
}
