// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Constructing and parsing an assembled system, which contains all the
//! images necessary to boot Fuchsia.

mod assembled_system;
mod base_package;
mod blobfs;
mod extra_hash_descriptor;
mod fvm;
mod fxfs;
mod image;
pub mod vbmeta;
mod zbi;

pub use assembled_system::AssembledSystem;
pub use assembly_release_info::ProductBundleReleaseInfo;
pub use image::{BlobfsContents, Image, PackageMetadata, PackageSetMetadata, PackagesMetadata};
