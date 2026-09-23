// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common utilities used by directory related tests.
//!
//! Most assertions are macros as they need to call async functions themselves.  As a typical test
//! will have multiple assertions, it save a bit of typing to write `assert_something!(arg)`
//! instead of `assert_something(arg).await`.

use crate::directory::common::encode_dirent;
use crate::directory::entry::EntryInfo;
use flex_fuchsia_io as fio;

/// A helper to build the "expected" output for a `ReadDirents` call from the Directory protocol in
/// fuchsia.io.
pub struct DirentsSameInodeBuilder {
    expected: Vec<u8>,
    inode: u64,
}

impl DirentsSameInodeBuilder {
    pub fn new(inode: u64) -> Self {
        DirentsSameInodeBuilder { expected: vec![], inode }
    }

    pub fn add(&mut self, type_: fio::DirentType, name: &str) -> &mut Self {
        assert!(
            encode_dirent(&mut self.expected, u64::MAX, &EntryInfo::new(self.inode, type_), name),
            "Failed to encode dirent for {name:?}"
        );
        self
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.expected
    }
}
