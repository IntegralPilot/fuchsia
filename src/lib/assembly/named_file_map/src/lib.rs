// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Map that ensures that all files have unique destinations and hashes.

mod named_file_map;

pub use named_file_map::{NamedFileMap, SourceMerklePair};
