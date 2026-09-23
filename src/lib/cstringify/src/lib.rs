// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate contains the [`cstringify`] macro for making a `&'static CStr` out of a path.

#![deny(missing_docs)]

/// Creates a `&'static CStr` from a path.
#[macro_export]
macro_rules! cstringify {
    ($x:path) => {
        // Safety: The concat!() adds a nul byte, and a Rust path cannot contain a nul byte.
        // The latter is true because https://doc.rust-lang.org/reference/identifiers.html excludes
        // Unicode control characters from identifiers, and U+0000 is a control character.
        unsafe {
            ::core::ffi::CStr::from_bytes_with_nul_unchecked(
                concat!(stringify!($x), "\0").as_bytes(),
            )
        }
    };
}

#[cfg(test)]
mod tests {
    use std::ffi;

    #[test]
    fn cstringify_simple_ident() {
        let expected = ffi::CString::new("foo").unwrap();
        let expected = expected.as_c_str();
        let result = cstringify!(foo);

        assert_eq!(expected, result);
    }

    #[test]
    fn cstringify_non_ascii_ident() {
        let expected = ffi::CString::new("例").unwrap();
        let expected = expected.as_c_str();
        let result = cstringify!(例);

        assert_eq!(expected, result);
    }

    #[test]
    fn compatible_with_const() {
        const _TEST_STRING: &'static ffi::CStr = cstringify!(foo);
    }
}
