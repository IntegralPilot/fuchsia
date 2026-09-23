// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::descriptor::{
    ChainPartitionDescriptor, Descriptor, HashDescriptorBuilder, KernelCmdlineDescriptor,
    PropertyDescriptor, Salt,
};
use crate::footer::append_vbmeta_as_footer;
use crate::header::Header;
use crate::key::{Key, SIGNATURE_SIZE, SignFailure};

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use ring::digest;
use std::collections::BTreeMap;
use std::fs;
use zerocopy::IntoBytes;

const HASH_SIZE: u64 = 0x40;

/// Specifies how and where to output the generated VBMeta artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VBMetaOutput {
    /// Name to identify the partition or output artifact
    /// (e.g., "fuchsia" producing `fuchsia.vbmeta`).
    pub name: String,

    /// If provided, appends the VBMeta as a footer to a copy of
    /// the specified image file
    /// instead of creating a standalone `.vbmeta` file.
    pub add_footer_to: Option<Utf8PathBuf>,
}

/// Fully declarative plumbing configuration for building
/// a single VBMeta artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct VBMetaConfig {
    /// Output generation and destination parameters.
    pub output: VBMetaOutput,

    /// Path to the PEM-encoded private signing key.
    pub key: Utf8PathBuf,

    /// Optional path to public key metadata (e.g., ATX metadata).
    pub key_metadata: Option<Utf8PathBuf>,

    /// Images to be hashed and embedded as Hash descriptors.
    pub hash_descriptors: Vec<HashDescriptor>,

    /// Pre-calculated or external raw Hash descriptors to embed
    /// without hashing.
    pub raw_descriptors: Vec<RawHashDescriptor>,

    /// Key-value properties to embed as Property descriptors.
    pub property_descriptors: BTreeMap<String, String>,

    /// Chained partitions to verify and embed.
    pub chain_partitions: Vec<ChainPartition>,

    /// Optional base merkle root to embed as a kernel command line descriptor.
    pub base_merkle: Option<String>,

    /// The rollback index to encode in the VBMeta header.
    pub rollback_index: u64,

    /// Optional salt to use when calculating digests
    /// (defaults to random if None).
    pub salt: Option<Salt>,
}

/// Fluent builder for constructing a VBMeta artifact.
#[derive(Debug)]
pub struct VBMetaBuilder {
    config: VBMetaConfig,
}

impl VBMetaBuilder {
    /// Appends the VBMeta as a footer to a copy of the specified image file.
    pub fn add_footer_to(mut self, path: impl Into<Utf8PathBuf>) -> Self {
        self.config.output.add_footer_to = Some(path.into());
        self
    }

    /// Sets optional public key metadata (e.g., ATX metadata).
    pub fn key_metadata(mut self, path: impl Into<Utf8PathBuf>) -> Self {
        self.config.key_metadata = Some(path.into());
        self
    }

    /// Adds an image Hash descriptor.
    pub fn hash_descriptor(
        mut self,
        partition_name: impl Into<String>,
        image_path: impl Into<Utf8PathBuf>,
    ) -> Self {
        self.config.hash_descriptors.push(HashDescriptor {
            partition_name: partition_name.into(),
            image_path: image_path.into(),
            flags: 0,
            min_avb_version: None,
        });
        self
    }

    /// Adds an image Hash descriptor with custom flags and minimum AVB version.
    pub fn hash_descriptor_with_flags(mut self, config: HashDescriptor) -> Self {
        self.config.hash_descriptors.push(config);
        self
    }

    /// Adds an external or unhashed raw Hash descriptor.
    pub fn raw_descriptor(mut self, raw: RawHashDescriptor) -> Self {
        self.config.raw_descriptors.push(raw);
        self
    }

    /// Adds a key-value property descriptor.
    pub fn property_descriptor(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.property_descriptors.insert(key.into(), value.into());
        self
    }

    /// Adds a chained partition descriptor.
    pub fn chain_partition(mut self, chain: ChainPartition) -> Self {
        self.config.chain_partitions.push(chain);
        self
    }

    /// Sets the base merkle root for the kernel command line descriptor.
    pub fn base_merkle(mut self, merkle: impl Into<String>) -> Self {
        self.config.base_merkle = Some(merkle.into());
        self
    }

