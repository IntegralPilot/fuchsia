// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An assembly container that holds a subset of product functionality that
//! can be shipped between repositories, included in product configs, and
//! swapped out in hybrid product configs.

#![deny(missing_docs)]

mod product_input_bundle;
pub use product_input_bundle::{ProductInputBundle, ProductPackageDetails, ProductPackagesConfig};
