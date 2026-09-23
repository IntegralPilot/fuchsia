// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_hash::Hash;
use std::collections::BTreeMap;
use std::io::{Read, Write};

pub struct PackageArchiveBuilder {
    entries: BTreeMap<String, (u64, Box<dyn Read>)>,
}

impl PackageArchiveBuilder {
    pub fn with_meta_far(meta_far_size: u64, meta_far_content: Box<dyn Read>) -> Self {
        Self { entries: BTreeMap::from([("meta.far".into(), (meta_far_size, meta_far_content))]) }
    }

    pub fn add_blob(&mut self, hash: Hash, blob_size: u64, blob_content: Box<dyn Read>) {
        self.entries.insert(hash.to_string(), (blob_size, blob_content));
    }

    pub fn build(self, out: impl Write) -> Result<(), fuchsia_archive::Error> {
        fuchsia_archive::write(out, self.entries)
    }
}
