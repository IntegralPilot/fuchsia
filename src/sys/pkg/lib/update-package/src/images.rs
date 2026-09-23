// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The images and firmware that should be downloaded and written during the update.

use crate::update_mode::UpdateMode;
use camino::Utf8Path;
use fidl_fuchsia_io as fio;
use fuchsia_url::ParseError;
use fuchsia_url::fuchsia_pkg::{AbsoluteComponentUrl, PinnedAbsolutePackageUrl};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use zx_status::Status;

/// An error encountered while resolving images.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ResolveImagesError {
    #[error("while listing files in the update package")]
    ListCandidates(#[source] fuchsia_fs::directory::EnumerateError),
}

/// An error encountered while verifying an [`ImagePackagesSlots`].
#[derive(Debug, Error, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum VerifyError {
    #[error("images list did not contain an entry for 'zbi'")]
    MissingZbi,

    #[error("images list unexpectedly contained an entry for 'zbi'")]
    UnexpectedZbi,
}

/// An error encountered while handling [`ImageMetadata`].
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ImageMetadataError {
    #[error("while reading the image")]
    Io(#[source] std::io::Error),

    #[error("invalid resource path")]
    InvalidResourcePath(#[source] ParseError),
}

/// An error encountered while loading the images.json manifest.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ImagePackagesError {
    #[error("`images.json` not present in update package")]
    NotFound,

    #[error("while opening `images.json`")]
    Open(#[source] fuchsia_fs::node::OpenError),

    #[error("while reading `images.json`")]
    Read(#[source] fuchsia_fs::file::ReadError),

    #[error("while parsing `images.json`")]
    Parse(#[source] serde_json::error::Error),
}

/// A builder of [`ImagePackagesManifest`].
#[derive(Debug, Clone)]
pub struct ImagePackagesManifestBuilder {
    slots: ImagesMetadata,
}

/// A versioned [`ImagePackagesManifest`].
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(tag = "version", content = "contents", deny_unknown_fields)]
#[allow(missing_docs)]
pub enum VersionedImagePackagesManifest {
    #[serde(rename = "1")]
    Version1(ImagePackagesManifest),
}

/// A manifest describing the various images and firmware packages that should be fetched and
/// written during a system update, as well as metadata about those images and where to find them.
#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
#[allow(missing_docs)]
pub struct ImagePackagesManifest {
    #[serde(rename = "partitions")]
    pub assets: Vec<AssetMetadata>,
    pub firmware: Vec<FirmwareMetadata>,
}

/// Metadata describing a firmware image.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct FirmwareMetadata {
    #[serde(rename = "type")]
    pub type_: String,
    pub size: u64,
    #[serde(rename = "hash")]
    pub sha256: fuchsia_hash::Sha256,
    pub url: AbsoluteComponentUrl,
}

/// Metadata describing a Zbi or Vbmeta image, whether or not it is for recovery, and where to
/// resolve it from.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
#[allow(missing_docs)]
pub struct AssetMetadata {
    pub slot: Slot,
    #[serde(rename = "type")]
    pub type_: AssetType,
    pub size: u64,
    #[serde(rename = "hash")]
    pub sha256: fuchsia_hash::Sha256,
    pub url: AbsoluteComponentUrl,
}

/// Whether an asset should be written to recovery or the non-current partition.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum Slot {
    /// Write the asset to the non-current partition (if ABR is supported, otherwise overwrite
    /// the current partition).
    Fuchsia,

    /// Write the asset to the recovery partition.
    Recovery,
}

/// Image asset type.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum AssetType {
    /// A Zircon Boot Image.
    Zbi,

    /// Verified Boot Metadata.
    Vbmeta,
}

impl From<ImagePackagesManifest> for ImagesMetadata {
    fn from(manifest: ImagePackagesManifest) -> Self {
        ImagesMetadata {
            fuchsia: manifest.fuchsia(),
            recovery: manifest.recovery(),
            firmware: manifest.firmware(),
        }
    }
}

/// The metadata for all the images of the OTA, arranged by how the system-updater would write them
/// to the paver.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ImagesMetadata {
    fuchsia: Option<ZbiAndOptionalVbmetaMetadata>,
    recovery: Option<ZbiAndOptionalVbmetaMetadata>,
    firmware: BTreeMap<String, ImageMetadata>,
}

/// Metadata for artifacts unique to an A/B/R boot slot.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ZbiAndOptionalVbmetaMetadata {
    /// The zircon boot image.
    zbi: ImageMetadata,

    /// The optional slot metadata.
    vbmeta: Option<ImageMetadata>,
}

impl ZbiAndOptionalVbmetaMetadata {
    /// Returns an immutable borrow to the ZBI designated in this boot slot.
    pub fn zbi(&self) -> &ImageMetadata {
        &self.zbi
    }

