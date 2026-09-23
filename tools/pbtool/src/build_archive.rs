// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generate a build archive from a product bundle.

use anyhow::{Context, Result};
use argh::FromArgs;
use assembled_system::{AssembledSystem, Image};
use assembly_release_info::SystemReleaseInfo;
use camino::Utf8PathBuf;
use product_bundle::ProductBundle;

use flate2::read::GzDecoder;
use std::fs::File;
use std::io::{BufReader, copy};
use std::os::unix::fs::PermissionsExt;

const FLASH_SCRIPT_TEMPLATE: &str = r#"#!/bin/sh
DIR="$(dirname "$0")"
set -e

ZIRCON_IMAGE=zircon-a.zbi
ZIRCON_VBMETA=zircon-a.vbmeta
RECOVERY_IMAGE=zircon-r.zbi
RECOVERY_VBMETA=zircon-r.vbmeta
RECOVERY=
SSH_KEY=

for i in "$@"
do
case $i in
    --recovery)
    RECOVERY=true
    ZIRCON_IMAGE=zircon-r.zbi
    ZIRCON_VBMETA=zircon-r.vbmeta
    shift
    ;;
    --ssh-key=*)
    SSH_KEY="${i#*=}"
    shift
    ;;
    *)
    break
    ;;
esac
done

FASTBOOT_ARGS="$@"
PRODUCT="%PRODUCT_NAME_STRING%"
actual=$("$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} getvar product 2>&1 | grep -i product | head -n1 | cut -d' ' -f2-)
if [[ "${actual}" != "${PRODUCT}" ]]; then
  echo >&2 "Expected device ${PRODUCT} but found ${actual}"
  exit 1
fi

BOOTLOADER_STR
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} reboot bootloader
echo 'Sleeping for 5 seconds for the device to de-enumerate.'
sleep 5

"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash zircon_a "${DIR}/${ZIRCON_IMAGE}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash zircon_b "${DIR}/${ZIRCON_IMAGE}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash zircon_r "${DIR}/${RECOVERY_IMAGE}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash vbmeta_a "${DIR}/${ZIRCON_VBMETA}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash vbmeta_b "${DIR}/${ZIRCON_VBMETA}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash vbmeta_r "${DIR}/${RECOVERY_VBMETA}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} set_active a

if [[ -z "${RECOVERY}" ]]; then
  "$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash fvm "${DIR}/fvm.fastboot.blk"
fi

