// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Comprehensive unit tests for path canonicalization matching `zircon/third_party/uapp/dash`
//! (`cd.c`).

use crate::path::canonicalize_logical_path;
use bstr::{BStr, BString};

#[test]
fn test_canonicalize_logical_path_relative() {
    let curdir = BStr::new("/usr/local");
    let dest = BStr::new("bin");
    assert_eq!(canonicalize_logical_path(curdir, dest), BString::from("/usr/local/bin"));

    let curdir_slash = BStr::new("/usr/local/");
    assert_eq!(canonicalize_logical_path(curdir_slash, dest), BString::from("/usr/local/bin"));
}

#[test]
fn test_canonicalize_logical_path_absolute() {
    let curdir = BStr::new("/usr/local");
    let dest = BStr::new("/etc/config");
    assert_eq!(canonicalize_logical_path(curdir, dest), BString::from("/etc/config"));
}

#[test]
fn test_canonicalize_logical_path_dots() {
    let curdir = BStr::new("/a/b/c");

    // Single dot
    assert_eq!(
        canonicalize_logical_path(curdir, BStr::new("./d/./e")),
        BString::from("/a/b/c/d/e")
    );

    // Parent directory ..
    assert_eq!(canonicalize_logical_path(curdir, BStr::new("../d")), BString::from("/a/b/d"));

    // Multiple .. pops
    assert_eq!(canonicalize_logical_path(curdir, BStr::new("../../x/y")), BString::from("/a/x/y"));
}

#[test]
fn test_canonicalize_logical_path_root_and_overpop() {
    let curdir = BStr::new("/a/b");

    // Over-popping past root clamps at / (matching dash / POSIX cd behavior)
    assert_eq!(canonicalize_logical_path(curdir, BStr::new("../../../..")), BString::from("/"));

    let root = BStr::new("/");
    assert_eq!(canonicalize_logical_path(root, BStr::new("..")), BString::from("/"));
    assert_eq!(canonicalize_logical_path(root, BStr::new(".")), BString::from("/"));
    assert_eq!(canonicalize_logical_path(root, BStr::new("foo")), BString::from("/foo"));
}

#[test]
fn test_canonicalize_logical_path_multiple_slashes() {
    let curdir = BStr::new("/a//b///");
    let dest = BStr::new("//c////d//");
    assert_eq!(canonicalize_logical_path(curdir, dest), BString::from("/c/d"));

    let rel_dest = BStr::new("c///d/");
    assert_eq!(canonicalize_logical_path(curdir, rel_dest), BString::from("/a/b/c/d"));
}

#[test]
fn test_canonicalize_logical_path_empty_dest() {
    let curdir = BStr::new("/var/log");
    let dest = BStr::new("");
    assert_eq!(canonicalize_logical_path(curdir, dest), BString::from("/var/log"));
}