    /// Returns an immutable borrow to the VBMeta designated in this boot slot, if one exists.
    pub fn vbmeta(&self) -> Option<&ImageMetadata> {
        self.vbmeta.as_ref()
    }
}

/// Metadata necessary to determine if a payload matches an image without needing to have the
/// actual image.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ImageMetadata {
    /// The size of the image, in bytes.
    size: u64,

    /// The sha256 hash of the image. Note this is not the merkle root. It is the content hash of
    /// the image.
    sha256: fuchsia_hash::Sha256,

    /// The URL of the image in its package.
    url: AbsoluteComponentUrl,
}

impl FirmwareMetadata {
    /// Creates a new [`FirmwareMetadata`] from the given image metadata and firmware type.
    pub fn new_from_metadata(type_: impl Into<String>, metadata: ImageMetadata) -> Self {
        Self {
            type_: type_.into(),
            size: metadata.size,
            sha256: metadata.sha256,
            url: metadata.url,
        }
    }

    fn key(&self) -> &str {
        &self.type_
    }

    /// Returns the [`ImageMetadata`] for this image.
    pub fn metadata(&self) -> ImageMetadata {
        ImageMetadata { size: self.size, sha256: self.sha256, url: self.url.clone() }
    }
}

impl AssetMetadata {
    /// Creates a new [`AssetMetadata`] from the given image metadata and target slot/type.
    pub fn new_from_metadata(slot: Slot, type_: AssetType, metadata: ImageMetadata) -> Self {
        Self { slot, type_, size: metadata.size, sha256: metadata.sha256, url: metadata.url }
    }

    fn key(&self) -> (Slot, AssetType) {
        (self.slot, self.type_)
    }

    /// Returns the [`ImageMetadata`] for this image.
    pub fn metadata(&self) -> ImageMetadata {
        ImageMetadata { size: self.size, sha256: self.sha256, url: self.url.clone() }
    }
}

impl ImagePackagesManifest {
    /// Returns a [`ImagePackagesManifestBuilder`] with no configured images.
    pub fn builder() -> ImagePackagesManifestBuilder {
        ImagePackagesManifestBuilder {
            slots: ImagesMetadata { fuchsia: None, recovery: None, firmware: Default::default() },
        }
    }

    fn image(&self, slot: Slot, type_: AssetType) -> Option<&AssetMetadata> {
        self.assets.iter().find(|image| image.slot == slot && image.type_ == type_)
    }

    fn image_metadata(&self, slot: Slot, type_: AssetType) -> Option<ImageMetadata> {
        self.image(slot, type_).map(|image| image.metadata())
    }

    fn slot_metadata(&self, slot: Slot) -> Option<ZbiAndOptionalVbmetaMetadata> {
        let zbi = self.image_metadata(slot, AssetType::Zbi);
        let vbmeta = self.image_metadata(slot, AssetType::Vbmeta);

        zbi.map(|zbi| ZbiAndOptionalVbmetaMetadata { zbi, vbmeta })
    }

    /// Returns metadata for the fuchsia boot slot, if present.
    pub fn fuchsia(&self) -> Option<ZbiAndOptionalVbmetaMetadata> {
        self.slot_metadata(Slot::Fuchsia)
    }

    /// Returns metadata for the recovery boot slot, if present.
    pub fn recovery(&self) -> Option<ZbiAndOptionalVbmetaMetadata> {
        self.slot_metadata(Slot::Recovery)
    }

    /// Returns metadata for the firmware images.
    pub fn firmware(&self) -> BTreeMap<String, ImageMetadata> {
        self.firmware.iter().map(|image| (image.type_.to_owned(), image.metadata())).collect()
    }
}

impl ImageMetadata {
    /// Returns new image metadata that designates the given `size` and `hash`, which can be found
    /// at the given `url`.
    pub fn new(size: u64, sha256: fuchsia_hash::Sha256, url: AbsoluteComponentUrl) -> Self {
        Self { size, sha256, url }
    }

    /// Returns the size of the image, in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns the sha256 hash of the image.
    pub fn sha256(&self) -> fuchsia_hash::Sha256 {
        self.sha256
    }

    /// Returns the url of the image.
    pub fn url(&self) -> &AbsoluteComponentUrl {
        &self.url
    }

    /// Compute the size and hash for the image file located at `path`, determining the image's
    /// fuchsia-pkg URL using the given base `url` and `resource` path within the package.
    pub fn for_path(
        path: &Utf8Path,
        url: PinnedAbsolutePackageUrl,
        resource: String,
    ) -> Result<Self, ImageMetadataError> {
        use sha2::Digest as _;

        // TODO(https://fxbug.dev/535596550): Find a better API making a custom Write impl.
        struct WriteHasher<'a, D>(&'a mut D);
        impl<D: sha2::Digest> std::io::Write for WriteHasher<'_, D> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.update(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut hasher = sha2::Sha256::new();
        let file = std::fs::File::open(path).map_err(ImageMetadataError::Io)?;
        let mut reader = std::io::BufReader::new(file);
        let size = std::io::copy(&mut reader, &mut WriteHasher(&mut hasher))
            .map_err(ImageMetadataError::Io)?;
        let sha256 = fuchsia_hash::Sha256::from(*AsRef::<[u8; 32]>::as_ref(&hasher.finalize()));