    /// Sets the rollback index encoded in the VBMeta header.
    pub fn rollback_index(mut self, index: u64) -> Self {
        self.config.rollback_index = index;
        self
    }

    /// Sets an explicit salt to use when calculating digests
    /// (defaults to random if None).
    pub fn salt(mut self, salt: Salt) -> Self {
        self.config.salt = Some(salt);
        self
    }

    /// Builds the declarative configuration struct.
    pub fn build(self) -> VBMetaConfig {
        self.config
    }

    /// Builds and constructs the VBMeta image, returning the
    /// resulting path on disk.
    pub fn construct(self, outdir: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        VBMeta::construct(&self.config, outdir)
    }
}

/// Configuration for an image Hash descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HashDescriptor {
    /// Name of the partition (e.g., "zircon", "boot", "initrd_normal").
    pub partition_name: String,

    /// Path to the source image file on host to be hashed.
    pub image_path: Utf8PathBuf,

    /// Custom flags for this descriptor.
    pub flags: u32,

    /// Optional minimum AVB version.
    pub min_avb_version: Option<[u32; 2]>,
}

/// Configuration for a pre-calculated or unhashed Hash descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawHashDescriptor {
    /// Name of the partition.
    pub partition_name: String,
    /// Declared size in bytes.
    pub size: u64,
    /// Optional salt.
    pub salt: Option<Salt>,
    /// Optional calculated digest.
    pub digest: Option<[u8; 32]>,
    /// Flags.
    pub flags: u32,
    /// Optional minimum AVB version.
    pub min_avb_version: Option<[u32; 2]>,
}

/// Configuration for a Chained Partition descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChainPartition {
    /// Name of the chained partition (e.g., "vbmeta_system").
    pub partition_name: String,

    /// Rollback index location.
    pub rollback_index_location: u32,

    /// Path to the public key file verifying this partition.
    pub public_key_path: Utf8PathBuf,
}

#[derive(Debug)]
/// A struct for creating the VBMeta image to be read on startup for verified boot.
///
/// This holds both the completed image bytes and the header, descriptors, and
/// key used to create the vbmeta image, with accessors for each of them.
pub struct VBMeta {
    /// The raw bytes of VBMeta that can be written to the device image.
    bytes: Vec<u8>,
}

impl VBMeta {
    /// Initiates a fluent builder for creating a VBMeta image
    /// with mandatory parameters.
    pub fn builder(output_name: impl Into<String>, key: impl Into<Utf8PathBuf>) -> VBMetaBuilder {
        VBMetaBuilder {
            config: VBMetaConfig {
                output: VBMetaOutput { name: output_name.into(), add_footer_to: None },
                key: key.into(),
                key_metadata: None,
                hash_descriptors: Vec::new(),
                raw_descriptors: Vec::new(),
                property_descriptors: BTreeMap::new(),
                chain_partitions: Vec::new(),
                base_merkle: None,
                rollback_index: 0,
                salt: None,
            },
        }
    }

