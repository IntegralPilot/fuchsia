// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Typesafe wrappers around an "update" package.

mod board;
mod epoch;
mod hash;
mod image;
pub mod images;
pub mod manifest;
mod name;
mod packages;
pub mod signed_manifest;
mod update_mode;
mod version;

pub use crate::board::VerifyBoardError;
pub use crate::epoch::ParseEpochError;
pub use crate::hash::HashError;
pub use crate::image::OpenImageError;
pub use crate::images::{
    ImageMetadata, ImageMetadataError, ImagePackagesError, ImagePackagesManifest,
    ImagePackagesManifestBuilder, ImagesMetadata, ResolveImagesError, VerifyError,
    VersionedImagePackagesManifest, ZbiAndOptionalVbmetaMetadata, parse_image_packages_json,
};
pub use crate::name::VerifyNameError;
pub use crate::packages::{
    ParsePackageError, SerializePackageError, parse_packages_json, serialize_packages_json,
};
pub use crate::signed_manifest::MANIFEST_DEV_KEY_PEM;
pub use crate::update_mode::{ParseUpdateModeError, UpdateMode};
pub use crate::version::{ReadVersionError, SystemVersion};

use fidl_fuchsia_io as fio;
use fuchsia_hash::Hash;
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;

/// An open handle to an image package.
#[cfg(target_os = "fuchsia")]
pub struct UpdateImagePackage {
    proxy: fio::DirectoryProxy,
}

#[cfg(target_os = "fuchsia")]
impl UpdateImagePackage {
    /// Creates a new [`UpdateImagePackage`] with a given proxy.
    pub fn new(proxy: fio::DirectoryProxy) -> Self {
        Self { proxy }
    }

    /// Opens the image at given `path` as a resizable VMO buffer.
    pub async fn open_image(&self, path: &str) -> Result<fidl_fuchsia_mem::Buffer, OpenImageError> {
        image::open_from_path(&self.proxy, path).await
    }
}

/// An open handle to an "update" package.
#[derive(Debug)]
pub struct UpdatePackage {
    proxy: fio::DirectoryProxy,
}

impl UpdatePackage {
    /// Creates a new [`UpdatePackage`] with the given proxy.
    pub fn new(proxy: fio::DirectoryProxy) -> Self {
        Self { proxy }
    }

    /// Verifies that the package's name/variant is "update/0".
    pub async fn verify_name(&self) -> Result<(), VerifyNameError> {
        name::verify(&self.proxy).await
    }

    /// Loads the image packages manifest, or determines that it is not present.
    pub async fn images_metadata(&self) -> Result<ImagesMetadata, ImagePackagesError> {
        images::images_metadata(&self.proxy).await
    }

    /// Verifies the board file has the given `contents`.
    pub async fn verify_board(&self, contents: &str) -> Result<(), VerifyBoardError> {
        board::verify_board(&self.proxy, contents).await
    }

    /// Parses the update-mode file to obtain update mode. Returns `Ok(None)` if the update-mode
    /// file is not present in the update package.
    pub async fn update_mode(&self) -> Result<Option<UpdateMode>, ParseUpdateModeError> {
        update_mode::update_mode(&self.proxy).await
    }

    /// Returns the list of package urls that go in the universe of this update package.
    pub async fn packages(&self) -> Result<Vec<PinnedAbsolutePackageUrl>, ParsePackageError> {
        packages::packages(&self.proxy).await
    }

    /// Returns the package hash of this update package.
    pub async fn hash(&self) -> Result<Hash, HashError> {
        hash::hash(&self.proxy).await
    }

    /// Returns the version of this update package.
    pub async fn version(&self) -> Result<SystemVersion, ReadVersionError> {
        version::read_version(&self.proxy).await
    }

    /// Parses the epoch.json file to obtain the epoch. Returns `Ok(None)` if the epoch.json file
    /// is not present in the update package.
    pub async fn epoch(&self) -> Result<Option<u64>, ParseEpochError> {
        epoch::epoch(&self.proxy).await
    }
}

#[cfg(test)]
struct TestUpdatePackage {
    update_pkg: UpdatePackage,
    temp_dir: tempfile::TempDir,
}

#[cfg(test)]
impl TestUpdatePackage {
    #[cfg(not(target_os = "fuchsia"))]
    compile_error!(
        "Building tests for non-fuchsia targets requires a library to serve a temp dir using the fidl_fuchsia_io::Directory protocol"
    );

    fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("/tmp to exist");
        let update_pkg_proxy = fuchsia_fs::directory::open_in_namespace(
            temp_dir.path().to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .expect("temp dir to open");
        Self { temp_dir, update_pkg: UpdatePackage::new(update_pkg_proxy) }
    }

    fn proxy(&self) -> &fio::DirectoryProxy {
        &self.update_pkg.proxy
    }

    async fn add_file(self, path: impl AsRef<std::path::Path>, contents: impl AsRef<[u8]>) -> Self {
        let path = path.as_ref();
        match path.parent() {
            Some(empty) if empty == std::path::Path::new("") => {}
            None => {}
            Some(parent) => std::fs::create_dir_all(self.temp_dir.path().join(parent)).unwrap(),
        }
        fuchsia_fs::file::write_in_namespace(
            self.temp_dir.path().join(path).to_str().unwrap(),
            contents,
        )
        .await
        .expect("create test update package file");
        self
    }
}

#[cfg(test)]
impl std::ops::Deref for TestUpdatePackage {
    type Target = UpdatePackage;

    fn deref(&self) -> &Self::Target {
        &self.update_pkg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn lifecycle() {
        let (proxy, _server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        UpdatePackage::new(proxy);
    }
}
