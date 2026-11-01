//! A copy of a font without its monochrome bitmap glyphs, for the GPU backend.
//!
//! vello draws every glyph of a font holding any bitmap strike through its emoji
//! path, taking the strike's bitmap wherever one exists at the size. The ClearType
//! faces Windows ships (Calibri, Cambria, Consolas and the like) carry small
//! black-and-white strikes (`EBLC`/`EBDT`) for screen sizes, which that path draws
//! blank, so most of their letters vanish from the canvas. Without those tables
//! the font has no strike and every glyph is drawn from its outline, as the CPU
//! backend draws it. Colour bitmaps (`CBDT`, `sbix`) are emoji and are left alone.

use ondin_core::peniko::{Blob, FontData};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// The monochrome bitmap tables: the strikes, their locations and their scaling.
const DROPPED: [&[u8; 4]; 3] = [b"EBDT", b"EBLC", b"EBSC"];

/// Tables that make a strike an emoji's, which vello draws as it should.
const COLOUR: [&[u8; 4]; 3] = [b"CBDT", b"CBLC", b"sbix"];

/// How many fonts the answer is kept for. Past it the cache starts over: the
/// copies are as large as the fonts, and a font the picker let go of stays here
/// otherwise.
const KEPT: usize = 256;

/// The answer per blob id and face index.
type Answers = HashMap<(u64, u32), Option<FontData>>;

/// Each font's answer. The copy is kept, not made per frame, so its blob id is
/// stable and vello's glyph cache holds across frames.
static CACHE: LazyLock<Mutex<Answers>> = LazyLock::new(Default::default);

/// `font` without its monochrome bitmap strikes, or `None` when it has none (or
/// has colour ones), so the caller draws `font` itself.
pub(crate) fn outline_only(font: &FontData) -> Option<FontData> {
    let key = (font.data.id(), font.index);
    let Ok(mut cache) = CACHE.lock() else {
        return None;
    };
    if let Some(answer) = cache.get(&key) {
        return answer.clone();
    }
    let answer =
        strip(font.data.as_ref(), font.index).map(|bytes| FontData::new(Blob::from(bytes), 0));
    if cache.len() >= KEPT {
        cache.clear();
    }
    cache.insert(key, answer.clone());
    answer
}

/// One table directory entry: tag, offset and length in the file.
type Record = ([u8; 4], usize, usize);

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The face's sfnt version and table records. A collection (`ttcf`) holds a
/// directory per face, its offsets into the whole file.
fn directory(data: &[u8], index: u32) -> Option<(u32, Vec<Record>)> {
    let dir = if data.get(..4)? == b"ttcf" {
        if index >= u32_at(data, 8)? {
            return None;
        }
        u32_at(data, 12 + 4 * index as usize)? as usize
    } else {
        0
    };
    let version = u32_at(data, dir)?;
    let count = u16_at(data, dir + 4)? as usize;
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let at = dir + 12 + 16 * i;
        let tag: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        let offset = u32_at(data, at + 8)? as usize;
        let length = u32_at(data, at + 12)? as usize;
        data.get(offset..offset.checked_add(length)?)?;
        records.push((tag, offset, length));
    }
    Some((version, records))
}

