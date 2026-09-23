// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A helper for constructing the vbmeta image.

use crate::base_package::BasePackage;
use anyhow::{Context, Result, anyhow};
use assembly_config_schema::BuildType;
use assembly_images_config::{VBMeta, VBMetaStyle};
use camino::{Utf8Path, Utf8PathBuf};
use std::path::Path;
use utf8_path::path_relative_from_current_dir;
use vbmeta::{RawHashDescriptor, VBMeta as VBMetaImage};

/// The conventional name for the partition name of the hash descriptor in a
/// VBMeta assembled for Fuchsia.
pub const FUCHSIA_HASH_DESCRIPTOR_NAME: &str = "zircon";

/// The Android Virtualization Framework expects a VBMeta with a hash
/// descriptor of the kernel (i.e., the boot shim in our case) with this
/// partition name.
const PVM_HASH_DESCRIPTOR_NAME_KERNEL: &str = "boot";

/// The Android Virtualization Framework expects a hash descriptor of the
/// ramdisk (i.e, the ZBI in our case) with one of two possible partition
/// names. This name enables a debug mode in which the guest starts in an
/// environment with the following properties:
///
/// * The UART MMIO is already shared with the host, which is something that
///   naturally should not be shared in a non-debug context (as it leaks info
///   to the host). All other MMIO and memory remains in the unshared state by
///   default.
///
/// * The DICE/certificate chain encodes debug-ness, signaling to remote
///   attestation services that the guest is not secure.
///
const PVM_HASH_DESCRIPTOR_NAME_RAMDISK_DEBUG: &str = "initrd_debug";

/// The Android Virtualization Framework expects a hash descriptor of the
/// ramdisk (i.e, the ZBI in our case) with one of two possible partition
/// names. This name enables the normal, production mode in which all memory
/// and MMIO starts unshared and the provided DICE chain verifiable as secure.
const PVM_HASH_DESCRIPTOR_NAME_RAMDISK_NORMAL: &str = "initrd_normal";

/// A property known to the Android Virtualization Framework that parameterizes
/// verified booting behaviour.
const PVM_PROP_VIRT_CAP_KEY: &str = "com.android.virt.cap";

/// Despite the name, this value is a general signal to skip rollback
/// protection, which we do not have a scheme for at this time. See
/// https://android.googlesource.com/platform/packages/modules/Virtualization/+/refs/heads/main/guest/pvmfw/#vbmeta-properties
const PVM_PROP_VIRT_CAP_VALUE: &str = "trusty_security_vm";

/// Represents a constructed VBMeta in one of the two supported forms.
#[derive(Debug)]
pub enum ConstructedVBMeta {
    /// The path to a normal, standalone, Fuchsia VBMeta.
    Standalone(Utf8PathBuf),

    /// The path to a system VBMeta image.
    VBMetaSystem(Utf8PathBuf),

    /// The path to a copy of the assembled system's QEMU kernel with a VBMeta
    /// footer appended.
    QemuKernelWithFooter(Utf8PathBuf),
}

