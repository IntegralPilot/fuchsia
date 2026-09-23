// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fuchsia_url::fuchsia_pkg::{AbsoluteComponentUrl, PinnedAbsolutePackageUrl};
use serde_json::json;
use sha2::Digest as _;
use std::collections::BTreeMap;

/// A fake `update_package::UpdatePackage` backed by a temp dir.
pub struct FakeUpdatePackage {
    update_pkg: update_package::UpdatePackage,
    temp_dir: tempfile::TempDir,
    packages: Vec<String>,
}

impl FakeUpdatePackage {
    /// Creates a new TestUpdatePackage with nothing in it.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("/tmp to exist");
        let update_pkg_proxy = fuchsia_fs::directory::open_in_namespace(
            temp_dir.path().to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .expect("temp dir to open");
        Self {
            temp_dir,
            update_pkg: update_package::UpdatePackage::new(update_pkg_proxy),
            packages: vec![],
        }
    }

    /// Adds a file to the update package, panics on error.
    pub async fn add_file(
        self,
        path: impl AsRef<std::path::Path>,
        contents: impl AsRef<[u8]>,
    ) -> Self {
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

    /// Adds a package to the update package, panics on error.
    pub async fn add_package(mut self, package_url: impl Into<String>) -> Self {
        self.packages.push(package_url.into());
        let packages_json = json!({
            "version": "1",
            "content": self.packages,
        })
        .to_string();
        self.add_file("packages.json", packages_json).await
    }

    /// Set the hash of the update package, panics on error.
    pub async fn hash(self, hash: impl AsRef<[u8]>) -> Self {
        self.add_file("meta", hash).await
    }
}

impl std::ops::Deref for FakeUpdatePackage {
    type Target = update_package::UpdatePackage;

    fn deref(&self) -> &Self::Target {
        &self.update_pkg
    }
}

/// Provided a list of strings representing fuchsia-pkg URLs, constructs
/// a `packages.json` representing those packages and returns the JSON as a
/// string.
pub fn make_packages_json<'a>(urls: impl AsRef<[&'a str]>) -> String {
    json!({
      "version": "1",
      "content": urls.as_ref(),
    })
    .to_string()
}

/// Integration test source epoch
///
/// We specifically make the integration tests dependent on this (rather than e.g. u64::MAX) so that
/// when we bump the epoch, most of the integration tests will fail. To fix this, simply bump this
/// constant to match the SOURCE epoch. This will encourage developers to think critically about
/// bumping the epoch and follow the policy documented on fuchsia.dev.
/// See //src/sys/pkg/bin/system-updater/epoch/playbook.md for information on bumping the epoch.
pub const SOURCE_EPOCH: u64 = 1;

/// Provided an epoch, constructs an `epoch.json` and returns the JSON as a string.
pub fn make_epoch_json(epoch: u64) -> String {
    serde_json::to_string(&epoch::EpochFile::Version1 { epoch }).unwrap()
}

/// Constructs an `epoch.json` with the current epoch and returns the JSON as a string.
pub fn make_current_epoch_json() -> String {
    make_epoch_json(SOURCE_EPOCH)
}

const IMAGES_PACKAGE_NAME: &str = "update-images";

/// Like `crate::PackageBuilder` except it only makes update packages.
pub struct UpdatePackageBuilder {
    images_package_repo: fuchsia_url::RepositoryUrl,
    epoch: Option<u64>,
    packages: Vec<PinnedAbsolutePackageUrl>,
    fuchsia_image: Option<(Vec<u8>, Option<Vec<u8>>)>,
    recovery_image: Option<(Vec<u8>, Option<Vec<u8>>)>,
    images_package_name: Option<fuchsia_url::PackageName>,
    firmware_images: BTreeMap<String, Vec<u8>>,
}

impl UpdatePackageBuilder {
    /// Images, like the ZBI or firmware, are not stored in the update package, but in a separate
    /// package(s) referred to by the images manifest in the update package. For convenience, this
    /// builder takes images by their contents and creates the manifest and separate package, but
    /// to do so this builder must be given the URL of the repository that will serve the images
    /// package so that the update package's manifest can reference it.
    pub fn new(images_package_repo: fuchsia_url::RepositoryUrl) -> Self {
        Self {
            images_package_repo,
            epoch: None,
            packages: vec![],
            fuchsia_image: None,
            recovery_image: None,
            images_package_name: None,
            firmware_images: BTreeMap::new(),
        }
    }

    /// The update package's epoch, used by the system-updater to prevent updating to an
    /// incompatible build.
    /// Defaults to `crate::SOURCE_EPOCH` if not set.
    pub fn epoch(mut self, epoch: u64) -> Self {
        assert_eq!(self.epoch, None);
        self.epoch = Some(epoch);
        self
    }

    /// The additional packages (e.g. base and cache) to be resolved during the OTA.
    pub fn packages(mut self, packages: Vec<PinnedAbsolutePackageUrl>) -> Self {
        assert_eq!(self.packages, vec![]);
        self.packages = packages;
        self
    }

    /// The contents of the zbi and, if provided, vbmeta.
    pub fn fuchsia_image(mut self, zbi: Vec<u8>, vbmeta: Option<Vec<u8>>) -> Self {
        assert_eq!(self.fuchsia_image, None);
        self.fuchsia_image = Some((zbi, vbmeta));
        self
    }

