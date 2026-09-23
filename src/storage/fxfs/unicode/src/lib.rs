// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fprint::TypeFingerprint;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

mod lookup;
mod nfd;
use unicode_gen;

/// Filters a valid sequence of unicode characters, casefolding.
pub(crate) struct CaseFoldIterator<I: Iterator<Item = char>> {
    /// The not-yet-normalized input sequence.
    input: I,
    buf: [char; 4],
    buf_len: usize,
    buf_pos: usize,
}

impl<I: Iterator<Item = char>> Iterator for CaseFoldIterator<I> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if self.buf_pos < self.buf_len {
            let ch = self.buf[self.buf_pos];
            self.buf_pos += 1;
            return Some(ch);
        }
        self.buf_len = 0;
        self.buf_pos = 0;

        self.input.next().map(|ch| {
            if let Some(mapping) = crate::lookup::casefold(ch) {
                let mut chars = mapping.chars();
                let first = chars.next().unwrap();
                for c in chars {
                    self.buf[self.buf_len] = c;
                    self.buf_len += 1;
                }
                first
            } else {
                ch
            }
        })
    }
}

pub(crate) fn casefold<I: Iterator<Item = char>>(input: I) -> CaseFoldIterator<I> {
    CaseFoldIterator { input, buf: ['\0'; 4], buf_len: 0, buf_pos: 0 }
}

/// Helper function to convert a `char` to an iterator over its UTF-8 bytes
/// without any heap allocations.
pub fn utf8_bytes(c: char) -> impl Iterator<Item = u8> {
    let mut buf = [0; 4];
    let len = c.encode_utf8(&mut buf).len();
    buf.into_iter().take(len)
}

/// A comparison function that:
///  * Applies casefolding.
///  * Removes default ignorable characters.
///  * Applies nfd normalization.
/// That function will early-out at the first non-matching character.
pub fn casefold_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let a_it = nfd::nfd(casefold(a.chars()).filter(|x| !lookup::default_ignorable(*x)));
    let b_it = nfd::nfd(casefold(b.chars()).filter(|x| !lookup::default_ignorable(*x)));
    a_it.cmp(b_it)
}

// A simple wrapper around String that provides casefolding comparison.
#[derive(arbitrary::Arbitrary, Clone, Eq, Serialize, Deserialize, TypeFingerprint)]
pub struct CasefoldString(String);
impl CasefoldString {
    pub fn new(s: String) -> Self {
        Self(s)
    }
}
impl std::fmt::Debug for CasefoldString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "CasefoldString(\"{}\")", self.0)
    }
}
impl std::fmt::Display for CasefoldString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self.0)
    }
}
/// A borrowed slice of a casefolded string.
///
/// It is designed to be the borrowed counterpart to `CasefoldString`, mirroring
/// the relationship between `str` and `String`.
///
/// We wrap `str` instead of a struct like `CasefoldStr<'a>(&'a str)`
/// so that `CasefoldString` can implement `Deref<Target = CasefoldStr>`. This allows
/// `CasefoldString` to coerce to `&CasefoldStr` automatically, and enables zero-allocation
/// map lookups via `Borrow<CasefoldStr>`.
#[repr(transparent)]
pub struct CasefoldStr(str);

impl CasefoldStr {
    pub fn new(s: &str) -> &Self {
        // SAFETY: `CasefoldStr` is `#[repr(transparent)]` around `str`, so it has the same layout
        // and alignment.
        unsafe { &*(s as *const str as *const CasefoldStr) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn casefold_normalized_chars(&self) -> impl Iterator<Item = char> + '_ {
        nfd::nfd(casefold(self.0.chars()).filter(|x| !lookup::default_ignorable(*x)))
    }
}

impl std::fmt::Debug for CasefoldStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "CasefoldStr(\"{}\")", &self.0)
    }
}

impl std::fmt::Display for CasefoldStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}", &self.0)
    }
}

impl std::cmp::PartialEq for CasefoldStr {
    fn eq(&self, rhs: &Self) -> bool {
        casefold_cmp(self.as_str(), rhs.as_str()).is_eq()
    }
}
impl std::cmp::Eq for CasefoldStr {}

impl std::cmp::Ord for CasefoldStr {
    fn cmp(&self, rhs: &Self) -> std::cmp::Ordering {
        casefold_cmp(self.as_str(), rhs.as_str())
    }
}
impl std::cmp::PartialOrd for CasefoldStr {
    fn partial_cmp(&self, rhs: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(rhs))
    }
}

impl ToOwned for CasefoldStr {
    type Owned = CasefoldString;
    fn to_owned(&self) -> Self::Owned {
        CasefoldString::new(self.as_str().to_owned())
    }
}

impl Hash for CasefoldStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for ch in self.casefold_normalized_chars() {
            ch.hash(state);
        }
    }
}

impl CasefoldString {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for CasefoldString {
    type Target = CasefoldStr;
    fn deref(&self) -> &CasefoldStr {
        CasefoldStr::new(&self.0)
    }
}

impl std::borrow::Borrow<CasefoldStr> for CasefoldString {
    fn borrow(&self) -> &CasefoldStr {
        &**self
    }
}

impl std::cmp::PartialEq for CasefoldString {
    fn eq(&self, rhs: &Self) -> bool {
        **self == **rhs
    }
}

impl std::cmp::Ord for CasefoldString {
    fn cmp(&self, rhs: &Self) -> std::cmp::Ordering {
        (**self).cmp(&**rhs)
    }
}

impl std::cmp::PartialOrd for CasefoldString {
    fn partial_cmp(&self, rhs: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(rhs))
    }
}