    /// Builds and signs a VBMeta image according to `config`,
    /// saving the artifact into `outdir` and returning its path.
    pub fn construct(config: &VBMetaConfig, outdir: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let outdir = outdir.as_ref();

        // 1. Read signing key and metadata
        let key_pem = fs::read_to_string(&config.key)
            .with_context(|| format!("reading signing key: {}", config.key))?;
        let key_metadata = match &config.key_metadata {
            Some(path) => {
                fs::read(path).with_context(|| format!("reading key metadata: {}", path))?
            }
            None => Vec::new(),
        };
        let key = Key::try_new(&key_pem, key_metadata).context("parsing AVB signing key")?;

        // 2. Determine salt for hashing (explicit or random)
        let salt = match &config.salt {
            Some(s) => s.clone(),
            None => Salt::random().context("generating random salt")?,
        };

        // 3. Assemble all descriptors
        let mut descriptors = Vec::new();

        // Hash descriptors
        for hash_config in &config.hash_descriptors {
            let image_bytes = fs::read(&hash_config.image_path)
                .with_context(|| format!("reading target image: {}", hash_config.image_path))?;

            let mut builder = HashDescriptorBuilder::default()
                .name(&hash_config.partition_name)
                .size(image_bytes.len() as u64)
                .salt(salt.clone());

            if hash_config.flags != 0 {
                builder = builder.flags(hash_config.flags);
            }
            if let Some(min_avb) = hash_config.min_avb_version {
                builder = builder.min_avb_version(min_avb);
            }

            let hash_desc = builder.build_with_digest_calculated_for(&image_bytes);
            descriptors.push(Descriptor::Hash(hash_desc));
        }

        // Raw / unhashed descriptors
        for raw_config in &config.raw_descriptors {
            let mut builder = HashDescriptorBuilder::default()
                .name(&raw_config.partition_name)
                .size(raw_config.size);

            if let Some(salt) = &raw_config.salt {
                builder = builder.salt(salt.clone());
            }
            if let Some(digest) = &raw_config.digest {
                builder = builder.digest(digest);
            }
            if raw_config.flags != 0 {
                builder = builder.flags(raw_config.flags);
            }
            if let Some(min_avb) = raw_config.min_avb_version {
                builder = builder.min_avb_version(min_avb);
            }

            descriptors.push(Descriptor::Hash(builder.build()));
        }

        // Property descriptors
        for (prop_key, prop_val) in &config.property_descriptors {
            let prop_desc = PropertyDescriptor::new(prop_key.clone(), prop_val.clone());
            descriptors.push(Descriptor::Property(prop_desc));
        }

        // Chain partition descriptors
        for chain_config in &config.chain_partitions {
            let public_key = fs::read(&chain_config.public_key_path)
                .with_context(|| format!("reading key: {}", chain_config.public_key_path))?;

            let chain_desc = ChainPartitionDescriptor {
                rollback_index_location: chain_config.rollback_index_location,
                partition_name: chain_config.partition_name.clone(),
                public_key,
            };
            descriptors.push(Descriptor::ChainPartition(chain_desc));
        }

        // Kernel command line descriptor
        if let Some(merkle) = &config.base_merkle {
            let cmdline = format!("system.base_merkle={}", merkle);
            let cmdline_desc = KernelCmdlineDescriptor::new(0, cmdline);
            descriptors.push(Descriptor::KernelCmdline(cmdline_desc));
        }

        // 4. Sign VBMeta
        let vbmeta = Self::sign_with_rollback(descriptors, key, config.rollback_index)
            .context("signing VBMeta image")?;

        // 5. Output generation (standalone vs footer)
        if let Some(target_image) = &config.output.add_footer_to {
            let base_name = target_image
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid image path: {}", target_image))?;
            let destination = outdir.join(base_name);
            vbmeta
                .append_as_footer(target_image, &destination)
                .with_context(|| format!("appending VBMeta footer: {}", destination))?;
            Ok(destination)
        } else {
            let destination = outdir.join(format!("{}.vbmeta", config.output.name));
            fs::write(&destination, &vbmeta.bytes)
                .with_context(|| format!("writing standalone VBMeta: {}", destination))?;
            Ok(destination)
        }
    }

    /// Similar to `sign` but with a specified rollback index.
    fn sign_with_rollback(
        descriptors: Vec<Descriptor>,
        key: Key,
        rollback_index: u64,
    ) -> Result<Self, SignFailure> {
        let mut header = Header::default();
        header.rollback_index = rollback_index.into();

        // the minimum version in the header must be the minimum version required
        // by all HashDescriptors.
        if let Some(required_avb_version) = descriptors
            .iter()
            .filter_map(|desc| {
                if let Descriptor::Hash(hash) = desc { hash.get_min_avb_version() } else { None }
            })
            .max()
        {
            header.min_avb_version_major = required_avb_version[0].into();
            header.min_avb_version_minor = required_avb_version[1].into();
        }

        let aux_data = generate_aux_data(&mut header, &descriptors, &key);
        let auth_data = generate_auth_data(&mut header, &key, &aux_data)?;

        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&auth_data);
        bytes.extend_from_slice(&aux_data);

        Ok(VBMeta { bytes })
    }

    /// Appends the binary contents to a copy of `image` as a VBMeta footer at
    /// `destination`.
    fn append_as_footer(
        &self,
        image: impl AsRef<Utf8Path>,
        destination: impl AsRef<Utf8Path>,
    ) -> Result<()> {
        append_vbmeta_as_footer(
            &self.bytes,
            image.as_ref().as_std_path(),
            destination.as_ref().as_std_path(),
        )
    }
}