    /// The contents of the zbi and, if provided, vbmeta, to write to the recovery slot.
    pub fn recovery_image(mut self, zbi: Vec<u8>, vbmeta: Option<Vec<u8>>) -> Self {
        assert_eq!(self.recovery_image, None);
        self.recovery_image = Some((zbi, vbmeta));
        self
    }

    /// The name of the package of images pointed to by the update package's images manifest.
    /// Defaults to `update-images` if not set.
    pub fn images_package_name(mut self, name: fuchsia_url::PackageName) -> Self {
        assert_eq!(self.images_package_name, None);
        self.images_package_name = Some(name);
        self
    }

    /// Any additional firmware to write.
    pub fn firmware_images(mut self, images: BTreeMap<String, Vec<u8>>) -> Self {
        assert_eq!(self.firmware_images, BTreeMap::new());
        self.firmware_images = images;
        self
    }

    /// Returns the update package and the package of images referenced by the contained images
    /// manifest.
    pub async fn build(self) -> (UpdatePackage, crate::Package) {
        let Self {
            images_package_repo,
            epoch,
            packages,
            fuchsia_image,
            recovery_image,
            images_package_name,
            firmware_images,
        } = self;
        let images_package_name =
            images_package_name.unwrap_or_else(|| IMAGES_PACKAGE_NAME.parse().unwrap());

        let mut update = crate::PackageBuilder::new("update")
            .add_resource_at(
                "packages.json",
                update_package::serialize_packages_json(&packages).unwrap().as_slice(),
            )
            .add_resource_at(
                "epoch.json",
                make_epoch_json(epoch.unwrap_or(SOURCE_EPOCH)).as_bytes(),
            );

        let mut images = crate::PackageBuilder::new(images_package_name.clone());
        if let Some((zbi, vbmeta)) = &fuchsia_image {
            images = images.add_resource_at("zbi", zbi.as_slice());
            if let Some(vbmeta) = vbmeta.as_ref() {
                images = images.add_resource_at("vbmeta", vbmeta.as_slice());
            }
        }
        if let Some((zbi, vbmeta)) = &recovery_image {
            images = images.add_resource_at("recovery-zbi", zbi.as_slice());
            if let Some(vbmeta) = vbmeta.as_ref() {
                images = images.add_resource_at("recovery-vbmeta", vbmeta.as_slice());
            }
        }
        for (type_, content) in &firmware_images {
            images = images.add_resource_at(format!("firmware-{type_}"), content.as_slice());
        }
        let images = images.build().await.unwrap();

        let mut images_manifest = update_package::ImagePackagesManifest::builder();
        if let Some((zbi, vbmeta)) = &fuchsia_image {
            images_manifest.fuchsia_package(
                image_metadata(
                    zbi,
                    AbsoluteComponentUrl::new(
                        images_package_repo.clone(),
                        images_package_name.clone(),
                        None,
                        Some(*images.hash()),
                        "zbi".to_owned(),
                    )
                    .unwrap(),
                ),
                vbmeta.as_ref().map(|v| {
                    image_metadata(
                        v,
                        AbsoluteComponentUrl::new(
                            images_package_repo.clone(),
                            images_package_name.clone(),
                            None,
                            Some(*images.hash()),
                            "vbmeta".to_owned(),
                        )
                        .unwrap(),
                    )
                }),
            );
        }
        if let Some((zbi, vbmeta)) = &recovery_image {
            images_manifest.recovery_package(
                image_metadata(
                    zbi,
                    AbsoluteComponentUrl::new(
                        images_package_repo.clone(),
                        images_package_name.clone(),
                        None,
                        Some(*images.hash()),
                        "recovery-zbi".to_owned(),
                    )
                    .unwrap(),
                ),
                vbmeta.as_ref().map(|v| {
                    image_metadata(
                        v,
                        AbsoluteComponentUrl::new(
                            images_package_repo.clone(),
                            images_package_name.clone(),
                            None,
                            Some(*images.hash()),
                            "recovery-vbmeta".to_owned(),
                        )
                        .unwrap(),
                    )
                }),
            );
        }
        images_manifest.firmware_package(
            firmware_images
                .into_iter()
                .map(|(type_, content)| {
                    (
                        type_.clone(),
                        image_metadata(
                            content.as_slice(),
                            AbsoluteComponentUrl::new(
                                images_package_repo.clone(),
                                images_package_name.clone(),
                                None,
                                Some(*images.hash()),
                                format!("firmware-{type_}"),
                            )
                            .unwrap(),
                        ),
                    )
                })
                .collect(),
        );
        update = update.add_resource_at(
            "images.json",
            serde_json::to_vec(&images_manifest.clone().build()).unwrap().as_slice(),
        );
        let update = update.build().await.unwrap();

        (UpdatePackage { package: update }, images)
    }
}

fn image_metadata(image: &[u8], url: AbsoluteComponentUrl) -> update_package::ImageMetadata {
    let mut hasher = sha2::Sha256::new();
    let () = hasher.update(image);
    update_package::ImageMetadata::new(
        image.len().try_into().unwrap(),
        fuchsia_hash::Sha256::from(*AsRef::<[u8; 32]>::as_ref(&hasher.finalize())),
        url,
    )
}

/// A `crate::Package` used to drive an OTA.
pub struct UpdatePackage {
    package: crate::Package,
}

impl UpdatePackage {
    /// The underlying `crate::Package`.
    pub fn as_package(&self) -> &crate::Package {
        &self.package
    }

    /// The underlying `crate::Package`.
    pub fn into_package(self) -> crate::Package {
        self.package
    }
}
