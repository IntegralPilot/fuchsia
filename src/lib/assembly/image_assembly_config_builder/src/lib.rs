// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod compiled_package;
mod image_assembly_config_builder;
mod product_assembly;

pub use crate::image_assembly_config_builder::{ImageAssemblyConfigBuilder, PackageOrigin};
pub use assembly_cli_args::ValidationMode;
pub use product_assembly::ProductAssembly;