/// Construct the vbmeta image.
pub fn construct_vbmeta(
    outdir: impl AsRef<Utf8Path>,
    vbmeta_config: &VBMeta,
    zbi: impl AsRef<Path>,
    boot_shim: impl AsRef<Utf8Path>,
    build_type: BuildType,
    base_package: Option<&BasePackage>,
) -> Result<ConstructedVBMeta> {
    let outdir = outdir.as_ref();
    let zbi_path = Utf8PathBuf::from_path_buf(zbi.as_ref().to_path_buf())
        .map_err(|p| anyhow!("invalid UTF-8 ZBI path: {}", p.display()))?;

    let output_name = match vbmeta_config.style {
        VBMetaStyle::Fuchsia | VBMetaStyle::VBMetaSystem => {
            vbmeta_config.filename.as_deref().unwrap_or("fuchsia").to_string()
        }
        VBMetaStyle::AndroidPvmAuto
        | VBMetaStyle::AndroidPvmNormal
        | VBMetaStyle::AndroidPvmDebug => String::from("boot"),
    };

    let mut builder = VBMetaImage::builder(output_name, vbmeta_config.key.clone());

    if let Some(metadata) = &vbmeta_config.key_metadata {
        builder = builder.key_metadata(metadata.clone());
    }

    let bp = if vbmeta_config.include_base_merkle { base_package } else { None };
    if let Some(bp) = bp {
        builder = builder.base_merkle(bp.merkle.to_string());
    }

    for hash in &vbmeta_config.additional_descriptors {
        builder = builder.raw_descriptor(RawHashDescriptor {
            partition_name: hash.name.clone(),
            size: hash.size,
            flags: hash.flags,
            ..Default::default()
        });
    }

    builder = match vbmeta_config.style {
        VBMetaStyle::Fuchsia | VBMetaStyle::VBMetaSystem => {
            builder.hash_descriptor(FUCHSIA_HASH_DESCRIPTOR_NAME, zbi_path)
        }
        pvm_style @ (VBMetaStyle::AndroidPvmAuto
        | VBMetaStyle::AndroidPvmNormal
        | VBMetaStyle::AndroidPvmDebug) => {
            let debug = pvm_style == VBMetaStyle::AndroidPvmDebug
                || (pvm_style == VBMetaStyle::AndroidPvmAuto && build_type != BuildType::User);

            let zbi_desc_name = if debug {
                PVM_HASH_DESCRIPTOR_NAME_RAMDISK_DEBUG
            } else {
                PVM_HASH_DESCRIPTOR_NAME_RAMDISK_NORMAL
            };

            builder
                .add_footer_to(boot_shim.as_ref().to_path_buf())
                .rollback_index(1)
                .hash_descriptor(PVM_HASH_DESCRIPTOR_NAME_KERNEL, boot_shim.as_ref().to_path_buf())
                .hash_descriptor(zbi_desc_name, zbi_path)
                .property_descriptor(PVM_PROP_VIRT_CAP_KEY, PVM_PROP_VIRT_CAP_VALUE)
        }
    };

    let generated_path = builder
        .construct(outdir)
        .context("constructing VBMeta via crate builder implementation")?;

    let relative_path = path_relative_from_current_dir(&generated_path)
        .with_context(|| format!("calculating relative path for: {}", generated_path))?;

    if vbmeta_config.style == VBMetaStyle::VBMetaSystem {
        Ok(ConstructedVBMeta::VBMetaSystem(relative_path))
    } else if vbmeta_config.style == VBMetaStyle::Fuchsia {
        Ok(ConstructedVBMeta::Standalone(relative_path))
    } else {
        Ok(ConstructedVBMeta::QemuKernelWithFooter(relative_path))
    }
}

#[cfg(test)]
mod tests {
    use super::{ConstructedVBMeta, construct_vbmeta};

    use crate::base_package::BasePackage;

    use assembly_config_schema::BuildType;
    use assembly_images_config::{VBMeta, VBMetaStyle};
    use camino::{Utf8Path, Utf8PathBuf};
    use fuchsia_hash::Hash;
    use tempfile::tempdir;
    use utf8_path::path_relative_from_current_dir;

    #[test]
    fn construct_fuchsia_style() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let key_path = dir.join("key");
        let metadata_path = dir.join("key_metadata");
        std::fs::write(&key_path, test_keys::ATX_TEST_KEY).unwrap();
        std::fs::write(&metadata_path, test_keys::TEST_RSA_4096_PEM).unwrap();

        let vbmeta_config = VBMeta {
            style: VBMetaStyle::Fuchsia,
            name: "custom_name".into(),
            filename: None,
            key: key_path,
            key_metadata: Some(metadata_path),
            additional_descriptors: vec![],
            include_base_merkle: false,
        };

        // Create a fake zbi.
        let zbi_path = dir.join("fuchsia.zbi");
        std::fs::write(&zbi_path, "fake zbi").unwrap();

        let vbmeta = construct_vbmeta(
            dir,
            &vbmeta_config,
            zbi_path,
            Utf8PathBuf::new(),
            BuildType::Eng,
            None,
        )
        .unwrap();