        let url = AbsoluteComponentUrl::from_package_url_and_resource(url.into(), resource)
            .map_err(ImageMetadataError::InvalidResourcePath)?;

        Ok(Self { size, sha256, url })
    }
}

impl ImagePackagesManifestBuilder {
    /// Configures the "fuchsia" images package to use the given zbi metadata, and optional
    /// vbmeta metadata.
    pub fn fuchsia_package(
        &mut self,
        zbi: ImageMetadata,
        vbmeta: Option<ImageMetadata>,
    ) -> &mut Self {
        self.slots.fuchsia = Some(ZbiAndOptionalVbmetaMetadata { zbi, vbmeta });
        self
    }

    /// Configures the "recovery" images package to use the given zbi metadata, and optional
    /// vbmeta metadata.
    pub fn recovery_package(
        &mut self,
        zbi: ImageMetadata,
        vbmeta: Option<ImageMetadata>,
    ) -> &mut Self {
        self.slots.recovery = Some(ZbiAndOptionalVbmetaMetadata { zbi, vbmeta });
        self
    }

    /// Configures the "firmware" images package from a BTreeMap of ImageMetadata
    pub fn firmware_package(&mut self, firmware: BTreeMap<String, ImageMetadata>) -> &mut Self {
        self.slots.firmware = firmware;
        self
    }

    /// Returns the constructed manifest.
    pub fn build(self) -> VersionedImagePackagesManifest {
        let mut assets = vec![];
        let mut firmware = vec![];

        if let Some(slot) = self.slots.fuchsia {
            assets.push(AssetMetadata::new_from_metadata(Slot::Fuchsia, AssetType::Zbi, slot.zbi));
            if let Some(vbmeta) = slot.vbmeta {
                assets.push(AssetMetadata::new_from_metadata(
                    Slot::Fuchsia,
                    AssetType::Vbmeta,
                    vbmeta,
                ));
            }
        }

        if let Some(slot) = self.slots.recovery {
            assets.push(AssetMetadata::new_from_metadata(Slot::Recovery, AssetType::Zbi, slot.zbi));
            if let Some(vbmeta) = slot.vbmeta {
                assets.push(AssetMetadata::new_from_metadata(
                    Slot::Recovery,
                    AssetType::Vbmeta,
                    vbmeta,
                ));
            }
        }

        for (type_, metadata) in self.slots.firmware {
            firmware.push(FirmwareMetadata::new_from_metadata(type_, metadata));
        }

        VersionedImagePackagesManifest::Version1(ImagePackagesManifest { assets, firmware })
    }
}

impl ImagesMetadata {
    /// Verify that this image package manifest is appropriate for the given update mode.
    ///
    /// * `UpdateMode::Normal` - a non-recovery kernel image is required.
    /// * `UpdateMode::ForceRecovery` - a non-recovery kernel image must not be present.
    pub fn verify(&self, mode: UpdateMode) -> Result<(), VerifyError> {
        let contains_zbi_entry = self.fuchsia.is_some();
        match mode {
            UpdateMode::Normal if !contains_zbi_entry => Err(VerifyError::MissingZbi),
            UpdateMode::ForceRecovery if contains_zbi_entry => Err(VerifyError::UnexpectedZbi),
            _ => Ok(()),
        }
    }

    /// Returns an immutable borrow to the boot slot image package designated as "fuchsia" in this
    /// image packages manifest.
    pub fn fuchsia(&self) -> Option<&ZbiAndOptionalVbmetaMetadata> {
        self.fuchsia.as_ref()
    }

    /// Returns an immutable borrow to the boot slot image package designated as "recovery" in this
    /// image packages manifest.
    pub fn recovery(&self) -> Option<&ZbiAndOptionalVbmetaMetadata> {
        self.recovery.as_ref()
    }

    /// Returns an immutable borrow to the boot slot image package designated as "firmware" in this
    /// image packages manifest.
    pub fn firmware(&self) -> &BTreeMap<String, ImageMetadata> {
        &self.firmware
    }
}

