// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use camino::Utf8PathBuf;
use fuchsia_url::Hash;
use fuchsia_url::fuchsia_pkg::UnpinnedAbsolutePackageUrl;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(PartialEq, Hash, Eq, Debug, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "protocol")]
    Protocol(String),
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageContents {
    /// The URL of this package.
    pub url: UnpinnedAbsolutePackageUrl,
    /// The named files included in this package.
    pub files: Vec<PackageFile>,
    /// The named components included in this package.
    pub components: HashMap<String, ComponentContents>,
    /// The blobs referenced by this package as "blobs/*" files.
    pub blobs: Vec<String>,
}

impl PackageContents {
    pub fn new(url: UnpinnedAbsolutePackageUrl) -> Self {
        Self { url, files: Vec::new(), components: HashMap::new(), blobs: Vec::new() }
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct PackageFile {
    pub name: String,
    pub hash: String,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ComponentContents {
    pub used_from_parent: HashSet<Capability>,
    pub used_from_child: HashSet<(Capability, String)>,
    pub offered_from_self: HashSet<Capability>,
    pub exposed_from_self: HashSet<Capability>,
    pub exposed_from_child: HashSet<(Capability, String)>,
}

#[derive(Serialize, Deserialize)]
pub struct OutputSummary {
    pub packages: BTreeMap<Hash, PackageContents>,
    pub contents: BTreeMap<Hash, FileInfo>,
    pub files: BTreeMap<u32, FileMetadata>,
    pub protocol_to_client: ProtocolToClientMap,
}

pub type ProtocolToClientMap = HashMap<String, HashMap<Hash, HashSet<String>>>;

#[derive(Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub source_path: String,
}

#[derive(Serialize, Deserialize)]
pub enum FileInfo {
    #[serde(rename = "elf")]
    Elf(ElfContents),
    #[serde(rename = "other")]
    Other(OtherContents),
}

#[derive(Serialize, Deserialize)]
pub struct ElfContents {
    pub source_path: String,
    pub source_file_references: BTreeSet<u32>,
}

impl ElfContents {
    pub fn new(source_path: String) -> Self {
        Self { source_path, source_file_references: BTreeSet::new() }
    }
}

#[derive(Serialize, Deserialize)]
pub struct OtherContents {
    pub source_path: Utf8PathBuf,
}
