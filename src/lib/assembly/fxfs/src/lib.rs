// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for constructing Fxfs images which can be preinstalled with a set of initial packages.

mod fxfs;

pub use fxfs::{FxfsBuilder, read_blobs_json};
pub use fxfs_make_blob_image::CompressionAlgorithm;
