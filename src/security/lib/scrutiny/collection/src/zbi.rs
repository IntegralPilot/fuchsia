// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::collection::DataCollection;
use scrutiny_utils::bootfs::{BootfsFileIndex, BootfsPackageIndex};
use scrutiny_utils::zbi::ZbiSection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use thiserror::Error;

/// Error that may occur when reading the ZBI.
#[derive(Clone, Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum ZbiError {
    #[error("Failed to open ZBI from update package at {update_package_path}\n{io_error}")]
    FailedToOpenUpdatePackage { update_package_path: PathBuf, io_error: String },
    #[error("Failed to read ZBI from update package at {update_package_path}\n{io_error}")]
    FailedToReadZbi { update_package_path: PathBuf, io_error: String },
    #[error("Failed to parse ZBI from update package at {update_package_path}\n{zbi_error}")]
    FailedToParseZbi { update_package_path: PathBuf, zbi_error: String },
    #[error("Failed to parse bootfs from ZBI from update package at {update_package_path}\n{bootfs_error}")]
    FailedToParseBootfs { update_package_path: PathBuf, bootfs_error: String },
}

/// Defines all of the parsed information in the ZBI.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Zbi {
    pub deps: HashSet<PathBuf>,
    // Raw section data for each zbi section. This section isn't serialized to
    // disk because it occupies a large amount of space.
    #[serde(skip)]
    pub sections: Vec<ZbiSection>,
    pub bootfs_files: BootfsFileIndex,
    pub bootfs_packages: BootfsPackageIndex,
    pub cmdline: Vec<String>,
}

impl DataCollection for Zbi {
    fn collection_name() -> String {
        "ZBI Collection".to_string()
    }
    fn collection_description() -> String {
        "Contains all the items found in the zircon boot image (ZBI) in the update package"
            .to_string()
    }
}
