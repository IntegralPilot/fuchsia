// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Data structures and functions relevant to `fuchsia.io` name processing.
//!
//! These names may be used to designate the location of a node as it
//! appears in a directory.
//!
//! These should be aligned with the library comments in sdk/fidl/fuchsia.io/io.fidl.

use fidl_fuchsia_io as fio;
use static_assertions::const_assert_eq;
use std::borrow::Borrow;
use std::fmt::Display;
use std::ops::Deref;
use thiserror::Error;
use zx_status::Status;

mod repr;
use repr::Repr;

/// The maximum length, in bytes, of a single filesystem component.
pub const MAX_NAME_LENGTH: usize = fio::MAX_NAME_LENGTH as usize;
const_assert_eq!(MAX_NAME_LENGTH as u64, fio::MAX_NAME_LENGTH);

/// The type for the name of a node, i.e. a single path component, e.g. `foo`.
///
/// ## Invariants
///
/// A valid node name must meet the following criteria:
///
/// * It cannot be longer than [MAX_NAME_LENGTH].
/// * It cannot be empty.
/// * It cannot be ".." (dot-dot).
/// * It cannot be "." (single dot).
/// * It cannot contain "/".
/// * It cannot contain embedded NUL.
#[derive(Clone)]
pub struct Name(Repr);

const_assert_eq!(std::mem::size_of::<Name>(), 16);
const_assert_eq!(std::mem::align_of::<Name>(), 8);
const_assert_eq!(std::mem::size_of::<Option<Name>>(), 16);
const_assert_eq!(std::mem::size_of::<Result<Name, ParseNameError>>(), 16);

impl Name {
    /// Returns a shared reference to the underlying string slice.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Constructs a `Name` from a static string slice.
    ///
    /// # Panics
    ///
    /// Panics if the name is invalid according to [validate_name].
    pub fn from_static(name: &'static str) -> Self {
        validate_name(name).expect("Invalid name");
        Self(Repr::from_static_str(name))
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Name").field(&self.as_str()).finish()
    }
}

impl Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for Name {
    type Error = ParseNameError;

    fn try_from(value: String) -> Result<Name, ParseNameError> {
        validate_name(&value)?;
        Ok(Self(Repr::from_string(value)))
    }
}

impl TryFrom<&String> for Name {
    type Error = ParseNameError;

    fn try_from(value: &String) -> Result<Name, ParseNameError> {
        Self::try_from(value.as_str())
    }
}

impl<'a> TryFrom<&'a str> for Name {
    type Error = ParseNameError;

    fn try_from(value: &'a str) -> Result<Name, ParseNameError> {
        validate_name(value)?;
        Ok(Self(Repr::from_str(value)))
    }
}

impl Deref for Name {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &*self
    }
}

