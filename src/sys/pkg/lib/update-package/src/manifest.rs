// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Structs for parsing an OTA manifest.

use crate::SystemVersion;
use crate::update_mode::UpdateMode;
use ota_manifest_proto::fuchsia::update::manifest as proto;
use prost::Message as _;
use std::convert::Infallible;
use std::str::FromStr as _;

/// The type of an image asset.
pub type AssetType = proto::AssetType;

/// Returns structured OTA manifest data based on raw file contents.
pub fn parse_ota_manifest(contents: &[u8]) -> Result<OtaManifest, OtaManifestError> {
    let manifest = proto::OtaManifest::decode(contents).map_err(OtaManifestError::ParseProto)?;
    manifest.try_into().map_err(OtaManifestError::InvalidManifest)
}

/// An error encountered while parsing the OTA manifest.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum OtaManifestError {
    #[error("while parsing proto")]
    ParseProto(#[source] prost::DecodeError),

    #[error("invalid proto manifest: {0}")]
    InvalidManifest(String),
}

/// Information about a particular version of the OS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtaManifest {
    /// The version from the product bundle of the target build. This field is for
    /// informational purposes only and does not change the updater's behavior.
    pub product_bundle_version: SystemVersion,
    /// The board this OTA is for (e.g., "x64", "arm64"). The system updater will
    /// reject the OTA if this does not match the device's expected board name
    /// from `build-info`.
    pub board: String,
    /// The epoch of this OTA. See RFC-0071 for details.
    pub epoch: u64,
    /// The update mode, indicating if this is a normal update or a forced
    /// recovery.
    pub mode: UpdateMode,
    /// The base URL prefix of the blobs, including the delivery blob type. The
    /// final URL for each blob will be "{blob_base_url}/{fuchsia_merkle_root}".
    /// Relative URLs are supported, and will be resolved relative to the URL of
    /// the OTA manifest.
    pub blob_base_url: String,
    /// The partition images that should be written during the update.
    pub images: Vec<Image>,
    /// Additional blobs that should be written to blob storage.
    pub blobs: Vec<Blob>,
}

impl OtaManifest {
    /// Serializes the manifest to a byte vector using the protobuf encoding.
    pub fn serialize(self) -> Vec<u8> {
        let proto: proto::OtaManifest = self.into();
        proto.encode_to_vec()
    }
}

/// The target slot for an image.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Slot {
    /// The primary A/B slot.
    #[default]
    AB,
    /// The recovery slot.
    R,
}

/// An image to be written to a partition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// The slot this image should be written to.
    pub slot: Slot,
    /// The type of the image.
    pub image_type: ImageType,
    /// Metadata about the blob containing the image data.
    pub blob: Blob,
}

impl Image {
    /// Create a new `Image` from a file path.
    pub fn from_path(
        path: impl AsRef<std::path::Path>,
        slot: Slot,
        image_type: ImageType,
    ) -> Result<Self, std::io::Error> {
        let file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        let fuchsia_merkle_root = fuchsia_merkle::root_from_reader(file)?;
        Ok(Self { slot, image_type, blob: Blob { uncompressed_size: size, fuchsia_merkle_root } })
    }
}

/// The type of the image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageType {
    /// A standard system asset like ZBI or VBMETA.
    Asset(AssetType),
    /// A firmware image, with the field value specifying the firmware type.
    Firmware(String),
}

/// Metadata for a blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    /// The uncompressed size of the blob in bytes.
    pub uncompressed_size: u64,
    /// The fuchsia merkle root of the uncompressed blob data.
    pub fuchsia_merkle_root: fuchsia_hash::Hash,
}

