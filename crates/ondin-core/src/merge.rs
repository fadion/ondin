//! Three-way merge of a document, for `ondin merge` and git's merge driver.
//!
//! The merge reads the three saved forms (each migrated to the current schema) as
//! JSON and merges them value by value:
//!
//! - A value one side changed takes that side; both sides changed alike, either.
//! - An object merges key by key. An object of one key whose key differs between
//!   the sides (an enum's variant, a layer's kind) is one value, and so are the
//!   keys `coupled` names: a text's content with its spans, a path's outline
//!   with its radii.
//! - A list of records that carry an `id` (layers, pages, guides, styles,
//!   variables, fills, effects) merges record by record, matched by id, in the
//!   order [`merge_order`] gives. A list of ids under a key `ORDERED_SETS`
//!   names (a layer's children) merges as an ordered set. Any other list of
//!   strings that kept its length (a variant's values) merges place by place.
//!   Any other list is one value.
//! - A record one side removed and the other left alone is removed; one removed
//!   by one side and changed by the other is kept, as a conflict. An image
//!   record the merged document still uses is kept whichever side removed it.
//!
//! Where both sides changed one value differently, ours wins and the place is
//! reported. The result must load as a document; if it does not — a layer moved
//! to two different parents, say — the merge keeps ours whole and says so. An
//! empty base (git's, for a file both branches added) merges two ways: what one
//! side alone holds arrives, and every value the sides hold differently is a
//! conflict.

use crate::io::{self, IoError};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// The keys whose list of strings is an ordered set of ids rather than a list
/// of values by position.
const ORDERED_SETS: [&str; 4] = ["children", "bound", "preferred", "values"];

/// A merged document and every place both sides changed differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    /// The merged document's saved bytes, canonical as `io::save` writes them.
    pub bytes: Vec<u8>,
    /// One line per conflict, ours kept at each. Empty for a clean merge.
    pub conflicts: Vec<String>,
}

/// Merge `ours` and `theirs`, both edited from `base`.
///
/// Errs only when one of the three cannot be read at all. A merge whose result
/// does not load comes back as `ours` with a conflict saying why.
pub fn merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> Result<Merged, IoError> {
    let read = |bytes: &[u8]| -> Result<Value, IoError> {
        io::migrate::migrate(serde_json::from_slice(bytes)?)
    };
    let o = if base.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        Some(read(base)?)
    };
    let (a, b) = (read(ours)?, read(theirs)?);
    let mut conflicts = Vec::new();
    let mut merged = merge3(o.as_ref(), Some(&a), Some(&b), "", &mut conflicts)
        .expect("a document's root is in ours and theirs");
    keep_used_images(&mut merged, [Some(&a), Some(&b), o.as_ref()]);
    let canonical = |value: Value| -> Result<Vec<u8>, IoError> {
        let doc = io::load(&serde_json::to_vec(&value)?)?;
        io::save(&doc)
    };
    match canonical(merged) {
        Ok(bytes) => Ok(Merged { bytes, conflicts }),
        Err(e) => {
            let mut conflicts = conflicts;
            conflicts.push(format!(
                "the merged document does not hold together ({e}), so ours is kept whole"
            ));
            Ok(Merged {
                bytes: canonical(a)?,
                conflicts,
            })
        }
    }
}

