// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::collection::DataCollection;
use fuchsia_merkle::Hash;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub enum BlobErrorType {
    /// The blob could not be found.
    Missing,
    /// The blob was found but its contents did not match the expected hash.
    Corrupted,
    /// An error occurred while trying to fetch the blob (e.g., HTTP error, network failure).
    FetchError,
    /// An unknown or generic IO error occurred.
    Other,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct BlobError {
    pub hash: Hash,
    pub parent_package_name: Option<String>,
    pub parent_package_hash: Option<Hash>,
    pub parent_package_url: Option<String>,
    pub error_type: BlobErrorType,
    pub error_details: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub enum OtaError {
    InvalidUpdatePackageHash(String),
    UpdatePackageMetaFarMissing(String),
    PackagesJsonMissing(String),
    PackagesJsonParseFailed(String),
    SystemImageMetaFarMissing(String),
    StaticPackagesMissing(String),
    StaticPackagesInvalidUtf8(String),
    StaticPackagesParseFailed(String),
    InvalidBasePackageHash { hash: String, pkg_name: String, err: String },
    SystemImageNotFound,
    PackageMetaFarMissing { pkg_name: String, err: String },
    PackageMetaContentsMissing { pkg_name: String, err: String },
    PackageMetaContentsInvalidUtf8 { pkg_name: String, err: String },
    PackageMetaContentsParseFailed { pkg_name: String, err: String },
    InvalidBlobHash { hash: String, pkg_name: String, err: String },
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct OtaVerificationReport {
    pub deps: HashSet<PathBuf>,
    pub failed_blobs: Vec<BlobError>,
    pub errors: Vec<OtaError>,
}

impl OtaVerificationReport {
    pub fn new() -> Self {
        Self { deps: HashSet::new(), failed_blobs: Vec::new(), errors: Vec::new() }
    }
}

impl Default for OtaVerificationReport {
    fn default() -> Self {
        Self::new()
    }
}

impl DataCollection for OtaVerificationReport {
    fn collection_name() -> String {
        "OTA Verification Report".to_string()
    }
    fn collection_description() -> String {
        "Reports missing packages, corrupted blobs, and errors from OTA verification".to_string()
    }
}
