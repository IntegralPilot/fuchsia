// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::collection::DataCollection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum AdditionalBootConfigError {
    #[error("Failed to open blobfs using build path {build_path} and blobfs archive paths {blobfs_paths:?}\n{blobfs_error}")]
    FailedToOpenBlobfs { build_path: PathBuf, blobfs_paths: Vec<PathBuf>, blobfs_error: String },
    #[error("Failed to parse zbi config path {additional_boot_args_path}")]
    FailedToParseAdditionalBootConfigPath { additional_boot_args_path: PathBuf },
    #[error("Failed to open ZBI from update package at {update_package_path}\n{io_error}")]
    FailedToOpenUpdatePackage { update_package_path: PathBuf, io_error: String },
    #[error("Failed to read ZBI from update package at {update_package_path}\n{io_error}")]
    FailedToReadZbi { update_package_path: PathBuf, io_error: String },
    #[error("Failed to parse ZBI from update package at {update_package_path}\n{zbi_error}")]
    FailedToParseZbi { update_package_path: PathBuf, zbi_error: String },
    #[error("Failed to parse bootfs from ZBI from update package at {update_package_path}\n{bootfs_error}")]
    FailedToParseBootfs { update_package_path: PathBuf, bootfs_error: String },
    #[error("Failed to parse UTF8 string from additional boot config at bootfs:{additional_boot_args_path} in ZBI from update package at {update_package_path}\n{utf8_error}")]
    FailedToParseUtf8AdditionalBootConfig {
        update_package_path: PathBuf,
        additional_boot_args_path: PathBuf,
        utf8_error: String,
    },
    #[error("Failed to parse additional boot config format from additional boot config at bootfs:{additional_boot_args_path} in ZBI from update package at {update_package_path}\n{parse_error}")]
    FailedToParseAdditionalBootConfigFormat {
        update_package_path: PathBuf,
        additional_boot_args_path: PathBuf,
        parse_error: AdditionalBootConfigParseError,
    },
    #[error(
        "Failed to locate additional boot config file at bootfs:{additional_boot_args_path} in ZBI from update package at {update_package_path}"
    )]
    FailedToLocateAdditionalBootConfig {
        update_package_path: PathBuf,
        additional_boot_args_path: PathBuf,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum AdditionalBootConfigParseError {
    #[error("Failed to parse [unique-key]=[values] from additional boot config on line {line_no}:\n{line_contents}")]
    FailedToParseKeyValue { line_no: usize, line_contents: String },
    #[error("AdditionalBoot config contains repeated key in [unique-key]=[values] on line {line_no}:\n{line_contents}\nPreviously declared on line {previous_line_no}:\n{previous_line_contents}")]
    RepeatedKey {
        line_no: usize,
        line_contents: String,
        previous_line_no: usize,
        previous_line_contents: String,
    },
}

/// AdditionalBoot config file contains lines of the form:
/// [unique-key]=[[value-1]+[value-2]+[...]+[value-n]].
pub type AdditionalBootConfigContents = HashMap<String, Vec<String>>;

#[derive(Deserialize, Serialize)]
pub struct AdditionalBootConfigCollection {
    pub deps: HashSet<PathBuf>,
    pub additional_boot_args: Option<AdditionalBootConfigContents>,
    pub errors: Vec<AdditionalBootConfigError>,
}

impl DataCollection for AdditionalBootConfigCollection {
    fn collection_name() -> String {
        "Additional Boot Config Collection".to_string()
    }
    fn collection_description() -> String {
        "Contains [key] => [[values]] entries loaded from a additional boot config file".to_string()
    }
}
