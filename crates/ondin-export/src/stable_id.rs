//! Ids for another tool's file, derived from what they name, so the same
//! document writes the same file.

/// A UUID derived from `parts`, stable across runs: two FNV-1a passes, marked
/// as version 8.
pub(crate) fn uuid(parts: &[&str]) -> String {
    let hash = |seed: u64| {
        let mut h = seed;
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                h ^= 0x1f;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            for byte in part.bytes() {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        h
    };
    let (hi, lo) = (hash(0xcbf2_9ce4_8422_2325), hash(0x8422_2325_cbf2_9ce4));
    let hi = (hi & !0xf000) | 0x8000;
    let lo = (lo & !0xc000_0000_0000_0000) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0xffff,
        lo >> 48,
        lo & 0xffff_ffff_ffff
    )
}
