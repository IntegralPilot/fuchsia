// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Path manipulation and canonicalization utilities.
//!
//! Logical path canonicalization in this module matches the semantics of POSIX `cd -L`
//! and `pwd -L` as implemented in `zircon/third_party/uapp/dash` (specifically `cd.c`
//! and `cdcmd`/`updatepwd`).

use bstr::{BStr, BString, ByteSlice};

/// Canonicalizes a path logically by joining `curdir` and `dest` (if `dest` is relative)
/// and resolving `.` and `..` components symbolically without consulting the filesystem.
///
/// This matches the logical path processing in `zircon/third_party/uapp/dash` (`cd.c`).
pub fn canonicalize_logical_path(curdir: &BStr, dest: &BStr) -> BString {
    let mut full = Vec::new();
    if !dest.starts_with(b"/") {
        full.extend_from_slice(curdir.as_bytes());
        if !curdir.ends_with(b"/") {
            full.push(b'/');
        }
    }
    full.extend_from_slice(dest.as_bytes());

    let mut stack: Vec<&[u8]> = Vec::new();
    for part in full.split(|&b| b == b'/') {
        if part.is_empty() || part == b"." {
            continue;
        }
        if part == b".." {
            stack.pop();
        } else {
            stack.push(part);
        }
    }

    if stack.is_empty() {
        BString::from("/")
    } else {
        let mut res = Vec::new();
        for comp in stack {
            res.push(b'/');
            res.extend_from_slice(comp);
        }
        BString::from(res)
    }
}