fn generate_aux_data(header: &mut Header, descriptors: &[Descriptor], key: &Key) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::new();

    // Append the descriptors.
    for descriptor in descriptors {
        data.extend_from_slice(&descriptor.to_bytes());
    }
    header.descriptors_offset.set(0);
    header.descriptors_size.set(data.len() as u64);

    // Append the key.
    let key_header = key.generate_key_header();
    header.public_key_offset.set(data.len() as u64);
    header.public_key_size.set(key_header.len() as u64);
    data.extend_from_slice(&key_header);

    // Append the metadata.
    header.public_key_metadata_offset.set(data.len() as u64);
    header.public_key_metadata_size.set(key.metadata_bytes.len() as u64);
    data.extend_from_slice(&key.metadata_bytes);

    // Pad the aux data to the nearest 64 byte boundary.
    let length_with_padding = data.len() + 63 & !63;
    data.resize(length_with_padding, 0);
    header.aux_data_size.set(data.len() as u64);

    data
}

fn generate_auth_data(
    header: &mut Header,
    key: &Key,
    aux_data: &[u8],
) -> Result<Vec<u8>, SignFailure> {
    let mut data: Vec<u8> = Vec::new();

    // Set the remaining header values, which must be completed before hashing the header below.
    header.hash_offset.set(0);
    header.hash_size.set(HASH_SIZE);
    header.signature_offset.set(HASH_SIZE);
    header.signature_size.set(SIGNATURE_SIZE);
    header.auth_data_size.set(SIGNATURE_SIZE + HASH_SIZE);

    // Append the hash.
    let mut header_and_aux_data: Vec<u8> = Vec::new();
    header_and_aux_data.extend_from_slice(header.as_bytes());
    header_and_aux_data.extend_from_slice(&aux_data);
    let hash = digest::digest(&digest::SHA512, &header_and_aux_data);
    data.extend_from_slice(&hash.as_ref());

    // Append the signature.
    let signature = key.sign(&header_and_aux_data)?;
    data.extend_from_slice(&signature);

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{HashDescriptor, HashDescriptorBuilder, PropertyDescriptor, Salt};
    use crate::test;

    #[test]
    fn simple_vbmeta() {
        #[rustfmt::skip]
        let expected_header = [
            // Magic: "AVB0"
            0x41, 0x56, 0x42, 0x30,

            // Minimum libavb version: 1.0
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,

            // Size of auth data: 0x240 bytes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x40,

            // Size of aux data: 0x500 bytes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00,

            // Algorithm: 5 = sha256
            0x00, 0x00, 0x00, 0x05,

            // Section offsets/sizes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // hash_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // hash_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // signature_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, // signature_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xD0, // public_key_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x08, // public_key_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0xD8, // public_key_metadata_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D, // public_key_metadata_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // descriptors_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xD0, // descriptors_size

            // Rollback index: 0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

            // Flags: 0
            0x00, 0x00, 0x00, 0x00,

            // Rollback index location: 0
            0x00, 0x00, 0x00, 0x00,

            // Release string: "avbtool 1.2.0"
            0x61, 0x76, 0x62, 0x74, 0x6F, 0x6F, 0x6C, 0x20,
            0x31, 0x2E, 0x32, 0x2E, 0x30, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

            // Padding
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let key = Key::try_new(test::TEST_PEM, test::TEST_METADATA).expect("new key");
        let salt = Salt::try_from(&[0xAA; 32][..]).expect("new salt");
        let descriptor = Descriptor::Hash(HashDescriptor::new("image_name", &[0xBB; 32], salt));
        let descriptors = vec![descriptor];
        let vbmeta_bytes = VBMeta::sign_with_rollback(descriptors, key, 0).unwrap().bytes;
        assert_eq!(vbmeta_bytes[..expected_header.len()], expected_header);
        test::hash_data_and_expect(
            &vbmeta_bytes,
            "295dad85e09205e0c9cb70ea313b4ddd4f959b3d25c4ff3606a9ff816634a240",
        );
    }

    #[test]
    fn simple_vbmeta_with_rollback_index() {
        const ROLLBACK_INDEX: u64 = 0xabcd;

        #[rustfmt::skip]
        let expected_header = [
            // Magic: "AVB0"
            0x41, 0x56, 0x42, 0x30,

            // Minimum libavb version: 1.0
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,

            // Size of auth data: 0x240 bytes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x40,

            // Size of aux data: 0x500 bytes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00,

            // Algorithm: 5 = sha256
            0x00, 0x00, 0x00, 0x05,

            // Section offsets/sizes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // hash_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // hash_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // signature_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, // signature_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xD0, // public_key_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x08, // public_key_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0xD8, // public_key_metadata_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D, // public_key_metadata_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // descriptors_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xD0, // descriptors_size

            // Rollback index: ROLLBACK_INDEX
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xab, 0xcd,

            // Flags: 0
            0x00, 0x00, 0x00, 0x00,

            // Rollback index location: 0
            0x00, 0x00, 0x00, 0x00,

            // Release string: "avbtool 1.2.0"
            0x61, 0x76, 0x62, 0x74, 0x6F, 0x6F, 0x6C, 0x20,
            0x31, 0x2E, 0x32, 0x2E, 0x30, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

            // Padding
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let key = Key::try_new(test::TEST_PEM, test::TEST_METADATA).expect("new key");
        let salt = Salt::try_from(&[0xAA; 32][..]).expect("new salt");
        let descriptor = Descriptor::Hash(HashDescriptor::new("image_name", &[0xBB; 32], salt));
        let descriptors = vec![descriptor];
        let vbmeta_bytes =
            VBMeta::sign_with_rollback(descriptors, key, ROLLBACK_INDEX).unwrap().bytes;
        assert_eq!(vbmeta_bytes[..expected_header.len()], expected_header);
        test::hash_data_and_expect(
            &vbmeta_bytes,
            "6430603d4d43e349de94736702ca132a7f1ac2c320b0b7dd8fd1b7ca7db604ef",
        );
    }

    #[test]
    fn vbmeta_with_multiple_descriptors() {
        #[rustfmt::skip]
        let expected_header_bytes: [u8; 256] = [
            // Magic: "AVB0"
            0x41, 0x56, 0x42, 0x30,

            // Minimum libavb version: 1.2
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,

            // Size of auth data: 0x240 bytes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x40,

            // Size of aux data: 0x5C0 bytes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0xC0,

            // Algorithm: 5 = sha256
            0x00, 0x00, 0x00, 0x05,

            // Section offsets/sizes
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // hash_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // hash_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // signature_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, // signature_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x98, // public_key_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x08, // public_key_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0xA0, // public_key_metadata_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D, // public_key_metadata_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // descriptors_offset
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x98, // descriptors_size

            // Rollback index: 0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

            // Flags: 0
            0x00, 0x00, 0x00, 0x00,

            // Rollback index location: 0
            0x00, 0x00, 0x00, 0x00,

            // Release string: "avbtool 1.2.0"
            0x61, 0x76, 0x62, 0x74, 0x6F, 0x6F, 0x6C, 0x20,
            0x31, 0x2E, 0x32, 0x2E, 0x30, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

            // Padding
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let key = Key::try_new(test::TEST_PEM, test::TEST_METADATA).expect("new key");
        let salt = Salt::try_from(&[0xAA; 32][..]).expect("new salt");
        let hash = Descriptor::Hash(HashDescriptor::new("image_name", &[0xBB; 32], salt));
        let hash_from_raw = Descriptor::Hash(
            HashDescriptorBuilder::default()
                .min_avb_version([1, 2])
                .name("other_image")
                .size(123456789)
                .flags(1)
                .build(),
        );
        let prop = Descriptor::Property(PropertyDescriptor::new(
            "prop_key".to_string(),
            "prop_value".to_string(),
        ));
        let descriptors = vec![hash, hash_from_raw, prop];
        let vbmeta = VBMeta::sign_with_rollback(descriptors, key, 0).unwrap();
        let vbmeta_bytes = &vbmeta.bytes;

        assert_eq!(&vbmeta_bytes[..expected_header_bytes.len()], &expected_header_bytes[..],);
        test::hash_data_and_expect(
            &vbmeta_bytes,
            "bb68ffc6bb7b3a74013de4187f67fe01e897e01818420e38201e41d8a8a823d8",
        );
    }
}
