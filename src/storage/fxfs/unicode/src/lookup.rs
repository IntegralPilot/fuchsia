// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A set of functions for parsing generated unicode data (see generator.rs and unicode_gen.rs)
use crate::unicode_gen::*;
use std::cmp::Ordering;
use types::MappingOffset;

/// Lookup the CCC for a code point.
/// This aims to minimise data set size whilst maintaining performance.
/// Worst case is O(log2(2^16)) = O(16).
pub fn ccc(ch: char) -> u8 {
    // CCC data is encoded in 'planes' using LUT for top two bytes of u32 unicode char.
    let plane_offset = CCC_PLANE_OFFSETS[ch as u32 as usize >> 16];
    let plane_end = CCC_PLANE_OFFSETS[((ch as u32 >> 16) + 1) as usize];
    let plane = &CCC_PLANE_DATA[plane_offset..plane_end];
    let ch_lsb = ch as u32 as u16;
    let ix = plane
        .binary_search_by(|entry| match entry.low_plane_end_ix.cmp(&ch_lsb) {
            Ordering::Equal => Ordering::Less,
            x => x,
        })
        .unwrap_or_else(|x| x);
    if ix < plane.len() {
        let entry = &plane[ix];
        if entry.range().contains(&ch_lsb) {
            return entry.ccc;
        }
    }
    0
}

/// Both decomposition and casefold use the same lookup mechanism
fn string_table_lookup(
    ch: char,
    plane_offsets: &[usize],
    plane_data: &[MappingOffset],
    string_data: &'static [u8],
) -> Option<&'static str> {
    // Unicode has 17 planes (0..=16). we store one extra entry to calculate plane_end easily.
    let plane_start = plane_offsets[(ch as u32 >> 16) as usize];
    let plane_end = plane_offsets[((ch as u32 >> 16) + 1) as usize];
    let plane = &plane_data[plane_start..plane_end];
    // Look up the lower 16-bit in the plane using binary search.
    let ch_lsb = ch as u32 as u16;
    if let Ok(ix) = plane.binary_search_by(|entry| entry.low_plane_ix.cmp(&ch_lsb)) {
        // Read the start of the string and look at the next entry to find the end of the string.
        let offset = plane[ix].offset as usize;
        let end = if plane.len() == ix + 1 {
            if plane_end < plane_data.len() {
                plane_data[plane_end].offset as usize
            } else {
                string_data.len()
            }
        } else {
            plane[ix + 1].offset as usize
        };
        let out = &string_data[offset..end];
        return Some(unsafe {
            // Safety: This comes from official unicode decompositions.
            std::str::from_utf8_unchecked(out)
        });
    }
    return None;
}

/// Returns the unicode decomposition of a given codepoint.
pub fn decomposition(ch: char) -> Option<&'static str> {
    string_table_lookup(ch, DECOMP_PLANE_OFFSETS, DECOMP_PLANE_DATA, DECOMP_STRINGS)
}

/// Returns true if a codepoint is a default ignorable codepoint.
pub fn default_ignorable(ch: char) -> bool {
    // Unicode has 17 planes (0..=16). we store one extra entry to calculate plane_end easily.
    let plane_offset = IGNORABLE_PLANE_OFFSETS[ch as u32 as usize >> 16];
    let plane_end = IGNORABLE_PLANE_OFFSETS[((ch as u32 >> 16) + 1) as usize];
    let plane = &IGNORABLE_PLANE_DATA[plane_offset..plane_end];
    let ch_lsb = ch as u32 as u16;
    let ix = plane
        .binary_search_by(|entry| match entry.end.cmp(&ch_lsb) {
            Ordering::Equal => Ordering::Less,
            x => x,
        })
        .unwrap_or_else(|x| x);
    if ix < plane.len() { plane[ix].contains(&ch_lsb) } else { false }
}

pub fn casefold(ch: char) -> Option<&'static str> {
    string_table_lookup(ch, CASEFOLD_PLANE_OFFSETS, CASEFOLD_PLANE_DATA, CASEFOLD_STRINGS)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_ccc() {
        assert_eq!(ccc('\u{0}'), 0);
        assert_eq!(ccc('A'), 0);
        assert_eq!(ccc('\u{059a}'), 222);
        assert_eq!(ccc('\u{2cee}'), 0);
        assert_eq!(ccc('\u{2cef}'), 230);
        assert_eq!(ccc('\u{2cf0}'), 230);
        assert_eq!(ccc('\u{2cf1}'), 230);
        assert_eq!(ccc('\u{2cf2}'), 0);
    }

    #[test]
    fn test_decomposition() {
        assert_eq!(decomposition('\u{f900}'), Some("\u{8c48}"));
        assert_eq!(decomposition('A'), None);
        assert_eq!(decomposition('\u{1f82}'), Some("α\u{313}\u{300}\u{345}"));
    }

    #[test]
    fn test_deafault_ignorable() {
        assert!(!default_ignorable('\u{00ac}'));
        assert!(default_ignorable('\u{00ad}'));
        assert!(!default_ignorable('\u{00ae}'));
        assert!(!default_ignorable('\u{115e}'));
        assert!(default_ignorable('\u{115f}'));
        assert!(default_ignorable('\u{1160}'));
        assert!(!default_ignorable('\u{1161}'));
    }

    #[test]
    fn test_casefold() {
        assert_eq!(casefold('A'), Some("a"));
        assert_eq!(casefold('a'), None);
    }

    #[test]
    fn test_last_char() {
        // We look up strings end by incrementing offset + 1 from string start so we need make sure
        // we handle the case where offset is the last string in the plane or the last string
        // overall.
        assert_eq!(decomposition('\u{10000}'), None);
        assert_eq!(decomposition('\u{1fbca}'), None);
        assert_eq!(decomposition('\u{1fbf9}'), None);
        assert_eq!(decomposition('\u{20000}'), None);
        assert_eq!(decomposition('\u{2fa1d}'), Some("\u{2a600}"));
        assert_eq!(casefold('\u{ff39}'), Some("\u{ff59}"));
        assert_eq!(casefold('\u{ff3a}'), Some("\u{ff5a}"));
        assert_eq!(casefold('\u{ff3b}'), None);
        assert_eq!(casefold('\u{10400}'), Some("\u{10428}"));
        assert_eq!(casefold('\u{1e920}'), Some("\u{1e942}"));
        assert_eq!(casefold('\u{1e921}'), Some("\u{1e943}"));
        assert_eq!(casefold('\u{1e922}'), None);
    }

    #[test]
    fn test_high_char() {
        // This is the maximum unicode codepoint.
        assert_eq!(decomposition('\u{10ffff}'), None);
        assert_eq!(casefold('\u{10ffff}'), None);
        assert_eq!(default_ignorable('\u{10ffff}'), false);
        // ensure ccc doesn't panic if called with high char (top-most plane)
        let _ = ccc('\u{10ffff}');
    }
}
