// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading /system/meta")]
    ReadSystemMeta(#[source] std::io::Error),

    #[error("parsing /system/meta merkle")]
    ParseSystemMeta(#[source] fuchsia_hash::ParseHashError),

    #[error("connecting to PackageResolver")]
    ConnectPackageResolver(#[source] anyhow::Error),

    #[error("connecting to Paver")]
    ConnectPaver(#[source] anyhow::Error),

    #[error("connecting to SpaceManager")]
    ConnectSpaceManager(#[source] anyhow::Error),

    #[error("system-updater component exited with failure")]
    SystemUpdaterFailed,

    #[error("installation ended unexpectedly")]
    InstallationEndedUnexpectedly,

    #[error("reboot FIDL returned error")]
    RebootFailed(#[source] anyhow::Error),

    #[error("update package")]
    UpdatePackage(#[from] UpdatePackage),

    #[error("getting the asset reader")]
    GetAssetReader(#[source] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum UpdatePackage {
    #[error("fidl error resolving update package")]
    ResolveFidl(#[source] fidl::Error),

    #[error("resolving update package")]
    Resolve(#[source] fidl_fuchsia_pkg_ext::ResolveError),

    #[error("extracting the 'packages' manifest")]
    ExtractPackagesManifest(#[source] update_package::ParsePackageError),

    #[error("extracting the 'images' manifest")]
    ExtractImagePackagesManifest(#[source] update_package::ImagePackagesError),

    #[error("could not find any Fuchsia images in the images manifest")]
    MissingFuchsiaImages,

    #[error("could not find system_image/0 in 'packages' manifest")]
    MissingSystemImage,

    #[error("extracting package hash")]
    Hash(#[source] update_package::HashError),
}