/// The face at `index` written out alone without [`DROPPED`], or `None` when it
/// has no such table, has a [`COLOUR`] one, or does not parse.
fn strip(data: &[u8], index: u32) -> Option<Vec<u8>> {
    let (version, records) = directory(data, index)?;
    let has = |tags: &[&[u8; 4]]| records.iter().any(|(t, ..)| tags.contains(&t));
    if !has(&DROPPED) || has(&COLOUR) {
        return None;
    }
    let mut kept: Vec<Record> = records
        .into_iter()
        .filter(|(t, ..)| !DROPPED.contains(&t))
        .collect();
    // A directory is sorted by tag.
    kept.sort_by_key(|(t, ..)| *t);

    let count = kept.len();
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&(count as u16).to_be_bytes());
    // The binary-search hints: the largest power of two not above the count.
    let power = if count == 0 {
        0
    } else {
        1usize << (usize::BITS - 1 - count.leading_zeros())
    };
    let range = (power * 16) as u16;
    out.extend_from_slice(&range.to_be_bytes());
    out.extend_from_slice(&(power.max(1).trailing_zeros() as u16).to_be_bytes());
    out.extend_from_slice(&((count * 16) as u16).wrapping_sub(range).to_be_bytes());

    let mut at = 12 + 16 * count;
    let mut body = Vec::new();
    for (tag, offset, length) in &kept {
        let table = &data[*offset..offset + length];
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(table).to_be_bytes());
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(*length as u32).to_be_bytes());
        body.extend_from_slice(table);
        // Each table starts on a four-byte boundary.
        let pad = (4 - length % 4) % 4;
        body.extend(std::iter::repeat_n(0, pad));
        at += length + pad;
    }
    out.extend_from_slice(&body);
    Some(out)
}

/// The sum of a table's big-endian words, the last one zero-padded.
fn checksum(table: &[u8]) -> u32 {
    table.chunks(4).fold(0u32, |sum, word| {
        let mut w = [0u8; 4];
        w[..word.len()].copy_from_slice(word);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal sfnt with these tables, each its tag's bytes repeated.
    fn sfnt(tags: &[&[u8; 4]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        out.extend_from_slice(&(tags.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        let mut at = 12 + 16 * tags.len();
        for tag in tags {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&6u32.to_be_bytes());
            at += 6;
        }
        for tag in tags {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&tag[..2]);
        }
        out
    }

    fn tags_of(data: &[u8]) -> Vec<[u8; 4]> {
        directory(data, 0)
            .unwrap()
            .1
            .iter()
            .map(|(t, ..)| *t)
            .collect()
    }

    /// The monochrome strike tables go, every other table stays with its bytes,
    /// and the copy is a directory a reader can walk.
    #[test]
    fn the_monochrome_strikes_are_dropped_and_the_rest_kept() {
        let font = sfnt(&[b"glyf", b"EBDT", b"head", b"EBLC", b"loca"]);
        let out = strip(&font, 0).expect("a strike to drop");
        assert_eq!(tags_of(&out), [*b"glyf", *b"head", *b"loca"]);
        let (_, records) = directory(&out, 0).unwrap();
        for (tag, offset, length) in records {
            assert_eq!(&out[offset..offset + 4], &tag, "{tag:?}'s own bytes");
            assert_eq!(length, 6);
            assert_eq!(offset % 4, 0, "{tag:?} on a four-byte boundary");
        }
    }

    /// A font with no strike, or with colour ones, is left to draw as it is.
    #[test]
    fn a_font_without_monochrome_strikes_is_left_alone() {
        assert!(strip(&sfnt(&[b"glyf", b"head"]), 0).is_none());
        assert!(strip(&sfnt(&[b"CBDT", b"CBLC", b"EBLC", b"glyf"]), 0).is_none());
        assert!(strip(b"not a font", 0).is_none());
    }

    /// A face of a collection is found by its index and written out alone.
    #[test]
    fn a_collections_face_is_written_out_alone() {
        let face = sfnt(&[b"EBLC", b"glyf"]);
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        ttc.extend_from_slice(&1u32.to_be_bytes());
        ttc.extend_from_slice(&16u32.to_be_bytes());
        // The face's offsets are into the whole file, so shift them by 16.
        let mut shifted = face.clone();
        for i in 0..2 {
            let at = 12 + 16 * i + 8;
            let offset = u32_at(&face, at).unwrap() + 16;
            shifted[at..at + 4].copy_from_slice(&offset.to_be_bytes());
        }
        ttc.extend_from_slice(&shifted);
        let out = strip(&ttc, 0).expect("the face");
        assert_eq!(tags_of(&out), [*b"glyf"]);
        assert!(strip(&ttc, 1).is_none(), "no such face");
    }
}
