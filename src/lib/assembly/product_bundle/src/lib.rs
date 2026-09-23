// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Product Bundles are hermetic directories of assembled artifacts that can be
//! emulated, flashed, and OTA'd.

mod product_bundle;
mod product_bundle_builder;
mod v2;

pub use product_bundle::{
    LoadedProductBundle, ProductBundle, ProductBundleExtractError, ProductBundleLoadError,
    ProductBundleWriteError, get_repositories,
};
pub use product_bundle_builder::ProductBundleBuilder;
pub use v2::{ProductBundleV2, Repository, Type};

// Re-export for convenience with the ProductBundleBuilder.
pub use assembly_partitions_config::Slot;
