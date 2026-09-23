// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for calculating the size of blobs in a package.

mod blob_size;

pub use blob_size::{BlobSize, BlobSizeCalculator};
