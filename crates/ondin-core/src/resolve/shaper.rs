//! Text shaped once per distinct text for the length of one resolve pass.

use crate::node::{NodeKind, TextRef};
use crate::text::{self, TextLayout};
use rustc_hash::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The layouts and content widths one [`super::Resolved::rebuild`] or
/// [`super::Resolved::update`] has shaped, keyed by the text kind itself, so
/// identical texts — a kit's hundred "Button" labels — are shaped once and share
/// one [`TextLayout`].
///
/// **Lives for one pass, never across commits.** Shaping also reads the fonts
/// registered on this thread; within a pass they cannot change, and across passes
/// the per-node `TextMemo` and `Resolved::text` keep what is still valid, so this
/// needs no invalidation and no eviction.
///
/// A hit is `NodeKind`'s `==` over every field the layout reads. The one
/// difference it lets through: a float field equal under `==` but not bit for
/// bit (`0.0` and `-0.0`) can share a layout, unless it is a field [`bucket`]
/// hashes.
#[derive(Default)]
pub(super) struct Shaper {
    layouts: Buckets<Arc<TextLayout>>,
    widths: Buckets<(f64, f64)>,
}

/// Results filed by [`bucket`], each beside the text kind it was shaped from.
type Buckets<T> = FxHashMap<u64, Vec<(NodeKind, T)>>;

impl Shaper {
    /// `kind`'s layout, shaped on the first ask for a text equal to it. `None`
    /// for any kind but a text.
    pub(super) fn layout(&mut self, kind: &NodeKind) -> Option<Arc<TextLayout>> {
        let parts = TextRef::of(kind)?;
        let bucket = self.layouts.entry(bucket(&parts)).or_default();
        if let Some((_, layout)) = bucket.iter().find(|(k, _)| k == kind) {
            return Some(layout.clone());
        }
        let layout = Arc::new(text::layout(parts));
        bucket.push((kind.clone(), layout.clone()));
        Some(layout)
    }

    /// [`text::content_widths`] of `kind`, on [`Self::layout`]'s terms.
    pub(super) fn content_widths(&mut self, kind: &NodeKind) -> Option<(f64, f64)> {
        let parts = TextRef::of(kind)?;
        let bucket = self.widths.entry(bucket(&parts)).or_default();
        if let Some((_, w)) = bucket.iter().find(|(k, _)| k == kind) {
            return Some(*w);
        }
        let w = text::content_widths(parts);
        bucket.push((kind.clone(), w));
        Some(w)
    }
}

/// The hash a text is filed under: the fields that tell most texts apart, cheap
/// to read. Floats by their bits. Which fields are hashed only spreads the
/// buckets; a hit is decided by `==` over the whole kind.
fn bucket(parts: &TextRef<'_>) -> u64 {
    let mut h = FxHasher::default();
    parts.content.hash(&mut h);
    parts.style.font_family.hash(&mut h);
    parts.style.font_size.to_bits().hash(&mut h);
    super::sizing_key_of(parts.sizing).hash(&mut h);
    parts.on_path.is_some().hash(&mut h);
    parts.on_path_offset.to_bits().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::TextSizing;
    use crate::typography::TextStyle;

    fn label(content: &str, sizing: TextSizing) -> NodeKind {
        NodeKind::Text {
            content: content.into(),
            style: Box::new(TextStyle {
                font_family: "Inter".into(),
                font_size: 12.0,
                ..Default::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// Equal texts are shaped once and share the layout; a different content,
    /// wrap width or style (outside the hashed fields) is shaped on its own.
    ///
    /// flip: `Shaper::layout`'s lookup matching on the bucket alone → the weight
    /// change shares the regular layout and fails at "a heavier weight".
    #[test]
    fn equal_texts_share_one_layout_and_different_ones_do_not() {
        let mut shaper = Shaper::default();
        let a = label("Button", TextSizing::Auto);
        let before = text::shapes();
        let first = shaper.layout(&a).unwrap();
        let again = shaper.layout(&a.clone()).unwrap();
        assert!(
            Arc::ptr_eq(&first, &again),
            "an equal text shares the layout"
        );
        assert_eq!(text::shapes() - before, 1, "and is shaped once");
        assert_eq!(*first, text::layout(TextRef::of(&a).unwrap()));

        let other = shaper.layout(&label("Cancel", TextSizing::Auto)).unwrap();
        assert!(!Arc::ptr_eq(&first, &other), "another content");
        let wrapped = shaper.layout(&label("Button", TextSizing::AutoHeight(20.0)));
        assert!(
            !Arc::ptr_eq(&first, &wrapped.unwrap()),
            "another wrap width"
        );
        let mut bold = a.clone();
        if let NodeKind::Text { style, .. } = &mut bold {
            style.weight = 700;
        }
        let bold_layout = shaper.layout(&bold).unwrap();
        assert!(!Arc::ptr_eq(&first, &bold_layout), "a heavier weight");
        assert_eq!(*bold_layout, text::layout(TextRef::of(&bold).unwrap()));
    }

    /// Content widths are shared on the same terms and equal a direct measure.
    #[test]
    fn content_widths_are_shared_and_exact() {
        let mut shaper = Shaper::default();
        let a = label("A longer label", TextSizing::Auto);
        let before = text::shapes();
        let w = shaper.content_widths(&a).unwrap();
        assert_eq!(shaper.content_widths(&a.clone()), Some(w));
        assert_eq!(text::shapes() - before, 1);
        assert_eq!(w, text::content_widths(TextRef::of(&a).unwrap()));
    }

    /// Any kind but a text has no layout.
    #[test]
    fn a_shape_has_no_layout() {
        let rect = NodeKind::Rect {
            size: kurbo::Size::new(1.0, 1.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        assert!(Shaper::default().layout(&rect).is_none());
        assert!(Shaper::default().content_widths(&rect).is_none());
    }
}
