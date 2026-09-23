// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod product_config;

pub mod assembly_input_bundle;
/// Configuration that's provided to Assembly by the Board
pub mod board_config;
pub mod board_input_bundle;
mod board_input_bundle_set;

pub mod common;
pub mod developer_overrides;
pub mod platform_settings;
pub mod product_settings;

pub use board_config::{Architecture, BibReference, BoardConfig};
pub use board_input_bundle::{BoardInputBundle, BoardProvidedConfig, IncludeInBuildType};
pub use board_input_bundle_set::{BoardInputBundleEntry, BoardInputBundleSet};
pub use common::{
    DriverDetails, FeatureControl, PackageDetails, PackageSet, PackagedDriverDetails,
};
pub use platform_settings::example_config::ExampleConfig;
pub use platform_settings::icu_config::{ICUConfig, Revision};
pub use platform_settings::intl_config::IntlConfig;
pub use platform_settings::{BuildType, FeatureSetLevel};
pub use product_config::ProductConfig;

use common::{option_path_schema, vec_path_schema};