impl TryFrom<proto::OtaManifest> for OtaManifest {
    type Error = String;
    fn try_from(proto: proto::OtaManifest) -> Result<Self, Self::Error> {
        let mode = proto.mode().into();
        let version: Result<_, Infallible> =
            crate::SystemVersion::from_str(&proto.product_bundle_version);
        Ok(Self {
            product_bundle_version: version.unwrap(),
            board: proto.board,
            epoch: proto.epoch,
            mode,
            blob_base_url: proto.blob_base_url,
            images: proto
                .images
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            blobs: proto.blobs.into_iter().map(TryInto::try_into).collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<OtaManifest> for proto::OtaManifest {
    fn from(manifest: OtaManifest) -> Self {
        Self {
            product_bundle_version: manifest.product_bundle_version.to_string(),
            board: manifest.board,
            epoch: manifest.epoch,
            mode: proto::UpdateMode::from(manifest.mode).into(),
            blob_base_url: manifest.blob_base_url,
            images: manifest.images.into_iter().map(Into::into).collect(),
            blobs: manifest.blobs.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<proto::UpdateMode> for UpdateMode {
    fn from(mode: proto::UpdateMode) -> Self {
        match mode {
            proto::UpdateMode::Normal => UpdateMode::Normal,
            proto::UpdateMode::ForceRecovery => UpdateMode::ForceRecovery,
        }
    }
}

impl From<UpdateMode> for proto::UpdateMode {
    fn from(mode: UpdateMode) -> Self {
        match mode {
            UpdateMode::Normal => proto::UpdateMode::Normal,
            UpdateMode::ForceRecovery => proto::UpdateMode::ForceRecovery,
        }
    }
}

impl TryFrom<proto::Image> for Image {
    type Error = String;
    fn try_from(image: proto::Image) -> Result<Self, Self::Error> {
        let slot = image.slot().into();
        let image_type =
            image.image_type.ok_or_else(|| "image_type missing".to_string())?.try_into()?;
        let blob = image.blob.ok_or_else(|| "blob missing".to_string())?.try_into()?;
        Ok(Self { slot, image_type, blob })
    }
}

impl From<Image> for proto::Image {
    fn from(image: Image) -> Self {
        Self {
            slot: proto::Slot::from(image.slot).into(),
            image_type: Some(image.image_type.into()),
            blob: Some(image.blob.into()),
        }
    }
}

impl From<proto::Slot> for Slot {
    fn from(slot: proto::Slot) -> Self {
        match slot {
            proto::Slot::Ab => Slot::AB,
            proto::Slot::R => Slot::R,
        }
    }
}

impl From<Slot> for proto::Slot {
    fn from(slot: Slot) -> Self {
        match slot {
            Slot::AB => proto::Slot::Ab,
            Slot::R => proto::Slot::R,
        }
    }
}

impl TryFrom<proto::image::ImageType> for ImageType {
    type Error = String;
    fn try_from(value: proto::image::ImageType) -> Result<Self, Self::Error> {
        match value {
            proto::image::ImageType::Asset(asset) => {
                let asset_type = proto::AssetType::try_from(asset)
                    .map_err(|_| format!("unknown asset type: {asset}"))?;
                Ok(ImageType::Asset(asset_type))
            }
            proto::image::ImageType::Firmware(firmware) => Ok(ImageType::Firmware(firmware)),
        }
    }
}

impl From<ImageType> for proto::image::ImageType {
    fn from(image_type: ImageType) -> Self {
        match image_type {
            ImageType::Asset(asset) => proto::image::ImageType::Asset(asset.into()),
            ImageType::Firmware(firmware) => proto::image::ImageType::Firmware(firmware),
        }
    }
}

impl From<Blob> for proto::Blob {
    fn from(blob: Blob) -> Self {
        Self {
            uncompressed_size: blob.uncompressed_size,
            fuchsia_merkle_root: blob.fuchsia_merkle_root.as_ref().to_vec(),
        }
    }
}

impl TryFrom<proto::Blob> for Blob {
    type Error = String;
    fn try_from(blob: proto::Blob) -> Result<Self, Self::Error> {
        Ok(Self {
            uncompressed_size: blob.uncompressed_size,
            fuchsia_merkle_root: fuchsia_hash::Hash::from(
                <[u8; 32]>::try_from(blob.fuchsia_merkle_root)
                    .map_err(|e| format!("invalid merkle root length: {e:?}"))?,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::assert_matches;
    use std::io::Write as _;
    use std::str::FromStr;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_ota_manifest_success() {
        let proto_manifest = proto::OtaManifest {
            product_bundle_version: "1.2.3.4".to_string(),
            board: "test-board".to_string(),
            epoch: 1,
            mode: proto::UpdateMode::Normal.into(),
            blob_base_url: "http://example.com".to_string(),
            images: vec![
                proto::Image {
                    slot: proto::Slot::Ab.into(),
                    image_type: Some(proto::image::ImageType::Asset(proto::AssetType::Zbi.into())),
                    blob: Some(proto::Blob {
                        uncompressed_size: 1234,
                        fuchsia_merkle_root: vec![1; 32],
                    }),
                },
                proto::Image {
                    slot: proto::Slot::Ab.into(),
                    image_type: Some(proto::image::ImageType::Firmware("bootloader".to_string())),
                    blob: Some(proto::Blob {
                        uncompressed_size: 3456,
                        fuchsia_merkle_root: vec![3; 32],
                    }),
                },
            ],
            blobs: vec![proto::Blob { uncompressed_size: 5678, fuchsia_merkle_root: vec![4; 32] }],
        };
        let buf = proto_manifest.encode_to_vec();

        let manifest = parse_ota_manifest(&buf).unwrap();
        assert_eq!(manifest.product_bundle_version, SystemVersion::from_str("1.2.3.4").unwrap());
        assert_eq!(manifest.board, "test-board");
        assert_eq!(manifest.epoch, 1);
        assert_eq!(manifest.mode, UpdateMode::Normal);
        assert_eq!(manifest.blob_base_url, "http://example.com");

        assert_eq!(manifest.images.len(), 2);
        assert_eq!(manifest.images[0].slot, Slot::AB);
        assert_eq!(manifest.images[0].image_type, ImageType::Asset(AssetType::Zbi));
        assert_eq!(manifest.images[0].blob.uncompressed_size, 1234);
        assert_eq!(manifest.images[0].blob.fuchsia_merkle_root, [1; 32].into());

        assert_eq!(manifest.images[1].slot, Slot::AB);
        assert_eq!(manifest.images[1].image_type, ImageType::Firmware("bootloader".to_string()));
        assert_eq!(manifest.images[1].blob.uncompressed_size, 3456);
        assert_eq!(manifest.images[1].blob.fuchsia_merkle_root, [3; 32].into());

        assert_eq!(manifest.blobs.len(), 1);
        assert_eq!(manifest.blobs[0].uncompressed_size, 5678);
        assert_eq!(manifest.blobs[0].fuchsia_merkle_root, [4; 32].into());
    }

    #[test]
    fn test_parse_ota_manifest_invalid_proto() {
        let err = parse_ota_manifest(b"invalid proto").unwrap_err();
        assert_matches!(err, OtaManifestError::ParseProto(_));
    }

    #[test]
    fn test_parse_ota_manifest_invalid_manifest() {
        let proto_manifest = proto::OtaManifest {
            product_bundle_version: "1.2.3.4".to_string(),
            board: "test-board".to_string(),
            epoch: 1,
            mode: proto::UpdateMode::Normal.into(),
            blob_base_url: "http://example.com".to_string(),
            images: vec![proto::Image {
                slot: proto::Slot::Ab.into(),
                image_type: None,
                blob: Some(proto::Blob {
                    uncompressed_size: 1234,
                    fuchsia_merkle_root: vec![1; 32],
                }),
            }],
            blobs: vec![],
        };
        let buf = proto_manifest.encode_to_vec();

        let err = parse_ota_manifest(&buf).unwrap_err();
        assert_matches!(err, OtaManifestError::InvalidManifest(msg) if msg == "image_type missing");
    }

    #[test]
    fn test_serialize_ota_manifest() {
        let manifest = OtaManifest {
            product_bundle_version: SystemVersion::from_str("1.2.3.4").unwrap(),
            board: "test-board".to_string(),
            epoch: 1,
            mode: UpdateMode::Normal,
            blob_base_url: "http://example.com".to_string(),
            images: vec![
                Image {
                    slot: Slot::AB,
                    image_type: ImageType::Asset(AssetType::Zbi),
                    blob: Blob { uncompressed_size: 1234, fuchsia_merkle_root: [1; 32].into() },
                },
                Image {
                    slot: Slot::AB,
                    image_type: ImageType::Firmware("bootloader".to_string()),
                    blob: Blob { uncompressed_size: 3456, fuchsia_merkle_root: [3; 32].into() },
                },
            ],
            blobs: vec![Blob { uncompressed_size: 5678, fuchsia_merkle_root: [4; 32].into() }],
        };

        let buf = manifest.clone().serialize();
        let parsed_manifest = parse_ota_manifest(&buf).unwrap();

        assert_eq!(manifest, parsed_manifest);
    }

    #[test]
    fn image_from_path() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"hello world").unwrap();

        let image =
            Image::from_path(file.path(), Slot::AB, ImageType::Asset(AssetType::Zbi)).unwrap();

        assert_eq!(image.blob.uncompressed_size, 11);
        assert_eq!(
            image.blob.fuchsia_merkle_root,
            "8af85e2fe5da3385ea468ed1cb8412eaea6530a90b5dd8dee96529c8d9d39b97".parse().unwrap()
        );
    }

    #[test]
    fn image_from_path_large_file() {
        let mut file = NamedTempFile::new().unwrap();
        let chunk = [1; 1024];
        let mut merkle_builder = fuchsia_merkle::BufferedMerkleRootBuilder::default();
        for _ in 0..1000 {
            file.write_all(&chunk).unwrap();
            merkle_builder.write(&chunk);
        }

        let image =
            Image::from_path(file.path(), Slot::AB, ImageType::Asset(AssetType::Zbi)).unwrap();

        assert_eq!(image.blob.uncompressed_size, 1000 * 1024);
        assert_eq!(image.blob.fuchsia_merkle_root, merkle_builder.complete());
    }
}
