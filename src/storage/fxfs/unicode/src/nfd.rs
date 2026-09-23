// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Normalized decomposition involves:
//!   1. Recursively applying character decompositions.
//!   1. Special-case Hangul decomposition.
//!   2. Reordering via the 'canonical ordering algorithm'.
//!
//! We expose a simple character-by-charcter iterator interface for NFD.
use crate::lookup::{self, ccc};
use smallvec::SmallVec;
use std::ops::Range;

/// Hangul and CJK have assigned ranges but no per-character data within those ranges but
/// only Hangul needs algorithmic decomposition.
const HANGUL_RANGE: Range<char> = '\u{ac00}'..'\u{d7a4}';

/// Decomposition of Hangul syllables as according to the unicode 15.0 standard.
fn hangul_decomposition(s: char, out: &mut SmallVec<[char; 16]>) {
    const SBASE: u32 = 0xac00;
    const LBASE: u32 = 0x1100;
    const VBASE: u32 = 0x1161;
    const TBASE: u32 = 0x11a7;
    const TCOUNT: u32 = 28;
    const NCOUNT: u32 = 588;
    let s_index = s as u32 - SBASE;
    out.push(unsafe {
        // Safety: Known unicode codepoint
        char::from_u32_unchecked(LBASE + s_index / NCOUNT)
    });
    out.push(unsafe {
        // Safety: Known unicode codepoint
        char::from_u32_unchecked(VBASE + (s_index % NCOUNT) / TCOUNT)
    });
    if s_index % TCOUNT != 0 {
        out.push(unsafe {
            // Safety: Known unicode codepoint
            char::from_u32_unchecked(TBASE + s_index % TCOUNT)
        });
    }
}

/// Perform decomposition (Hangul, then lookup), appending result to 'out'
fn decompose(ch: char, out: &mut SmallVec<[char; 16]>) {
    if HANGUL_RANGE.contains(&ch) {
        hangul_decomposition(ch, out);
    } else {
        if let Some(mapping) = lookup::decomposition(ch) {
            out.extend(mapping.chars());
        } else {
            out.push(ch);
        }
    }
}

/// Filters a valid sequence of unicode characters, outputting an normalized (NFD) sequence.
pub struct NfdIterator<I: Iterator<Item = char>> {
    /// The not-yet-normalized input sequence.
    input: std::iter::Peekable<I>,
    /// A working area used to normalize characters prior to emitting them.
    buf: SmallVec<[char; 16]>,
    /// Tracks the cursor position in 'buf'.
    pos: usize,
}

impl<I: Iterator<Item = char>> std::iter::FusedIterator for NfdIterator<I> {}

impl<I: Iterator<Item = char>> Iterator for NfdIterator<I> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if self.pos < self.buf.len() {
            let ch = self.buf[self.pos];
            self.pos += 1;
            return Some(ch);
        }
        self.buf.clear();
        self.pos = 0;

        // Printable entities start with a ccc=0 and their decompositions start with ccc=0
        if let Some(ch) = self.input.next_if(|&x| ccc(x) == 0) {
            decompose(ch, &mut self.buf);
        }
        // Fill buffer with any ccc>0 (modifiers) and then sort it.
        while let Some(ch) = self.input.next_if(|&x| ccc(x) != 0) {
            decompose(ch, &mut self.buf);
        }
        // Rust's sort is stable so we can rely on it not to reorder the ccc=0 elements.
        self.buf.sort_by(|a, b| ccc(*a).cmp(&ccc(*b)));

        if self.pos < self.buf.len() {
            let ch = self.buf[self.pos];
            self.pos += 1;
            Some(ch)
        } else {
            None
        }
    }
}

pub fn nfd<I: Iterator<Item = char>>(input: I) -> NfdIterator<I> {
    NfdIterator { input: input.peekable(), buf: SmallVec::new(), pos: 0 }
}

#[cfg(test)]
mod test {
    use super::nfd;
    use regex::Regex;
    use std::io::Read;
    use zip::read::ZipArchive;

    pub fn read_zip(path: &str) -> Result<String, std::io::Error> {
        const ZIP_PATH: &'static str = std::env!("UCD_ZIP");
        let mut zip = ZipArchive::new(std::fs::File::open(ZIP_PATH).unwrap()).unwrap();
        let mut file = zip.by_name(path).map_err(|_| std::io::ErrorKind::NotFound)?;
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        Ok(String::from_utf8(out).unwrap())
    }

    /// Test suite of codepoint to normalized forms.
    /// Value contains different types of normalization (original, nfc, nfd, nfkc, nfkd)
    pub fn normalization_test()
    -> Result<Vec<(Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>)>, std::io::Error> {
        // We use the UCD provided test data for validating our implementation.
        let data = read_zip("NormalizationTest.txt")?;

        let mut tests = Vec::new();
        let re =
            Regex::new(r"([A-F0-9 ]+);([A-F0-9 ]+);([A-F0-9 ]+);([A-F0-9 ]+);([A-F0-9 ]+); #.*$")
                .unwrap();
        for line in data.split("\n") {
            if line.len() == 0 {
                continue;
            }
            if let Some(cap) = re.captures(&line) {
                let source: Vec<u32> =
                    cap[1].split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
                let nfc: Vec<u32> =
                    cap[2].split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
                let nfd: Vec<u32> =
                    cap[3].split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
                let nfkc: Vec<u32> =
                    cap[4].split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
                let nfkd: Vec<u32> =
                    cap[5].split(" ").map(|x| u32::from_str_radix(x, 16).unwrap()).collect();
                tests.push((source, nfc, nfd, nfkc, nfkd));
            }
        }
        Ok(tests)
    }

    #[test]
    fn test_decomposition_of_non_starters() {
        // '\u{0340}' decomposes to '\u{0300}'. Neither are ccc=0.
        // This tests that we don't make the assumption that decompositions are ccc=0.
        assert_eq!(nfd("\u{0360}\u{0340}a".chars()).collect::<String>(), "\u{0300}\u{0360}a");

        assert_eq!(nfd("\u{09CB}\u{0300}".chars()).collect::<String>(), "\u{09c7}\u{09be}\u{0300}");
    }

    #[test]
    fn test_nfd() {
        for (line, testdata) in normalization_test().unwrap().into_iter().enumerate() {
            let source: String = testdata.0.iter().map(|x| char::from_u32(*x).unwrap()).collect();
            let expected: String = testdata.2.iter().map(|x| char::from_u32(*x).unwrap()).collect();
            assert_eq!(
                nfd(source.chars()).collect::<String>(),
                expected,
                "Failute in case {line} of NormalizationTest.txt: {:#x} {testdata:?}..",
                testdata.0[0],
            );
        }
    }
}
