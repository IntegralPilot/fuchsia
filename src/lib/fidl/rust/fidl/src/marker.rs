// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Marker types supporting FIDL bindings.

/// A marker for source-breaking patterns.
///
/// Generated FIDL bindings use this type to explicitly mark Rust types' usage
/// pattern as source-breaking. That is, even if the marked type provides ABI
/// guarantees handling fields marked with [`SourceBreaking`] means _probably_
/// opting out of source compatibility guarantees provided by the Rust FIDL
/// bindings.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct SourceBreaking;

/// A wrapper type whose [`Debug`](std::fmt::Debug) implementation formats the inner value wrapped in `"SENSITIVE{"` and `"}"`.
///
/// Generated FIDL bindings use this type to decorate fields marked with the
/// `@sensitive` attribute in their [`Debug`](std::fmt::Debug) implementations.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SensitiveDebug<'a, T: ?Sized>(pub &'a T);

impl<'a, T: ?Sized + std::fmt::Debug> std::fmt::Debug for SensitiveDebug<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SENSITIVE{{{:?}}}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitive_debug() {
        let val = 42;
        assert_eq!(format!("{:?}", SensitiveDebug(&val)), "SENSITIVE{42}");
        let str_val = "secret";
        assert_eq!(format!("{:?}", SensitiveDebug(&str_val)), "SENSITIVE{\"secret\"}");
    }
}