/// The merge of one value present (or not) in each of the three. `None` is
/// absent: a key not there, or a record removed.
fn merge3(
    o: Option<&Value>,
    a: Option<&Value>,
    b: Option<&Value>,
    path: &str,
    conflicts: &mut Vec<String>,
) -> Option<Value> {
    if a == b {
        return a.cloned();
    }
    if a == o {
        return b.cloned();
    }
    if b == o {
        return a.cloned();
    }
    // Both changed it, differently.
    match (o, a, b) {
        (_, Some(Value::Object(am)), Some(Value::Object(bm))) if !variant_differs(o, am, bm) => {
            let om = o.and_then(Value::as_object);
            let mut keys: Vec<&String> = am.keys().collect();
            for k in bm.keys() {
                if !am.contains_key(k) {
                    keys.push(k);
                }
            }
            let group = coupled(path);
            fn members<'m>(
                group: &[&str],
                m: Option<&'m Map<String, Value>>,
            ) -> Vec<Option<&'m Value>> {
                group.iter().map(|k| m.and_then(|m| m.get(*k))).collect()
            }
            let (go, ga, gb) = (
                members(group, om),
                members(group, Some(am)),
                members(group, Some(bm)),
            );
            // Both sides changed the group, differently: ours' members whole.
            let whole = !group.is_empty() && ga != go && gb != go && ga != gb;
            if whole {
                conflicts.push(conflict_line(&format!("{path}.{}", group.join("+"))));
            }
            let mut out = Map::new();
            for k in keys {
                if whole && group.contains(&k.as_str()) {
                    if let Some(v) = am.get(k) {
                        out.insert(k.clone(), v.clone());
                    }
                    continue;
                }
                let here = format!("{path}.{k}");
                let v = merge3(
                    om.and_then(|m| m.get(k)),
                    am.get(k),
                    bm.get(k),
                    &here,
                    conflicts,
                );
                if let Some(v) = v {
                    out.insert(k.clone(), v);
                }
            }
            Some(Value::Object(out))
        }
        (_, Some(Value::Array(aa)), Some(Value::Array(ba))) => {
            let oa: &[Value] = o.and_then(Value::as_array).map_or(&[], Vec::as_slice);
            let key = path.rsplit('.').next().unwrap_or_default();
            let strings = |l: &[Value]| l.iter().all(Value::is_string);
            if [oa, aa, ba].iter().all(|l| keyed(l)) {
                Some(Value::Array(merge_records(oa, aa, ba, path, conflicts)))
            } else if ORDERED_SETS.contains(&key)
                && [oa, aa, ba].iter().all(|l| distinct_strings(l))
            {
                let ids = |l: &[Value]| -> Vec<String> {
                    l.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                };
                let (order, clash) = merge_order(&ids(oa), &ids(aa), &ids(ba));
                if clash {
                    conflicts.push(order_conflict_line(path));
                }
                Some(Value::Array(order.into_iter().map(Value::String).collect()))
            } else if [oa, aa, ba]
                .iter()
                .all(|l| strings(l) && l.len() == aa.len())
            {
                // Values by position — a variant's, one per property of its set.
                let merged = (0..aa.len()).filter_map(|i| {
                    merge3(
                        oa.get(i),
                        aa.get(i),
                        ba.get(i),
                        &format!("{path}[{i}]"),
                        conflicts,
                    )
                });
                Some(Value::Array(merged.collect()))
            } else {
                conflicts.push(conflict_line(path));
                a.cloned()
            }
        }
        // Removed by one side and changed by the other: kept, as the change.
        (Some(_), None, Some(_)) => {
            conflicts.push(format!(
                "{}: removed by ours, changed by theirs",
                label(path)
            ));
            b.cloned()
        }
        (Some(_), Some(_), None) => {
            conflicts.push(format!(
                "{}: changed by ours, removed by theirs",
                label(path)
            ));
            a.cloned()
        }
        _ => {
            conflicts.push(conflict_line(path));
            a.cloned()
        }
    }
}

/// Whether `a` and `b` are one-key objects naming different keys, or either
/// names a different key from `o`'s: an enum switched variant, one value whole.
fn variant_differs(o: Option<&Value>, a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    let tag = |m: &Map<String, Value>| (m.len() == 1).then(|| m.keys().next().cloned()).flatten();
    let (ta, tb) = (tag(a), tag(b));
    let to = o.and_then(Value::as_object).and_then(tag);
    match (ta, tb) {
        (Some(x), Some(y)) => x != y || to.is_some_and(|t| t != x),
        _ => false,
    }
}

/// The keys of the object at `path` that are one value: a text's content with
/// the byte ranges its spans style, a path's outline with the radii indexed by
/// its anchors. Merged apart, one side's ranges land on the other's characters.
fn coupled(path: &str) -> &'static [&'static str] {
    if path.ends_with(".kind.Text") {
        &["content", "spans", "para_spans"]
    } else if path.ends_with(".kind.Path") {
        &["path", "corner_radii"]
    } else {
        &[]
    }
}

fn conflict_line(path: &str) -> String {
    format!(
        "{}: changed differently on both sides; ours kept",
        label(path)
    )
}

fn order_conflict_line(path: &str) -> String {
    format!(
        "{}: reordered differently on both sides; ours' order kept",
        label(path)
    )
}

/// A path as the conflict lines print it, without the leading dot.
fn label(path: &str) -> &str {
    path.strip_prefix('.').unwrap_or(path)
}

/// Whether every element is an object with a string `id`, all different. An empty
/// list qualifies, so a list one side emptied still merges by record.
fn keyed(list: &[Value]) -> bool {
    let mut seen = std::collections::HashSet::new();
    list.iter().all(|v| {
        v.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| seen.insert(id.to_string()))
    })
}

/// Whether every element is a string, all different.
fn distinct_strings(list: &[Value]) -> bool {
    let mut seen = std::collections::HashSet::new();
    list.iter()
        .all(|v| v.as_str().is_some_and(|s| seen.insert(s.to_string())))
}