impl<'de> Deserialize<'de> for ImagePackagesManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        #[derive(Debug, Deserialize)]
        pub struct DeImagePackagesManifest {
            partitions: Vec<AssetMetadata>,
            firmware: Vec<FirmwareMetadata>,
        }

        let parsed = DeImagePackagesManifest::deserialize(deserializer)?;

        // Check for duplicate image destinations, verify URL always contains a hash,
        // and check that a zbi is present if vbmeta is present.
        {
            let mut keys = HashSet::new();
            for image in &parsed.partitions {
                if image.metadata().url().package_url().hash().is_none() {
                    return Err(D::Error::custom(format!(
                        "image url {:?} does not contain hash",
                        image.metadata().url()
                    )));
                }

                if !keys.insert(image.key()) {
                    return Err(D::Error::custom(format!(
                        "duplicate image entry: {:?}",
                        image.key()
                    )));
                }
            }

            for slot in [Slot::Fuchsia, Slot::Recovery] {
                if keys.contains(&(slot, AssetType::Vbmeta))
                    && !keys.contains(&(slot, AssetType::Zbi))
                {
                    return Err(D::Error::custom(format!(
                        "vbmeta without zbi entry in partition {slot:?}"
                    )));
                }
            }
        }

        // Check for duplicate firmware destinations and verify that url field contains a  hash.
        {
            let mut keys = HashSet::new();
            for image in &parsed.firmware {
                if image.metadata().url().package_url().hash().is_none() {
                    return Err(D::Error::custom(format!(
                        "firmware url {:?} does not contain hash",
                        image.metadata().url()
                    )));
                }

                if !keys.insert(image.key()) {
                    return Err(D::Error::custom(format!(
                        "duplicate firmware entry: {:?}",
                        image.key()
                    )));
                }
            }
        }

        Ok(ImagePackagesManifest { assets: parsed.partitions, firmware: parsed.firmware })
    }
}

/// Returns structured `images.json` data based on raw file contents.
pub fn parse_image_packages_json(
    contents: &[u8],
) -> Result<ImagePackagesManifest, ImagePackagesError> {
    let VersionedImagePackagesManifest::Version1(manifest) =
        serde_json::from_slice(contents).map_err(ImagePackagesError::Parse)?;

    Ok(manifest)
}

pub(crate) async fn images_metadata(
    proxy: &fio::DirectoryProxy,
) -> Result<ImagesMetadata, ImagePackagesError> {
    image_packages(proxy).await.map(Into::into)
}

