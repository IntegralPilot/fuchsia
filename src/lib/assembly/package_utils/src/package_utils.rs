// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_util::{PathTypeMarker, TypedPathBuf, impl_path_type_marker};
use camino::Utf8Path;
use fuchsia_pkg::{BlobInfo, PackageManifest};
use schemars::JsonSchema;

/// Read the package manifest and return the blobs that should be added to bootfs.
/// This filters out the meta/ blob and strips the "bootfs/" prefix from the blob path if present.
pub fn bootfs_files_from_package(manifest_path: impl AsRef<Utf8Path>) -> Result<Vec<BlobInfo>> {
    let manifest = PackageManifest::try_load_from(manifest_path.as_ref())
        .with_context(|| format!("parsing {} as a package manifest", manifest_path.as_ref()))?;

    Ok(manifest
        .into_blobs()
        .into_iter()
        .filter_map(|mut blob| {
            if blob.path.starts_with("meta/") {
                return None;
            }
            if let Some(stripped) = blob.path.strip_prefix("bootfs/") {
                blob.path = stripped.to_string();
            }
            Some(blob)
        })
        .collect())
}

/// The marker trait for paths within a package
#[derive(JsonSchema)]
pub struct InternalPathMarker {}
impl_path_type_marker!(InternalPathMarker);

/// The semantic type for paths within a package
pub type PackageInternalPathBuf = TypedPathBuf<InternalPathMarker>;

/// The marker trait for the source path when that's ambiguous (like in a list
/// of source to destination paths)
#[derive(JsonSchema)]
pub struct SourcePathMarker {}
impl_path_type_marker!(SourcePathMarker);

/// The semantic type for paths that are the path to the source of a file to use
/// in some context.  Such as the source file for a blob in a package.
pub type SourcePathBuf = TypedPathBuf<SourcePathMarker>;

/// The marker trait for paths to a PackageManifest
#[derive(JsonSchema)]
pub struct PackageManifestPathMarker {}
impl_path_type_marker!(PackageManifestPathMarker);

/// The semantic type for paths that are the path to a package manifest.
pub type PackageManifestPathBuf = TypedPathBuf<PackageManifestPathMarker>;
