// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow, bail};
use fuchsia_pkg::{BlobInfo, PackageManifest};
use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
struct Manifest {
    init_rc: Option<String>,
    vintf_manifest: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ManifestBlobs {
    pub init_rc: Option<BlobInfo>,
    pub vintf_manifest: Option<BlobInfo>,
}

/// Load the blobs that contain HAL configuration from the HAL package manifest.
///
/// Returns a tuple of blobs and optionally a path to a manifest JSON, for depfile tracking.
pub fn load_from_package(
    package_manifest: &PackageManifest,
) -> Result<(ManifestBlobs, Option<String>)> {
    // TODO(https://fxbug.dev/42081193): Move theses config files outside of the runtime package.
    // TODO(https://fxbug.dev/42079949): Always require HAL manifest after soft transition.
    const HAL_MANIFEST_PATH: &str = "__android_config__/manifest.json";
    let Some(hal_manifest) =
        package_manifest.blobs().iter().find(|blob| blob.path == HAL_MANIFEST_PATH)
    else {
        return Ok((ManifestBlobs::default(), None));
    };

    let manifest = std::fs::read_to_string(&hal_manifest.source_path)?;
    let manifest: Manifest = serde_json::from_str(&manifest)?;
    let init_rc = manifest.init_rc.map(|p| load_blob(package_manifest, &p)).transpose()?;
    let vintf_manifest =
        manifest.vintf_manifest.map(|p| load_blob(package_manifest, &p)).transpose()?;
    Ok((ManifestBlobs { init_rc, vintf_manifest }, Some(hal_manifest.source_path.clone())))
}

fn load_blob(package_manifest: &PackageManifest, path: &str) -> Result<BlobInfo> {
    if path.starts_with("meta/") {
        bail!("HAL config file paths under `meta/` in the package are not supported: {path}");
    }
    package_manifest
        .blobs()
        .iter()
        .find(|blob| blob.path == path)
        .ok_or_else(|| anyhow!("Cannot find {path} in the package"))
        .map(|blob| blob.clone())
}