// Nb: This trait is provided for completeness but is NOT intended to be performant.
impl Hash for CasefoldString {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        (**self).hash(state);
    }
}

impl From<&str> for CasefoldString {
    fn from(item: &str) -> Self {
        CasefoldString(item.into())
    }
}

impl<'a> From<&'a str> for &'a CasefoldStr {
    fn from(item: &'a str) -> Self {
        CasefoldStr::new(item)
    }
}

impl<'a> std::convert::TryFrom<&'a [u8]> for &'a CasefoldStr {
    type Error = std::str::Utf8Error;

    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        std::str::from_utf8(bytes).map(CasefoldStr::new)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::hash::{Hash, Hasher};

    fn get_hash<T: Hash + ?Sized>(t: &T) -> u64 {
        let mut s = std::collections::hash_map::DefaultHasher::new();
        t.hash(&mut s);
        s.finish()
    }

    #[test]
    fn test_casefold() {
        assert_eq!(casefold("Hello There".chars()).collect::<String>(), "hello there");
        assert_eq!(casefold("HELLO There".chars()).collect::<String>(), "hello there");
    }

    #[test]
    fn test_casefold_cmp() {
        assert_eq!(casefold_cmp("Hello", "hello"), std::cmp::Ordering::Equal);
        assert_eq!(casefold_cmp("Hello There", "hello"), std::cmp::Ordering::Greater);
        assert_eq!(casefold_cmp("hello there", "hello"), std::cmp::Ordering::Greater);
        assert_eq!(casefold_cmp("hello\u{00AD}", "hello"), std::cmp::Ordering::Equal);
        assert_eq!(casefold_cmp("\u{03AA}", "\u{0399}\u{0308}"), std::cmp::Ordering::Equal);

        // Gracefully handle the degenerate case where we start with modifiers
        assert_eq!(
            casefold_cmp("\u{308}\u{05ae}Hello", "\u{05ae}\u{308}hello"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_casefoldstring() {
        let a = CasefoldString::new("Hello There".to_owned());
        let b = CasefoldString::new("hello there".to_owned());
        let c = CasefoldString::new("hello".to_owned());
        let d = CasefoldString::new("\u{03AA}".to_owned());
        let e = CasefoldString::new("\u{0399}\u{0308}".to_owned());
        // Check some comparisons.
        assert_eq!(a, a);
        assert_eq!(a, b);
        assert_eq!(d, e);
        assert!(a > c);
        assert!(b > c);
        assert_eq!(a.0, format!("{}", a));
        // Debug::fmt should show type to avoid confusion.
        assert_eq!("CasefoldString(\"Hello There\")", format!("{:?}", a));
        // Displays the same as String.
        assert_eq!(format!("{}", "Hello There".to_owned()), format!("{}", a));
    }

    #[test]
    fn test_casefold_hash_equality() {
        // hello and hello with soft hyphen (ignorable) should be equal and have identical hashes
        let a = CasefoldString::new("hello\u{00AD}".to_owned());
        let b = CasefoldString::new("hello".to_owned());
        assert_eq!(a, b);
        assert_eq!(get_hash(&a), get_hash(&b));
    }

    #[test]
    fn test_casefoldstr() {
        use std::borrow::Borrow;

        let a = CasefoldString::new("Hello There".to_owned());
        let b = CasefoldStr::new("hello there");
        let c = CasefoldStr::new("hello");

        // Compare CasefoldString with CasefoldStr (via Borrow)
        let borrowed_a: &CasefoldStr = a.borrow();
        assert_eq!(borrowed_a, b);
        assert!(borrowed_a > c);

        // Compare CasefoldStr with CasefoldStr
        assert_eq!(b, b);
        assert_eq!(CasefoldStr::new("Hello"), CasefoldStr::new("hello"));
        assert!(b > c);

        // Verify as_str() returns the original case-sensitive string
        assert_eq!(b.as_str(), "hello there");
        assert_ne!(b.as_str(), "Hello There");

        // Test ToOwned
        let owned_b: CasefoldString = b.to_owned();
        assert_eq!(owned_b, a);

        // Test Hash consistency
        assert_eq!(get_hash(&a), get_hash(borrowed_a));
        assert_eq!(get_hash(&owned_b), get_hash(b));
    }

    #[test]
    fn test_casefold_str_normalized() {
        // soft hyphen (ignorable), ohm sign (decomposes to omega)
        let raw = "Hello\u{00AD} World\u{2126}";
        let cf_str = CasefoldStr::new(raw);

        let normalized_chars: String = cf_str.casefold_normalized_chars().collect();

        // expected:
        // "Hello" -> "hello"
        // "\u{00AD}" -> stripped
        // " " -> " "
        // "World" -> "world"
        // "\u{2126}" -> \u{03c9} (omega lowercase in NFD)
        let expected_str = "hello world\u{03c9}";
        assert_eq!(normalized_chars, expected_str);
    }
}