async fn image_packages(
    proxy: &fio::DirectoryProxy,
) -> Result<ImagePackagesManifest, ImagePackagesError> {
    let file = fuchsia_fs::directory::open_file(proxy, "images.json", fio::PERM_READABLE)
        .await
        .map_err(|e| match e {
            fuchsia_fs::node::OpenError::OpenError(Status::NOT_FOUND) => {
                ImagePackagesError::NotFound
            }
            e => ImagePackagesError::Open(e),
        })?;

    let contents = fuchsia_fs::file::read(&file).await.map_err(ImagePackagesError::Read)?;

    parse_image_packages_json(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;

    fn sha256(n: u8) -> fuchsia_hash::Sha256 {
        [n; 32].into()
    }

    fn sha256str(n: u8) -> String {
        sha256(n).to_string()
    }

    fn hashstr(n: u8) -> String {
        fuchsia_hash::Hash::from([n; 32]).to_string()
    }

    fn test_url(data: &str) -> AbsoluteComponentUrl {
        format!("fuchsia-pkg://fuchsia.com/update-images-firmware/0?hash=000000000000000000000000000000000000000000000000000000000000000a#{data}").parse().unwrap()
    }

    fn image_package_url(name: &str, hash: u8) -> PinnedAbsolutePackageUrl {
        format!("fuchsia-pkg://fuchsia.com/{name}/0?hash={}", hashstr(hash)).parse().unwrap()
    }

    fn image_package_resource_url(name: &str, hash: u8, resource: &str) -> AbsoluteComponentUrl {
        format!("fuchsia-pkg://fuchsia.com/{name}/0?hash={}#{resource}", hashstr(hash))
            .parse()
            .unwrap()
    }

    #[test]
    fn image_metadata_for_path_empty() {
        let tmp = tempfile::tempdir().expect("/tmp to exist");
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("empty");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"").unwrap();
        drop(f);

        let resource = "resource";
        let url = image_package_url("package", 1);

        assert_eq!(
            ImageMetadata::for_path(&path, url, resource.to_string()).unwrap(),
            ImageMetadata::new(
                0,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".parse().unwrap(),
                image_package_resource_url("package", 1, resource)
            ),
        );
    }

    #[test]
    fn image_metadata_for_path_with_unaligned_data() {
        let tmp = tempfile::tempdir().expect("/tmp to exist");
        let path = Utf8Path::from_path(tmp.path()).unwrap().join("empty");
        let mut f = File::create(&path).unwrap();
        f.write_all(&[0; 8192 + 4096]).unwrap();
        drop(f);

        let resource = "resource";

        let url = image_package_url("package", 1);

        assert_eq!(
            ImageMetadata::for_path(&path, url, resource.to_string()).unwrap(),
            ImageMetadata::new(
                8192 + 4096,
                "f3cc103136423a57975750907ebc1d367e2985ac6338976d4d5a439f50323f4a".parse().unwrap(),
                image_package_resource_url("package", 1, resource)
            ),
        );
    }

    #[test]
    fn parses_minimal_manifest() {
        let raw_json = json!({
            "version": "1",
            "contents": { "partitions" : [], "firmware" : []},
        })
        .to_string();

        let actual = parse_image_packages_json(raw_json.as_bytes()).unwrap();
        assert_eq!(actual, ImagePackagesManifest { assets: vec![], firmware: vec![] });
    }

    #[test]
    fn builder_builds_minimal() {
        assert_eq!(
            ImagePackagesManifest::builder().build(),
            VersionedImagePackagesManifest::Version1(ImagePackagesManifest {
                assets: vec![],
                firmware: vec![],
            }),
        );
    }

    #[test]
    fn builder_builds_populated_manifest() {
        let actual = ImagePackagesManifest::builder()
            .fuchsia_package(
                ImageMetadata::new(
                    1,
                    sha256(1),
                    image_package_resource_url("update-images-fuchsia", 9, "zbi"),
                ),
                Some(ImageMetadata::new(
                    2,
                    sha256(2),
                    image_package_resource_url("update-images-fuchsia", 8, "vbmeta"),
                )),
            )
            .recovery_package(
                ImageMetadata::new(
                    3,
                    sha256(3),
                    image_package_resource_url("update-images-recovery", 7, "zbi"),
                ),
                None,
            )
            .firmware_package(BTreeMap::from([
                (
                    "".into(),
                    ImageMetadata::new(
                        5,
                        sha256(5),
                        image_package_resource_url("update-images-firmware", 6, "a"),
                    ),
                ),
                (
                    "bl2".into(),
                    ImageMetadata::new(
                        6,
                        sha256(6),
                        image_package_resource_url("update-images-firmware", 5, "b"),
                    ),
                ),
            ]))
            .clone()
            .build();
        assert_eq!(
            actual,
            VersionedImagePackagesManifest::Version1(ImagePackagesManifest {
                assets: vec![
                    AssetMetadata {
                        slot: Slot::Fuchsia,
                        type_: AssetType::Zbi,
                        size: 1,
                        sha256: sha256(1),
                        url: image_package_resource_url("update-images-fuchsia", 9, "zbi"),
                    },
                    AssetMetadata {
                        slot: Slot::Fuchsia,
                        type_: AssetType::Vbmeta,
                        size: 2,
                        sha256: sha256(2),
                        url: image_package_resource_url("update-images-fuchsia", 8, "vbmeta"),
                    },
                    AssetMetadata {
                        slot: Slot::Recovery,
                        type_: AssetType::Zbi,
                        size: 3,
                        sha256: sha256(3),
                        url: image_package_resource_url("update-images-recovery", 7, "zbi"),
                    },
                ],
                firmware: vec![
                    FirmwareMetadata {
                        type_: "".to_owned(),
                        size: 5,
                        sha256: sha256(5),
                        url: image_package_resource_url("update-images-firmware", 6, "a"),
                    },
                    FirmwareMetadata {
                        type_: "bl2".to_owned(),
                        size: 6,
                        sha256: sha256(6),
                        url: image_package_resource_url("update-images-firmware", 5, "b"),
                    },
                ],
            })
        );
    }

    #[test]
    fn parses_example_manifest() {
        let raw_json = json!({
            "version": "1",
            "contents":  {
                "partitions": [
                    {
                    "slot" : "fuchsia",
                    "type" : "zbi",
                    "size" : 1,
                    "hash" : sha256str(1),
                    "url" : image_package_resource_url("package", 1, "zbi")
                }, {
                    "slot" : "fuchsia",
                    "type" : "vbmeta",
                    "size" : 2,
                    "hash" : sha256str(2),
                    "url" : image_package_resource_url("package", 1, "vbmeta")
                },
                {
                    "slot" : "recovery",
                    "type" : "zbi",
                    "size" : 3,
                    "hash" : sha256str(3),
                    "url" : image_package_resource_url("package", 1, "rzbi")
                }, {
                    "slot" : "recovery",
                    "type" : "vbmeta",
                    "size" : 3,
                    "hash" : sha256str(3),
                    "url" : image_package_resource_url("package", 1, "rvbmeta")
                    },
                ],
                "firmware": [
                    {
                        "type" : "",
                        "size" : 5,
                        "hash" : sha256str(5),
                        "url" : image_package_resource_url("package", 1, "firmware")
                    }, {
                        "type" : "bl2",
                        "size" : 6,
                        "hash" : sha256str(6),
                        "url" : image_package_resource_url("package", 1, "firmware")
                    },
                ],

            }

            }
        )
        .to_string();

        let actual = parse_image_packages_json(raw_json.as_bytes()).unwrap();
        assert_eq!(
            ImagesMetadata::from(actual),
            ImagesMetadata {
                fuchsia: Some(ZbiAndOptionalVbmetaMetadata {
                    zbi: ImageMetadata::new(
                        1,
                        sha256(1),
                        image_package_resource_url("package", 1, "zbi")
                    ),
                    vbmeta: Some(ImageMetadata::new(
                        2,
                        sha256(2),
                        image_package_resource_url("package", 1, "vbmeta")
                    )),
                },),
                recovery: Some(ZbiAndOptionalVbmetaMetadata {
                    zbi: ImageMetadata::new(
                        3,
                        sha256(3),
                        image_package_resource_url("package", 1, "rzbi")
                    ),
                    vbmeta: Some(ImageMetadata::new(
                        3,
                        sha256(3),
                        image_package_resource_url("package", 1, "rvbmeta")
                    )),
                }),
                firmware: BTreeMap::from([
                    (
                        "".into(),
                        ImageMetadata::new(
                            5,
                            sha256(5),
                            image_package_resource_url("package", 1, "firmware")
                        )
                    ),
                    (
                        "bl2".into(),
                        ImageMetadata::new(
                            6,
                            sha256(6),
                            image_package_resource_url("package", 1, "firmware")
                        )
                    ),
                ])
            }
        );
    }

    #[test]
    fn rejects_duplicate_image_keys() {
        let raw_json = json!({
            "version": "1",
            "contents":  {
                "partitions": [ {
                    "slot" : "fuchsia",
                    "type" : "zbi",
                    "size" : 1,
                    "hash" : sha256str(1),
                    "url" : image_package_resource_url("package", 1, "zbi")
                }, {
                    "slot" : "fuchsia",
                    "type" : "zbi",
                    "size" : 1,
                    "hash" : sha256str(1),
                    "url" : image_package_resource_url("package", 1, "zbi")
                },
                ],
                "firmware": [],
            }
        })
        .to_string();

        assert_matches!(
            parse_image_packages_json(raw_json.as_bytes()),
            Err(ImagePackagesError::Parse(e))
                if e.to_string().contains("duplicate image entry: (Fuchsia, Zbi)")
        );
    }

    #[test]
    fn rejects_duplicate_firmware_keys() {
        let raw_json = json!({
            "version": "1",
            "contents":  {
                "partitions": [],
                "firmware": [
                    {
                        "type" : "",
                        "size" : 5,
                        "hash" : sha256str(5),
                        "url" : image_package_resource_url("package", 1, "firmware")
                    }, {
                        "type" : "",
                        "size" : 5,
                        "hash" : sha256str(5),
                        "url" : image_package_resource_url("package", 1, "firmware")
                    },
                ],
            }
        })
        .to_string();

        assert_matches!(
            parse_image_packages_json(raw_json.as_bytes()),
            Err(ImagePackagesError::Parse(e))
                if e.to_string().contains(r#"duplicate firmware entry: """#)
        );
    }

    #[test]
    fn rejects_vbmeta_without_zbi() {
        let raw_json = json!({
            "version": "1",
            "contents":  {
                "partitions": [{
                    "slot" : "fuchsia",
                    "type" : "vbmeta",
                    "size" : 1,
                    "hash" : sha256str(1),
                    "url" : image_package_resource_url("package", 1, "vbmeta")
                }],
                "firmware": [],
            }
        })
        .to_string();

        assert_matches!(
            parse_image_packages_json(raw_json.as_bytes()),
            Err(ImagePackagesError::Parse(e))
                if e.to_string().contains("vbmeta without zbi entry in partition Fuchsia")
        );
    }

    #[test]
    fn rejects_urls_without_hash_partitions() {
        let raw_json = json!({
            "version": "1",
            "contents":  {
                "partitions": [{
                    "slot" : "fuchsia",
                    "type" : "zbi",
                    "size" : 1,
                    "hash" : sha256str(1),
                    "url" : "fuchsia-pkg://fuchsia.com/package/0#zbi"
                }],
                "firmware": [],
            }
        })
        .to_string();

        assert_matches!(
            parse_image_packages_json(raw_json.as_bytes()),
            Err(ImagePackagesError::Parse(e)) if e.to_string().contains("does not contain hash")
        );
    }

    #[test]
    fn rejects_urls_without_hash_firmware() {
        let raw_json = json!({
            "version": "1",
            "contents":  {
                "partitions": [],
                "firmware": [{
                    "type" : "",
                    "size" : 5,
                    "hash" : sha256str(5),
                    "url" : "fuchsia-pkg://fuchsia.com/package/0#firmware"
                }],
            }
        })
        .to_string();

        assert_matches!(
            parse_image_packages_json(raw_json.as_bytes()),
            Err(ImagePackagesError::Parse(e)) if e.to_string().contains("does not contain hash")
        );
    }

    #[test]
    fn verify_mode_normal_requires_zbi() {
        let with_zbi = ImagesMetadata {
            fuchsia: Some(ZbiAndOptionalVbmetaMetadata {
                zbi: ImageMetadata::new(1, sha256(1), test_url("zbi")),
                vbmeta: None,
            }),
            recovery: None,
            firmware: BTreeMap::new(),
        };

        assert_eq!(with_zbi.verify(UpdateMode::Normal), Ok(()));

        let without_zbi =
            ImagesMetadata { fuchsia: None, recovery: None, firmware: BTreeMap::new() };

        assert_eq!(without_zbi.verify(UpdateMode::Normal), Err(VerifyError::MissingZbi));
    }

    #[test]
    fn verify_mode_force_recovery_requires_no_zbi() {
        let with_zbi = ImagesMetadata {
            fuchsia: Some(ZbiAndOptionalVbmetaMetadata {
                zbi: ImageMetadata::new(1, sha256(1), test_url("zbi")),
                vbmeta: None,
            }),
            recovery: None,
            firmware: BTreeMap::new(),
        };

        assert_eq!(with_zbi.verify(UpdateMode::ForceRecovery), Err(VerifyError::UnexpectedZbi));

        let without_zbi =
            ImagesMetadata { fuchsia: None, recovery: None, firmware: BTreeMap::new() };

        assert_eq!(without_zbi.verify(UpdateMode::ForceRecovery), Ok(()));
    }

    #[fuchsia::test]
    async fn image_packages_detects_missing_manifest() {
        let proxy = vfs::directory::serve_read_only(
            pseudo_directory! {},
            vfs::execution_scope::ExecutionScope::new(),
        );

        assert_matches!(image_packages(&proxy).await, Err(ImagePackagesError::NotFound));
    }

    #[fuchsia::test]
    async fn image_packages_detects_invalid_json() {
        let proxy = vfs::directory::serve_read_only(
            pseudo_directory! {
                "images.json" => read_only("not json!"),
            },
            vfs::execution_scope::ExecutionScope::new(),
        );

        assert_matches!(image_packages(&proxy).await, Err(ImagePackagesError::Parse(_)));
    }

    #[fuchsia::test]
    async fn image_packages_loads_valid_manifest() {
        let proxy = vfs::directory::serve_read_only(
            pseudo_directory! {
                "images.json" => read_only(r#"{
"version": "1",
"contents": { "partitions" : [], "firmware" : [] }
}"#),
            },
            vfs::execution_scope::ExecutionScope::new(),
        );

        assert_eq!(
            image_packages(&proxy).await.unwrap(),
            ImagePackagesManifest { assets: vec![], firmware: vec![] }
        );
    }

    #[fuchsia::test]
    fn boot_slot_accessors() {
        let slot = ZbiAndOptionalVbmetaMetadata {
            zbi: ImageMetadata::new(1, sha256(1), test_url("zbi")),
            vbmeta: Some(ImageMetadata::new(2, sha256(2), test_url("vbmeta"))),
        };

        assert_eq!(slot.zbi(), &ImageMetadata::new(1, sha256(1), test_url("zbi")));
        assert_eq!(slot.vbmeta(), Some(&ImageMetadata::new(2, sha256(2), test_url("vbmeta"))));

        let slot = ZbiAndOptionalVbmetaMetadata {
            zbi: ImageMetadata::new(1, sha256(1), test_url("zbi")),
            vbmeta: None,
        };
        assert_eq!(slot.vbmeta(), None);
    }

    #[fuchsia::test]
    fn image_packages_manifest_accessors() {
        let slot = ZbiAndOptionalVbmetaMetadata {
            zbi: ImageMetadata::new(1, sha256(1), test_url("zbi")),
            vbmeta: Some(ImageMetadata::new(2, sha256(2), test_url("vbmeta"))),
        };

        let mut builder = ImagePackagesManifest::builder();
        builder.fuchsia_package(
            ImageMetadata::new(1, sha256(1), test_url("zbi")),
            Some(ImageMetadata::new(2, sha256(2), test_url("vbmeta"))),
        );
        let VersionedImagePackagesManifest::Version1(manifest) = builder.build();

        assert_eq!(manifest.fuchsia(), Some(slot.clone()));
        assert_eq!(manifest.recovery(), None);
        assert_eq!(manifest.firmware(), BTreeMap::new());

        let mut builder = ImagePackagesManifest::builder();
        builder.recovery_package(
            ImageMetadata::new(1, sha256(1), test_url("zbi")),
            Some(ImageMetadata::new(2, sha256(2), test_url("vbmeta"))),
        );
        let VersionedImagePackagesManifest::Version1(manifest) = builder.build();

        assert_eq!(manifest.fuchsia(), None);
        assert_eq!(manifest.recovery(), Some(slot));
        assert_eq!(manifest.firmware(), BTreeMap::new());

        let mut builder = ImagePackagesManifest::builder();
        builder.firmware_package(BTreeMap::from([(
            "".into(),
            ImageMetadata::new(
                5,
                sha256(5),
                image_package_resource_url("update-images-firmware", 6, "a"),
            ),
        )]));
        let VersionedImagePackagesManifest::Version1(manifest) = builder.build();
        assert_eq!(manifest.fuchsia(), None);
        assert_eq!(manifest.recovery(), None);
        assert_eq!(
            manifest.firmware(),
            BTreeMap::from([(
                "".into(),
                ImageMetadata::new(
                    5,
                    sha256(5),
                    image_package_resource_url("update-images-firmware", 6, "a")
                )
            )])
        )
    }

    #[fuchsia::test]
    fn firmware_image_format_to_image_metadata() {
        let assembly_firmware = FirmwareMetadata {
            type_: "".to_string(),
            size: 1,
            sha256: sha256(1),
            url: image_package_resource_url("package", 1, "firmware"),
        };

        let image_meta_data = ImageMetadata {
            size: 1,
            sha256: sha256(1),
            url: image_package_resource_url("package", 1, "firmware"),
        };

        let firmware_into: ImageMetadata = assembly_firmware.metadata();

        assert_eq!(firmware_into, image_meta_data);
    }

    #[fuchsia::test]
    fn assembly_image_format_to_image_metadata() {
        let assembly_image = AssetMetadata {
            slot: Slot::Fuchsia,
            type_: AssetType::Zbi,
            size: 1,
            sha256: sha256(1),
            url: image_package_resource_url("package", 1, "image"),
        };

        let image_meta_data = ImageMetadata {
            size: 1,
            sha256: sha256(1),
            url: image_package_resource_url("package", 1, "image"),
        };

        let image_into: ImageMetadata = assembly_image.metadata();

        assert_eq!(image_into, image_meta_data);
    }

    #[fuchsia::test]
    fn manifest_conversion_minimal() {
        let manifest = ImagePackagesManifest { assets: vec![], firmware: vec![] };

        let slots = ImagesMetadata { fuchsia: None, recovery: None, firmware: BTreeMap::new() };

        let translated_manifest: ImagesMetadata = manifest.into();
        assert_eq!(translated_manifest, slots);
    }

    #[fuchsia::test]
    fn manifest_conversion_maximal() {
        let manifest = ImagePackagesManifest {
            assets: vec![
                AssetMetadata {
                    slot: Slot::Fuchsia,
                    type_: AssetType::Zbi,
                    size: 1,
                    sha256: sha256(1),
                    url: test_url("1"),
                },
                AssetMetadata {
                    slot: Slot::Fuchsia,
                    type_: AssetType::Vbmeta,
                    size: 2,
                    sha256: sha256(2),
                    url: test_url("2"),
                },
                AssetMetadata {
                    slot: Slot::Recovery,
                    type_: AssetType::Zbi,
                    size: 3,
                    sha256: sha256(3),
                    url: test_url("3"),
                },
                AssetMetadata {
                    slot: Slot::Recovery,
                    type_: AssetType::Vbmeta,
                    size: 4,
                    sha256: sha256(4),
                    url: test_url("4"),
                },
            ],
            firmware: vec![
                FirmwareMetadata {
                    type_: "".to_string(),
                    size: 5,
                    sha256: sha256(5),
                    url: test_url("5"),
                },
                FirmwareMetadata {
                    type_: "bl2".to_string(),
                    size: 6,
                    sha256: sha256(6),
                    url: test_url("6"),
                },
            ],
        };

        let slots = ImagesMetadata {
            fuchsia: Some(ZbiAndOptionalVbmetaMetadata {
                zbi: ImageMetadata::new(1, sha256(1), test_url("1")),
                vbmeta: Some(ImageMetadata::new(2, sha256(2), test_url("2"))),
            }),
            recovery: Some(ZbiAndOptionalVbmetaMetadata {
                zbi: ImageMetadata::new(3, sha256(3), test_url("3")),
                vbmeta: Some(ImageMetadata::new(4, sha256(4), test_url("4"))),
            }),
            firmware: BTreeMap::from([
                ("".into(), ImageMetadata::new(5, sha256(5), test_url("5"))),
                ("bl2".into(), ImageMetadata::new(6, sha256(6), test_url("6"))),
            ]),
        };

        let translated_manifest: ImagesMetadata = manifest.into();
        assert_eq!(translated_manifest, slots);
    }
}