fn id_of(v: &Value) -> String {
    v.get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Lists of records merged by id: the ids in [`merge_order`]'s order, each
/// record [`merge3`]'d, a record merged away dropped.
fn merge_records(
    o: &[Value],
    a: &[Value],
    b: &[Value],
    path: &str,
    conflicts: &mut Vec<String>,
) -> Vec<Value> {
    let ids = |l: &[Value]| l.iter().map(id_of).collect::<Vec<_>>();
    let find =
        |l: &'_ [Value], id: &str| -> Option<Value> { l.iter().find(|v| id_of(v) == id).cloned() };
    let (oi, ai, bi) = (ids(o), ids(a), ids(b));
    // Every id any side holds, so a record removed by one side and changed by
    // the other is found: `merge_order` leaves such an id out.
    let (mut order, clash) = merge_order(&oi, &ai, &bi);
    if clash {
        conflicts.push(order_conflict_line(path));
    }
    for id in ai.iter().chain(&bi) {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|id| {
            let here = format!("{path}[{id}]");
            merge3(
                find(o, &id).as_ref(),
                find(a, &id).as_ref(),
                find(b, &id).as_ref(),
                &here,
                conflicts,
            )
        })
        .collect()
}

/// The ordered merge of three lists of distinct ids, and whether both sides
/// reordered the ids all three hold, differently — ours' order is then kept.
///
/// The order comes from the side that reordered, ours when neither or both did;
/// the other side's removals are taken out of it and its additions put in
/// (`rebase`).
pub fn merge_order(o: &[String], a: &[String], b: &[String]) -> (Vec<String>, bool) {
    if a == o {
        return (b.to_vec(), false);
    }
    if b == o || a == b {
        return (a.to_vec(), false);
    }
    let shared = |l: &[String]| -> Vec<String> {
        l.iter()
            .filter(|id| o.contains(id) && a.contains(id) && b.contains(id))
            .cloned()
            .collect()
    };
    let (so, sa, sb) = (shared(o), shared(a), shared(b));
    if sb != so && sa == so {
        (rebase(o, b, a), false)
    } else {
        (rebase(o, a, b), sb != so && sa != sb)
    }
}

/// `lead`'s order less what `other` removed from `o`, with what `other` added
/// put after the id it follows in `other` — first when nothing does, last when
/// only additions follow it.
fn rebase(o: &[String], lead: &[String], other: &[String]) -> Vec<String> {
    let mut out: Vec<String> = lead
        .iter()
        .filter(|id| !(o.contains(id) && !other.contains(id)))
        .cloned()
        .collect();
    for (k, id) in other.iter().enumerate() {
        if o.contains(id) || out.contains(id) {
            continue;
        }
        let at = if other[k + 1..].iter().all(|next| !o.contains(next)) {
            out.len()
        } else {
            other[..k]
                .iter()
                .rev()
                .find_map(|prev| out.iter().position(|x| x == prev))
                .map_or(0, |p| p + 1)
        };
        out.insert(at, id.clone());
    }
    out
}

/// Put back every image record `doc` uses that the merge dropped, from the first
/// of `sides` holding it, and the same inside a kit's published document.
///
/// One side removing a picture it stopped using merges its record away, while
/// the other side may have put that picture on a layer; the loader admits a
/// reference to nothing (a picture to relink), so the bytes would go quietly.
/// A reference is any string in the document equal to the image's id: ids are
/// content hashes, so nothing else collides with one.
fn keep_used_images(doc: &mut Value, sides: [Option<&Value>; 3]) {
    if let Some(inner) = doc.get_mut("published").and_then(|p| p.get_mut("document")) {
        let inner_sides = sides.map(|s| {
            s.and_then(|s| s.get("published"))
                .and_then(|p| p.get("document"))
        });
        keep_used_images(inner, inner_sides);
    }
    let Some(map) = doc.as_object_mut() else {
        return;
    };
    let held: HashSet<String> = map
        .get("images")
        .and_then(Value::as_array)
        .map(|l| l.iter().map(id_of).collect())
        .unwrap_or_default();
    let mut dropped: Vec<&Value> = Vec::new();
    for side in sides.into_iter().flatten() {
        for record in side
            .get("images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = id_of(record);
            if !held.contains(&id) && !dropped.iter().any(|r| id_of(r) == id) {
                dropped.push(record);
            }
        }
    }
    if dropped.is_empty() {
        return;
    }
    let mut used = HashSet::new();
    for (k, v) in map.iter() {
        if k != "images" && k != "published" {
            strings_in(v, &mut used);
        }
    }
    let kept: Vec<Value> = dropped
        .into_iter()
        .filter(|r| used.contains(id_of(r).as_str()))
        .cloned()
        .collect();
    if kept.is_empty() {
        return;
    }
    let images = map
        .entry("images")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(list) = images.as_array_mut() {
        list.extend(kept);
    }
}

/// Every string in `v`, at any depth.
fn strings_in<'v>(v: &'v Value, out: &mut HashSet<&'v str>) {
    match v {
        Value::String(s) => {
            out.insert(s);
        }
        Value::Array(l) => l.iter().for_each(|x| strings_in(x, out)),
        Value::Object(m) => m.values().for_each(|x| strings_in(x, out)),
        _ => {}
    }
}