impl From<Name> for String {
    fn from(value: Name) -> Self {
        value.0.into()
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseNameError {
    #[error("name is too long")]
    TooLong,

    #[error("name cannot be empty")]
    Empty,

    #[error("name cannot be `.`")]
    Dot,

    #[error("name cannot be `..`")]
    DotDot,

    #[error("name cannot contain `/`")]
    Slash,

    #[error("name cannot contain embedded NUL")]
    EmbeddedNul,
}

impl From<ParseNameError> for Status {
    fn from(value: ParseNameError) -> Self {
        match value {
            ParseNameError::TooLong => Status::BAD_PATH,
            _ => Status::INVALID_ARGS,
        }
    }
}

// This lets methods take `name: impl TryInto<Name, Error: Into<ParseNameError>>` as an argument and
// return a `Result` with an error type of either `ParseNameError` or `Status`. If a Name is passed
// to the method, the `try_into` call will return a `Result<Name, Infallible>` and `Infallible`
// needs to be convertible to the error type returned by the method even though it will never
// happen.
impl From<std::convert::Infallible> for ParseNameError {
    fn from(value: std::convert::Infallible) -> Self {
        match value {}
    }
}

/// Validates whether a string slice is a valid node name.
///
/// A valid node name must meet the following criteria:
/// * It cannot be longer than [MAX_NAME_LENGTH] (255 bytes).
/// * It cannot be empty.
/// * It cannot be "." (single dot) or ".." (dot-dot).
/// * It cannot contain "/" (slash) or embedded NUL (`\0`) characters.
pub fn validate_name(name: &str) -> Result<(), ParseNameError> {
    let len = name.len();
    if len > MAX_NAME_LENGTH {
        return Err(ParseNameError::TooLong);
    }
    if len == 0 {
        return Err(ParseNameError::Empty);
    }
    let bytes = name.as_bytes();
    if bytes[0] == b'.' {
        if len == 1 {
            return Err(ParseNameError::Dot);
        }
        if len == 2 && bytes[1] == b'.' {
            return Err(ParseNameError::DotDot);
        }
    }
    if let Some(idx) = memchr::memchr2(b'/', 0, name.as_bytes()) {
        if name.as_bytes()[idx] == 0 {
            return Err(ParseNameError::EmbeddedNul);
        } else {
            return Err(ParseNameError::Slash);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn test_validate_name() {
        assert_matches!(validate_name(&"a".repeat(1000)), Err(ParseNameError::TooLong));
        assert_matches!(
            validate_name(
                std::str::from_utf8(&vec![65; fio::MAX_NAME_LENGTH as usize + 1]).unwrap()
            ),
            Err(ParseNameError::TooLong)
        );
        assert_matches!(
            validate_name(std::str::from_utf8(&vec![65; fio::MAX_NAME_LENGTH as usize]).unwrap()),
            Ok(())
        );
        assert_matches!(validate_name(""), Err(ParseNameError::Empty));
        assert_matches!(validate_name("."), Err(ParseNameError::Dot));
        assert_matches!(validate_name(".."), Err(ParseNameError::DotDot));
        assert_matches!(validate_name(".a"), Ok(()));
        assert_matches!(validate_name("..a"), Ok(()));
        assert_matches!(validate_name("a/b"), Err(ParseNameError::Slash));
        assert_matches!(validate_name("a\0b"), Err(ParseNameError::EmbeddedNul));
        assert_matches!(validate_name("abc"), Ok(()));
    }

    #[test]
    fn test_try_from() {
        assert_matches!(Name::try_from("a".repeat(1000)), Err(ParseNameError::TooLong));
        assert_matches!(Name::try_from("abc".to_string()), Ok(name) if &*name == "abc");
    }

    #[test]
    fn test_into() {
        let name = Name::try_from("a".to_string()).unwrap();
        let name: String = name.into();
        assert_eq!(name, "a".to_string());
    }

    #[test]
    fn test_deref() {
        let name = Name::try_from("a".to_string()).unwrap();
        let name: &str = &name;
        assert_eq!(name, "a");
    }

    #[test]
    fn test_inline() {
        let name = Name::try_from("123456789012345").unwrap();
        assert_eq!(name.as_str(), "123456789012345");

        let name_string = Name::try_from("123456789012345".to_string()).unwrap();
        assert_eq!(name_string.as_str(), "123456789012345");
    }

    #[test]
    fn test_heap() {
        let name = Name::try_from("1234567890123456").unwrap();
        assert_eq!(name.as_str(), "1234567890123456");

        let name_string = Name::try_from("1234567890123456".to_string()).unwrap();
        assert_eq!(name_string.as_str(), "1234567890123456");
    }

    #[test]
    fn test_static_borrow() {
        let name = Name::from_static("static_string");
        assert_eq!(name.as_str(), "static_string");

        let name_large = Name::from_static("a_very_large_static_string_that_exceeds_15_bytes");
        assert_eq!(name_large.as_str(), "a_very_large_static_string_that_exceeds_15_bytes");
    }

    #[test]
    fn test_clone() {
        let inline = Name::try_from("inline").unwrap();
        let inline_clone = inline.clone();
        assert_eq!(inline.as_str(), inline_clone.as_str());

        let heap = Name::try_from("heap_allocated_name_large").unwrap();
        let heap_clone = heap.clone();
        assert_eq!(heap.as_str(), heap_clone.as_str());

        let static_borrow = Name::from_static("static_borrow_large_name");
        let static_borrow_clone = static_borrow.clone();
        assert_eq!(static_borrow.as_str(), static_borrow_clone.as_str());
    }

    #[test]
    fn test_equivalence() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let s = "a_very_large_string_that_exceeds_15_bytes";

        let static_name = Name::from_static(s);
        let heap_name = Name::try_from(s.to_string()).unwrap();
        let borrowed_name = Name::try_from(s).unwrap();

        // They must all be equal
        assert_eq!(static_name, heap_name);
        assert_eq!(static_name, borrowed_name);
        assert_eq!(heap_name, borrowed_name);

        // They must have the same hash
        fn calculate_hash<T: Hash>(t: &T) -> u64 {
            let mut s = DefaultHasher::new();
            t.hash(&mut s);
            s.finish()
        }

        assert_eq!(calculate_hash(&static_name), calculate_hash(&heap_name));
        assert_eq!(calculate_hash(&static_name), calculate_hash(&borrowed_name));

        // Different names are not equal and should have different hashes
        let s2 = "another_large_string_that_is_different";
        let other_name = Name::from_static(s2);
        assert_ne!(static_name, other_name);
        assert_ne!(calculate_hash(&static_name), calculate_hash(&other_name));
    }
}
