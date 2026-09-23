// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sys::ZX_MAX_NAME_LEN;
use crate::Status;
use bstr::BStr;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// A wrapper around zircon name fields.
///
/// This type is ABI-compatible with the syscall interface but offers convenient ways to print and
/// create UTF-8 strings.
///
/// Can be created from regular strings or byte slices.
#[derive(
    IntoBytes,
    Copy,
    Clone,
    Default,
    Eq,
    KnownLayout,
    FromBytes,
    Hash,
    Immutable,
    PartialEq,
    PartialOrd,
    Ord,
)]
#[repr(transparent)]
pub struct Name([u8; ZX_MAX_NAME_LEN]);

impl Name {
    /// Create a new name value.
    ///
    /// Returns an error if the string is too long to fit or contains null bytes.
    #[inline]
    pub const fn new(s: &str) -> Result<Self, Status> {
        Self::from_bytes(s.as_bytes())
    }

    /// Create a new name value, truncating the input string to fit and stripping null bytes if
    /// necessary.
    #[inline]
    pub const fn new_lossy(s: &str) -> Self {
        Self::from_bytes_lossy(s.as_bytes())
    }

    /// Create a new name value from raw bytes.
    ///
    /// Returns an error if the slice is longer than `ZX_MAX_LEN_NAME - 1` or contains null bytes.
    #[inline]
    pub const fn from_bytes(b: &[u8]) -> Result<Self, Status> {
        if b.len() >= ZX_MAX_NAME_LEN {
            return Err(Status::INVALID_ARGS);
        }

        let mut inner = [0u8; ZX_MAX_NAME_LEN];
        let mut i = 0;
        while i < b.len() {
            if b[i] == 0 {
                return Err(Status::INVALID_ARGS);
            }
            inner[i] = b[i];
            i += 1;
        }

        Ok(Self(inner))
    }

    /// Create a new name value from raw bytes, truncating the input to fit if necessary. Strips
    /// null bytes.
    #[inline]
    pub const fn from_bytes_lossy(b: &[u8]) -> Self {
        let to_copy = if b.len() <= ZX_MAX_NAME_LEN - 1 { b.len() } else { ZX_MAX_NAME_LEN - 1 };

        let mut inner = [0u8; ZX_MAX_NAME_LEN];
        let mut source_idx = 0;
        let mut dest_idx = 0;
        while source_idx < to_copy {
            if b[source_idx] != 0 {
                inner[dest_idx] = b[source_idx];
                dest_idx += 1;
            }
            source_idx += 1;
        }

        Self(inner)
    }

    fn before_nulls(&self) -> &[u8] {
        self.0.splitn(ZX_MAX_NAME_LEN - 1, |b| *b == 0).next().unwrap_or(&[])
    }

    /// Returns a raw pointer to the name for passing to syscalls.
    pub(crate) fn as_raw(&self) -> *const u8 {
        self.0.as_ptr()
    }

    /// Returns the length of the name before a terminating null byte.
    #[inline]
    pub fn len(&self) -> usize {
        self.before_nulls().len()
    }

    /// Returns whether the name has any non-null bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.before_nulls().is_empty()
    }

    /// Return the non-null portion of the name as a BStr.
    #[inline]
    pub fn as_bstr(&self) -> &BStr {
        BStr::new(self.before_nulls())
    }
}

impl std::fmt::Debug for Name {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_bstr(), f)
    }
}

impl std::fmt::Display for Name {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.as_bstr(), f)
    }
}

impl PartialEq<str> for Name {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.before_nulls() == other.as_bytes()
    }
}

impl PartialEq<&str> for Name {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.eq(*other)
    }
}

impl PartialEq<Name> for str {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        self.as_bytes() == other.before_nulls()
    }
}

impl PartialEq<Name> for &str {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        (*self).eq(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_name() {
        assert_eq!(Name::new("").unwrap(), "");
        assert_eq!(Name::new_lossy(""), "");
    }

    #[test]
    fn short_name() {
        assert_eq!(Name::new("v").unwrap(), "v");
        assert_eq!(Name::new_lossy("v"), "v");
    }

    #[test]
    fn max_len_name() {
        let max_len_name = "a_great_maximum_length_vmo_name";
        assert_eq!(max_len_name.len(), ZX_MAX_NAME_LEN - 1);
        assert_eq!(Name::new(max_len_name).unwrap(), max_len_name);
        assert_eq!(Name::new_lossy(max_len_name), max_len_name);
    }

    #[test]
    fn too_long_name() {
        let too_long_name = "bad_really_too_too_long_vmo_name";
        assert_eq!(too_long_name.len(), ZX_MAX_NAME_LEN);
        assert_eq!(Name::new(too_long_name), Err(Status::INVALID_ARGS));
        assert_eq!(Name::new_lossy(too_long_name), "bad_really_too_too_long_vmo_nam");
    }

    #[test]
    fn interior_null_handling() {
        assert_eq!(Name::new("lol\0lol"), Err(Status::INVALID_ARGS));
        assert_eq!(Name::new_lossy("lol\0lol"), "lollol");
    }
}