if [[ ! -z "${SSH_KEY}" ]]; then
  is_userspace=$("$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} getvar is-userspace 2>&1 | head -n1 | cut -d' ' -f2-)
  if [[ "${is_userspace}" == "yes" ]]; then
    echo "running in userspace fastboot. rebooting to userspace fastboot"
    "$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} reboot bootloader
    sleep 5
  fi
  "$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} stage "${SSH_KEY}" oem add-staged-bootloader-file ssh.authorized_keys
fi

"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} continue
"#;

const BOOTLOADER_STR: &str = r#""$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash TYPE "${DIR}/BOOTLOADER_NAME"
"#;

/// Generate a build archive using the specified `args`.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "generate-build-archive")]
pub struct GenerateBuildArchive {
    /// path to a product bundle.
    #[argh(option)]
    product_bundle: Utf8PathBuf,

    /// path to a fastboot binary. When set, a flash script using this fastboot
    /// binary is generated at the root of the build archive.
    #[argh(option)]
    fastboot: Option<Utf8PathBuf>,

    /// path to the directory to write a build archive into.
    #[argh(option)]
    out_dir: Utf8PathBuf,
}

impl GenerateBuildArchive {
    pub fn generate(self) -> Result<()> {
        println!("@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@");
        println!("@");
        println!("@  The `pbtool generate-build-archive` is deprecated.");
        println!("@");
        println!("@  Please flash using a product bundle (v2) instead.");
        println!("@");
        println!("@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@");
        let product_bundle = ProductBundle::try_load_from(&self.product_bundle)?;
        let mut product_bundle = match product_bundle {
            ProductBundle::V2(pb) => pb,
        };

        // Ensure the `out_dir` exists.
        std::fs::create_dir_all(&self.out_dir)
            .with_context(|| format!("Creating the out_dir: {}", self.out_dir))?;

        // Collect the Images with the final destinations to add to an images manifest later.
        let mut images = vec![];

        let mut bootloader_string = "".to_owned();

        let copy_artifact = |path: &Utf8PathBuf, name: &str| -> Result<()> {
            // Copy the image to the out_dir.
            let destination = self.out_dir.join(name);
            std::fs::copy(&path, &destination)
                .with_context(|| format!("Copying artifact {} to {}", path, destination))?;
            Ok(())
        };

        for part in &mut product_bundle.partitions.bootstrap_partitions {
            let filename = part
                .image
                .file_name()
                .context(format!("Misformatted bootstrap partition: {}", part.image))?;
            copy_artifact(&part.image, filename)?;
        }
        for part in &mut product_bundle.partitions.bootloader_partitions {
            if part.name.is_none() {
                continue;
            }
            let name = if part.partition_type == "" {
                "firmware.img".to_owned()
            } else {
                format!("{}_{}.img", "firmware", part.partition_type)
            };
            let bootloader_type = part.name.clone().unwrap();
            let bootloader_str = BOOTLOADER_STR.replace("TYPE", &bootloader_type);
            bootloader_string.push_str(&bootloader_str.replace("BOOTLOADER_NAME", &name));
            copy_artifact(&part.image, &name)?;
        }
        for cred in &mut product_bundle.partitions.unlock_credentials {
            let filename =
                cred.file_name().context(format!("Misformatted credential: {}", cred))?;
            copy_artifact(&cred, filename)?;
        }

        // Pull out the relevant files.
        if let Some(a) = product_bundle.system_a {
            for image in a.iter() {
                let entry = match &image {
                    Image::ZBI { path, signed: _ } => Some((path, "zircon-a.zbi")),
                    Image::VBMeta(path) => Some((path, "zircon-a.vbmeta")),
                    Image::FVM(path) => Some((path, "storage-full.blk")),
                    Image::QemuKernel(path) => Some((path, "qemu-kernel.kernel")),
                    Image::FVMFastboot(path) => Some((path, "fvm.fastboot.blk")),
                    Image::FxfsSparse { path, .. } => Some((path, "fxfs.sparse.blk")),
                    _ => None,
                };
                if let Some((path, name)) = entry {
                    copy_artifact(path, name)?;

                    // Create a new Image with the new path.
                    let destination = self.out_dir.join(name);
                    let mut new_image = image.clone();
                    new_image.set_source(destination);
                    images.push(new_image);
                }
            }
        }

        if let Some(r) = product_bundle.system_r {
            for image in r.iter() {
                let entry = match &image {
                    Image::ZBI { path, signed: _ } => Some((path, "zircon-r.zbi")),
                    Image::VBMeta(path) => Some((path, "zircon-r.vbmeta")),
                    _ => None,
                };
                if let Some((path, name)) = entry {
                    copy_artifact(path, name)?;
                }
            }
        }

        // Write the images manifest with the rebased image paths.
        let images_manifest = AssembledSystem {
            images,
            board_name: product_bundle.partitions.hardware_revision.clone(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let images_manifest_path = self.out_dir.join("images.json");
        images_manifest.write_old(images_manifest_path).context("Writing images manifest")?;

        if let Some(path) = self.fastboot {
            let input = File::open(&path).context("Could not read fastboot file")?;

            let mut gz = GzDecoder::new(BufReader::new(&input));
            match gz.header() {
                Some(_) => {
                    // copy gzip file to destination path
                    let destination = self.out_dir.join("fastboot.exe.linux-x64");
                    let mut output = File::create(&destination)
                        .context("Could not create output 'fastboot.exe.linux-x64' file")?;
                    let mut perms = output
                        .metadata()
                        .context("Could not read metadata of 'fastboot.exe.linux-x64'")?
                        .permissions();
                    perms.set_mode(0o755);
                    output
                        .set_permissions(perms)
                        .context("Failed to set permissions on 'fastboot.exe.linux-x64'")?;
                    copy(&mut gz, &mut output)
                        .context("Fail to write to 'fastboot.exe.linux-x64' file")?;
                }
                None => {
                    // copy regular file to destination path
                    copy_artifact(&path, "fastboot.exe.linux-x64")?;
                }
            };
        }

        // Create flash.sh file
        let flash_script_content =
            FLASH_SCRIPT_TEMPLATE.replace("BOOTLOADER_STR", &bootloader_string.trim());
        let flash_script_path = self.out_dir.join("flash.sh");
        std::fs::write(flash_script_path, flash_script_content)
            .context("Failed to write flash.sh")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembly_container::AssemblyContainer;
    use assembly_partitions_config::PartitionsConfig;
    use camino::Utf8Path;
    use product_bundle::ProductBundleV2;
    use serde_json::Value;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_generate_build_archive() {
        let tmp = tempdir().unwrap();
        let tempdir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let json = r#"
            {
                "bootloader_partitions": [
                    {
                        "image": "u-boot.bin.signed.b4",
                        "name": "bootloader",
                        "type": "skip_metadata"
                    }
                ],
                "bootstrap_partitions": [
                    {
                        "condition": {
                            "value": "0xe9000000",
                            "variable": "emmc-total-bytes"
                        },
                        "image": "gpt.fuchsia.3728.bin",
                        "name": "gpt"
                    },
                    {
                        "condition": {
                            "value": "0xec000000",
                            "variable": "emmc-total-bytes"
                        },
                        "image": "gpt.fuchsia.3776.bin",
                        "name": "gpt"
                    }
                ],
                partitions: [
                    {
                        type: "ZBI",
                        name: "zircon_a",
                        slot: "A",
                    },
                    {
                        type: "VBMeta",
                        name: "vbmeta_b",
                        slot: "B",
                    },
                    {
                        type: "FVM",
                        name: "fvm",
                    },
                ],
                hardware_revision: "hw",
                unlock_credentials: [
                    "unlock_creds.zip",
                ],
            }
        "#;
        let partitions_config_path = tempdir.join("partitions_config.json");
        File::create(partitions_config_path.as_path()).unwrap().write_all(json.as_bytes()).unwrap();

        let create_temp_file = |name: &str| {
            let path = tempdir.join(name);
            let mut file = File::create(path).unwrap();
            write!(file, "{}", name).unwrap();
        };

        create_temp_file("unlock_creds.zip");
        create_temp_file("gpt.fuchsia.3728.bin");
        create_temp_file("gpt.fuchsia.3776.bin");
        create_temp_file("u-boot.bin.signed.b4");
        create_temp_file("fuchsia.zbi");
        create_temp_file("fuchsia.vbmeta");
        create_temp_file("fvm.blk");
        create_temp_file("fvm.fastboot.blk");
        create_temp_file("kernel");
        create_temp_file("zedboot.zbi");
        create_temp_file("zedboot.vbmeta");
        create_temp_file("fastboot");

        let config = PartitionsConfig::from_dir(&tempdir).unwrap();

        let pb = ProductBundle::V2(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: config,
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::ZBI { path: tempdir.join("fuchsia.zbi"), signed: false },
                Image::VBMeta(tempdir.join("fuchsia.vbmeta")),
                Image::FVM(tempdir.join("fvm.blk")),
                Image::FVMFastboot(tempdir.join("fvm.fastboot.blk")),
                Image::QemuKernel(tempdir.join("kernel")),
            ]),
            system_b: None,
            system_r: Some(vec![
                Image::ZBI { path: tempdir.join("zedboot.zbi"), signed: false },
                Image::VBMeta(tempdir.join("zedboot.vbmeta")),
            ]),
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        let pb_path = tempdir.join("product_bundle");
        std::fs::create_dir_all(&pb_path).unwrap();
        pb.write(&pb_path).unwrap();

        let ba_path = tempdir.join("build_archive");
        let cmd = GenerateBuildArchive {
            product_bundle: pb_path.clone(),
            out_dir: ba_path.clone(),
            fastboot: Some(tempdir.join("fastboot")),
        };
        cmd.generate().unwrap();

        assert!(ba_path.join("unlock_creds.zip").exists());
        assert!(ba_path.join("gpt.fuchsia.3728.bin").exists());
        assert!(ba_path.join("gpt.fuchsia.3776.bin").exists());
        assert!(ba_path.join("firmware_skip_metadata.img").exists());
        assert!(ba_path.join("zircon-a.zbi").exists());
        assert!(ba_path.join("zircon-a.vbmeta").exists());
        assert!(ba_path.join("fvm.fastboot.blk").exists());
        assert!(ba_path.join("storage-full.blk").exists());
        assert!(ba_path.join("qemu-kernel.kernel").exists());
        assert!(ba_path.join("zircon-r.zbi").exists());
        assert!(ba_path.join("zircon-r.vbmeta").exists());
        assert!(ba_path.join("flash.sh").exists());

        let images_manifest_file = File::open(ba_path.join("images.json")).unwrap();
        let images_manifest: Value = serde_json::from_reader(images_manifest_file).unwrap();
        assert_eq!(
            images_manifest,
            serde_json::from_str::<Value>(
                r#"
            [
                {
                    "name": "zircon-a",
                    "type": "zbi",
                    "path": "zircon-a.zbi",
                    "signed": false
                },
                {
                    "name": "zircon-a",
                    "type": "vbmeta",
                    "path": "zircon-a.vbmeta"
                },
                {
                    "type": "blk",
                    "name": "storage-full",
                    "path": "storage-full.blk"
                },
                {
                    "name": "fvm.fastboot",
                    "path": "fvm.fastboot.blk",
                    "type": "blk"
                },
                {
                    "type": "kernel",
                    "name": "qemu-kernel",
                    "path": "qemu-kernel.kernel"
                }
            ]
            "#
            )
            .unwrap()
        );

        let expected_flash_content = r#"#!/bin/sh
DIR="$(dirname "$0")"
set -e

ZIRCON_IMAGE=zircon-a.zbi
ZIRCON_VBMETA=zircon-a.vbmeta
RECOVERY_IMAGE=zircon-r.zbi
RECOVERY_VBMETA=zircon-r.vbmeta
RECOVERY=
SSH_KEY=

for i in "$@"
do
case $i in
    --recovery)
    RECOVERY=true
    ZIRCON_IMAGE=zircon-r.zbi
    ZIRCON_VBMETA=zircon-r.vbmeta
    shift
    ;;
    --ssh-key=*)
    SSH_KEY="${i#*=}"
    shift
    ;;
    *)
    break
    ;;
esac
done

FASTBOOT_ARGS="$@"
PRODUCT="%PRODUCT_NAME_STRING%"
actual=$("$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} getvar product 2>&1 | grep -i product | head -n1 | cut -d' ' -f2-)
if [[ "${actual}" != "${PRODUCT}" ]]; then
  echo >&2 "Expected device ${PRODUCT} but found ${actual}"
  exit 1
fi

"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash bootloader "${DIR}/firmware_skip_metadata.img"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} reboot bootloader
echo 'Sleeping for 5 seconds for the device to de-enumerate.'
sleep 5

"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash zircon_a "${DIR}/${ZIRCON_IMAGE}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash zircon_b "${DIR}/${ZIRCON_IMAGE}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash zircon_r "${DIR}/${RECOVERY_IMAGE}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash vbmeta_a "${DIR}/${ZIRCON_VBMETA}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash vbmeta_b "${DIR}/${ZIRCON_VBMETA}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash vbmeta_r "${DIR}/${RECOVERY_VBMETA}"
"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} set_active a

if [[ -z "${RECOVERY}" ]]; then
  "$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} flash fvm "${DIR}/fvm.fastboot.blk"
fi

if [[ ! -z "${SSH_KEY}" ]]; then
  is_userspace=$("$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} getvar is-userspace 2>&1 | head -n1 | cut -d' ' -f2-)
  if [[ "${is_userspace}" == "yes" ]]; then
    echo "running in userspace fastboot. rebooting to userspace fastboot"
    "$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} reboot bootloader
    sleep 5
  fi
  "$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} stage "${SSH_KEY}" oem add-staged-bootloader-file ssh.authorized_keys
fi

"$DIR/fastboot.exe.linux-x64" ${FASTBOOT_ARGS} continue
"#;

        let flash_content = std::fs::read_to_string(ba_path.join("flash.sh")).unwrap();
        assert_eq!(expected_flash_content, flash_content);
    }
}
