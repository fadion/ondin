//! Three-way merge of a document, for `ondin merge` and git's merge driver.
//!
//! The merge reads the three saved forms (each migrated to the current schema) as
//! JSON and merges them value by value:
//!
//! - A value one side changed takes that side; both sides changed alike, either.
//! - An object merges key by key. An object of one key whose key differs between
//!   the sides (an enum's variant, a layer's kind) is one value.
//! - A list of records that carry an `id` (layers, pages, guides, styles,
//!   variables, fills, effects) merges record by record, matched by id, in the
//!   order [`merge_order`] gives. A list of distinct strings (a layer's children)
//!   merges as an ordered set. Any other list is one value.
//! - A record one side removed and the other left alone is removed; one removed
//!   by one side and changed by the other is kept, as a conflict.
//!
//! Where both sides changed one value differently, ours wins and the place is
//! reported. The result must load as a document; if it does not — a layer moved
//! to two different parents, say — the merge keeps ours whole and says so.

use crate::io::{self, IoError};
use serde_json::{Map, Value};

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
    let (o, a, b) = (read(base)?, read(ours)?, read(theirs)?);
    let mut conflicts = Vec::new();
    let merged = merge3(Some(&o), Some(&a), Some(&b), "", &mut conflicts)
        .expect("a document's root is in all three");
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
            let mut out = Map::new();
            for k in keys {
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
            if [oa, aa, ba].iter().all(|l| keyed(l)) {
                Some(Value::Array(merge_records(oa, aa, ba, path, conflicts)))
            } else if [oa, aa, ba].iter().all(|l| distinct_strings(l)) {
                let ids = |l: &[Value]| -> Vec<String> {
                    l.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                };
                let order = merge_order(&ids(oa), &ids(aa), &ids(ba));
                Some(Value::Array(order.into_iter().map(Value::String).collect()))
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

fn conflict_line(path: &str) -> String {
    format!(
        "{}: changed differently on both sides; ours kept",
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
    let mut order = merge_order(&oi, &ai, &bi);
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

/// The ordered merge of three lists of distinct ids: ours' order, less what
/// theirs removed, with what theirs added put after the id it follows in theirs
/// (or first). When ours left the order alone, theirs' order is taken.
pub fn merge_order(o: &[String], a: &[String], b: &[String]) -> Vec<String> {
    if a == o {
        return b.to_vec();
    }
    if b == o || a == b {
        return a.to_vec();
    }
    let mut out: Vec<String> = a
        .iter()
        .filter(|id| !(o.contains(id) && !b.contains(id)))
        .cloned()
        .collect();
    for (k, id) in b.iter().enumerate() {
        if o.contains(id) || out.contains(id) {
            continue;
        }
        let at = b[..k]
            .iter()
            .rev()
            .find_map(|prev| out.iter().position(|x| x == prev))
            .map_or(0, |p| p + 1);
        out.insert(at, id.clone());
    }
    out
}