        let ConstructedVBMeta::Standalone(vbmeta_path) = vbmeta else {
            panic!("Expected standalone VBMeta image; got {vbmeta:#?}");
        };
        assert_eq!(
            vbmeta_path,
            path_relative_from_current_dir(dir.join("fuchsia.vbmeta")).unwrap()
        );
    }

    #[test]
    fn construct_fuchsia_style_with_filename() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let key_path = dir.join("key");
        let metadata_path = dir.join("key_metadata");
        std::fs::write(&key_path, test_keys::ATX_TEST_KEY).unwrap();
        std::fs::write(&metadata_path, test_keys::TEST_RSA_4096_PEM).unwrap();

        let vbmeta_config = VBMeta {
            style: VBMetaStyle::Fuchsia,
            name: "custom_name".into(),
            filename: Some("custom_file".into()),
            key: key_path,
            key_metadata: Some(metadata_path),
            additional_descriptors: vec![],
            include_base_merkle: false,
        };

        // Create a fake zbi.
        let zbi_path = dir.join("fuchsia.zbi");
        std::fs::write(&zbi_path, "fake zbi").unwrap();

        let vbmeta = construct_vbmeta(
            dir,
            &vbmeta_config,
            zbi_path,
            Utf8PathBuf::new(),
            BuildType::Eng,
            None,
        )
        .unwrap();

        let ConstructedVBMeta::Standalone(vbmeta_path) = vbmeta else {
            panic!("Expected standalone VBMeta image; got {vbmeta:#?}");
        };
        assert_eq!(
            vbmeta_path,
            path_relative_from_current_dir(dir.join("custom_file.vbmeta")).unwrap()
        );
    }

    #[test]
    fn construct_fuchsia_style_with_base_merkle() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let key_path = dir.join("key");
        let metadata_path = dir.join("key_metadata");
        std::fs::write(&key_path, test_keys::ATX_TEST_KEY).unwrap();
        std::fs::write(&metadata_path, test_keys::TEST_RSA_4096_PEM).unwrap();

        let vbmeta_config = VBMeta {
            style: VBMetaStyle::Fuchsia,
            name: "fuchsia".into(),
            filename: None,
            key: key_path,
            key_metadata: Some(metadata_path),
            additional_descriptors: vec![],
            include_base_merkle: true,
        };

        // Create a fake zbi.
        let zbi_path = dir.join("fuchsia.zbi");
        std::fs::write(&zbi_path, "fake zbi").unwrap();

        // Create a fake base merkle.
        let base_merkle = Hash::from([0xAA; 32]);
        let base_package = BasePackage {
            merkle: base_merkle,
            manifest_path: Utf8PathBuf::from("path/to/manifest"),
        };

        let vbmeta = construct_vbmeta(
            dir,
            &vbmeta_config,
            zbi_path,
            Utf8PathBuf::new(),
            BuildType::Eng,
            Some(&base_package),
        )
        .unwrap();

        let ConstructedVBMeta::Standalone(vbmeta_path) = vbmeta else {
            panic!("Expected standalone VBMeta image; got {vbmeta:#?}");
        };
        assert_eq!(
            vbmeta_path,
            path_relative_from_current_dir(dir.join("fuchsia.vbmeta")).unwrap()
        );
    }

    #[test]
    fn construct_pvm_style() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let key_path = dir.join("key");
        std::fs::write(&key_path, test_keys::ATX_TEST_KEY).unwrap();

        let zbi_path = dir.join("fuchsia.zbi");
        std::fs::write(&zbi_path, "fake zbi").unwrap();

        let boot_shim_path = dir.join("boot-shim.bin");
        std::fs::write(&boot_shim_path, "fake boot shim").unwrap();

        const PVM_STYLES: [VBMetaStyle; 3] = [
            VBMetaStyle::AndroidPvmAuto,
            VBMetaStyle::AndroidPvmDebug,
            VBMetaStyle::AndroidPvmNormal,
        ];
        const BUILD_TYPES: [BuildType; 3] = [BuildType::Eng, BuildType::User, BuildType::UserDebug];

        for style in PVM_STYLES {
            for build_type in BUILD_TYPES {
                let vbmeta_config = VBMeta {
                    style,
                    name: String::new(),
                    filename: None,
                    key: key_path.clone(),
                    key_metadata: None,
                    additional_descriptors: vec![],
                    include_base_merkle: false,
                };

                let vbmeta = construct_vbmeta(
                    dir,
                    &vbmeta_config,
                    &zbi_path,
                    &boot_shim_path,
                    build_type,
                    None,
                )
                .unwrap();

                let ConstructedVBMeta::QemuKernelWithFooter(new_boot_shim_path) = vbmeta else {
                    panic!("Expected standalone VBMeta image; got {vbmeta:#?}");
                };

                assert_eq!(
                    new_boot_shim_path,
                    path_relative_from_current_dir(dir.join("boot-shim.bin")).unwrap()
                );
            }
        }
    }
}
