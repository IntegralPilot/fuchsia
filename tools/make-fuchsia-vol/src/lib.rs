// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow, bail, ensure};
use assembled_system::Image;
use assembly_partitions_config::{Partition, PartitionsConfig, Slot};
use byteorder::{BigEndian, WriteBytesExt};
use camino::{Utf8Path, Utf8PathBuf};
use fatfs::{FsOptions, NullTimeProvider, OemCpConverter, TimeProvider};
use gpt::partition_types::{OperatingSystem, Type as PartType};
use gpt::{DiskDevice, GptDisk};
use product_bundle::{LoadedProductBundle, ProductBundle};
use rand::{Rng as _, SeedableRng as _};
use rand_xorshift::XorShiftRng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::process::Command;
use zerocopy::{Immutable, IntoBytes};

pub mod args;
use args::{Arch, BootPart, TopLevel};

const GUID_BOOTLOADER_STRING: PartType = PartType {
    guid: uuid::uuid!("5ECE94FE-4C86-11E8-A15B-480FCF35F8E6"),
    os: OperatingSystem::None,
};
const GPT_ZIRCON_ABR_TYPE_GUID: PartType = PartType {
    guid: uuid::uuid!("9B37FFF6-2E58-466A-983A-F7926D0B04E0"),
    os: OperatingSystem::None,
};
const GPT_VBMETA_ABR_TYPE_GUID: PartType = PartType {
    guid: uuid::uuid!("421A8BFC-85D9-4D85-ACDA-B64EEC0133E9"),
    os: OperatingSystem::None,
};
const GPT_DURABLE_BOOT_TYPE_GUID: PartType = PartType {
    guid: uuid::uuid!("A409E16B-78AA-4ACC-995C-302352621A41"),
    os: OperatingSystem::None,
};
const INSTALLER_GUID: PartType = PartType {
    guid: uuid::uuid!("4DCE98CE-E77E-45C1-A863-CAF92F1330C1"),
    os: OperatingSystem::None,
};
const GPT_FVM_TYPE_GUID: PartType = PartType {
    guid: uuid::uuid!("49FD7CB8-DF15-4E73-B9D9-992070127F0F"),
    os: OperatingSystem::None,
};

/// On QEMU, the bootloader will look for an EFI application commandline in this partition, to
/// allow tests to control the boot mode.
///
/// The bootloader will match against the partition name; the type GUID doesn't matter.
const QEMU_COMMANDLINE_PARTITION_NAME: &str = "qemu-commandline";
const QEMU_COMMANDLINE_PARTITION_SIZE: u64 = 1024;
const QEMU_COMMANDLINE_PARTITION_GUID: PartType = PartType {
    guid: uuid::uuid!("F5F38420-F848-423C-9945-3D845492A05A"),
    os: OperatingSystem::None,
};

// The relevant part of the product bundle metadata schema, as realized by
// entries in the product_bundles.json build API module.
#[derive(Deserialize, Serialize)]
struct ProductBundleMetadata {
    name: String,
    path: Utf8PathBuf,
}

// Shift takes something that implements Read + Write + Seek and offsets it by `offset`.  This is
// used to allow fatfs to read the ESP partition.  There are no checks to ensure that fatfs doesn't
// escape the partition, but that shouldn't happen.
struct Shift<T> {
    inner: T,
    offset: u64,
}

impl<T: Seek> Shift<T> {
    fn new(mut inner: T, offset: u64) -> Self {
        assert_eq!(inner.seek(SeekFrom::Start(offset)).unwrap(), offset);
        Self { inner, offset }
    }
}

impl<T: Read> Read for Shift<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<T: Write> Write for Shift<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<T: Seek> Seek for Shift<T> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match pos {
            SeekFrom::Start(offset) => {
                Ok(self.inner.seek(SeekFrom::Start(offset + self.offset))? - self.offset)
            }
            SeekFrom::End(_) => panic!("Not supported"),
            SeekFrom::Current(offset) => {
                Ok(self.inner.seek(SeekFrom::Current(offset))? - self.offset)
            }
        }
    }
}

fn get_product_bundle_partitions(path: &Utf8Path) -> Result<PartitionsConfig, Error> {
    let product_bundle = LoadedProductBundle::try_load_from(path)?;

    match product_bundle.into() {
        ProductBundle::V2(pb) => Ok(pb.partitions),
    }
}

fn get_bootloader_partition_name(partitions: &Option<PartitionsConfig>) -> Result<&str, Error> {
    if let Some(partitions) = &partitions {
        match &partitions.bootloader_partitions[..] {
            [bootloader_partition] => {
                if let Some(name) = &bootloader_partition.name {
                    return Ok(name);
                } else {
                    bail!("product bundle bootloader partition doesn't have a name");
                }
            }
            [] => bail!("product bundle doesn't have any bootloader partitions"),
            _ => bail!("product bundles with multiple bootloader partitions is not supported"),
        }
    }

    Ok("bootloader")
}

fn get_zbi_partition_name(
    partitions: &Option<PartitionsConfig>,
    partition_slot: Slot,
) -> Result<&str, Error> {
    let Some(partitions) = partitions else {
        return Ok(match partition_slot {
            Slot::A => "zircon_a",
            Slot::B => "zircon_b",
            Slot::R => "zircon_r",
        });
    };

    for partition in &partitions.partitions {
        if let Partition::ZBI { name, slot, .. } = &partition {
            if slot == &partition_slot {
                return Ok(name);
            }
        }
    }

    bail!("could not find ZBI partition name for slot {partition_slot:?}")
}

fn get_vbmeta_partition_name(
    partitions: &Option<PartitionsConfig>,
    partition_slot: Slot,
) -> Result<&str, Error> {
    let Some(partitions) = partitions else {
        return Ok(match partition_slot {
            Slot::A => "vbmeta_a",
            Slot::B => "vbmeta_b",
            Slot::R => "vbmeta_r",
        });
    };

    for partition in &partitions.partitions {
        if let Partition::VBMeta { name, slot, .. } = &partition {
            if slot == &partition_slot {
                return Ok(name);
            }
        }
    }

    bail!("could not find ZBI partition name for slot {partition_slot:?}")
}

/// Create a protective MBR
///
/// The disk size is the number of logical blocks on the disk.
fn write_protective_mbr<D>(disk: &mut D, disk_size: u64, block_size: u64) -> Result<usize, Error>
where
    D: DiskDevice,
{
    let mbr = gpt::mbr::ProtectiveMBR::with_lb_size(
        // The with_lb_size fn expects the disk size minus 1 block since the protected partition
        // begins at LBA 1, not 0.
        u32::try_from((disk_size - 1) / block_size).unwrap_or(0xffffffff),
    );
    Ok(mbr.overwrite_lba0(disk)?)
}

pub fn run(mut args: TopLevel) -> Result<(), Error> {
    check_args(&mut args)?;

    let product_bundle_partitions = if let Some(pbms_path) = &args.product_bundle {
        Some(get_product_bundle_partitions(pbms_path)?)
    } else {
        None
    };

    let mut disk = OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(args.resize.is_some())
        .create(args.resize.is_some())
        .open(&args.disk_path)
        .context(format!("Failed to open output file {}", args.disk_path))?;

    let disk_size = if let Some(size) = args.resize {
        disk.set_len(size)?;
        size
    } else {
        disk.seek(SeekFrom::End(0))?
    };

    let mut config = gpt::GptConfig::new().writable(true);
    if let Some(block_size) = args.block_size {
        config = config.logical_block_size(block_size.try_into()?);
    }
    let mut gpt_disk = config.create_from_device(Box::new(&mut disk), None)?;
    gpt_disk.update_partitions(BTreeMap::new())?;

    let efi_partition_name = get_bootloader_partition_name(&product_bundle_partitions)?;
    let efi_part =
        add_partition(&mut gpt_disk, efi_partition_name, args.efi_size, GUID_BOOTLOADER_STRING)?;

    // If a QEMU commandline is specified, this tuple will hold (partition_range, contents).
    let qemu_commandline = match args.qemu_commandline {
        Some(ref commandline) => Some((
            add_partition(
                &mut gpt_disk,
                QEMU_COMMANDLINE_PARTITION_NAME,
                QEMU_COMMANDLINE_PARTITION_SIZE,
                QEMU_COMMANDLINE_PARTITION_GUID,
            )?,
            commandline,
        )),
        None => None,
    };

    struct Partitions {
        zircon_a: Range<u64>,
        vbmeta_a: Range<u64>,
        zircon_b: Range<u64>,
        vbmeta_b: Range<u64>,
        zircon_r: Range<u64>,
        vbmeta_r: Range<u64>,
        abr_meta: Range<u64>,
    }

    let abr_partitions = if !args.no_abr {
        Some(Partitions {
            zircon_a: add_partition(
                &mut gpt_disk,
                get_zbi_partition_name(&product_bundle_partitions, Slot::A)?,
                args.abr_size,
                GPT_ZIRCON_ABR_TYPE_GUID,
            )?,
            vbmeta_a: add_partition(
                &mut gpt_disk,
                get_vbmeta_partition_name(&product_bundle_partitions, Slot::A)?,
                args.vbmeta_size,
                GPT_VBMETA_ABR_TYPE_GUID,
            )?,
            zircon_b: add_partition(
                &mut gpt_disk,
                get_zbi_partition_name(&product_bundle_partitions, Slot::B)?,
                args.abr_size,
                GPT_ZIRCON_ABR_TYPE_GUID,
            )?,
            vbmeta_b: add_partition(
                &mut gpt_disk,
                get_vbmeta_partition_name(&product_bundle_partitions, Slot::B)?,
                args.vbmeta_size,
                GPT_VBMETA_ABR_TYPE_GUID,
            )?,
            zircon_r: add_partition(
                &mut gpt_disk,
                get_zbi_partition_name(&product_bundle_partitions, Slot::R)?,
                args.abr_size,
                GPT_VBMETA_ABR_TYPE_GUID,
            )?,
            vbmeta_r: add_partition(
                &mut gpt_disk,
                get_vbmeta_partition_name(&product_bundle_partitions, Slot::R)?,
                args.vbmeta_size,
                GPT_VBMETA_ABR_TYPE_GUID,
            )?,
            abr_meta: add_partition(
                &mut gpt_disk,
                "durable_boot",
                args.vbmeta_size,
                GPT_DURABLE_BOOT_TYPE_GUID,
            )?,
        })
    } else {
        None
    };

    let block_size: u64 = gpt_disk.logical_block_size().clone().into();

    let fvm_part = if !args.ramdisk_only && !args.use_fxfs {
        let avail =
            gpt_disk.find_free_sectors().iter().map(|(_offset, length)| length).max().unwrap()
                * block_size;
        let size = std::cmp::min(avail, args.system_disk_size.unwrap_or(avail));
        if args.use_sparse_fvm {
            Some(add_partition(&mut gpt_disk, "storage-sparse", size, INSTALLER_GUID)?)
        } else {
            Some(add_partition(&mut gpt_disk, "fvm", size, GPT_FVM_TYPE_GUID)?)
        }
    } else {
        None
    };
    let fxfs_part = if args.fxfs.is_some() {
        assert!(fvm_part.is_none(), "Can't have both FVM and Fxfs");
        let size = args.system_disk_size.unwrap_or_else(|| {
            gpt_disk.find_free_sectors().iter().map(|(_offset, length)| length).max().unwrap()
                * block_size
        });
        // For now, we use the same name and type as FVM because the paver looks for this.
        Some(add_partition(&mut gpt_disk, "fvm", size, GPT_FVM_TYPE_GUID)?)
    } else {
        None
    };

    if let Some(seed) = &args.seed {
        let mut seed_bytes = [0; 16];
        for chunk in seed.as_bytes().chunks(16) {
            seed_bytes.iter_mut().zip(chunk.iter()).for_each(|(x, b)| *x ^= *b);
        }
        let mut rng = XorShiftRng::from_seed(seed_bytes);
        let mut bytes = [0; 16];

        rng.fill_bytes(&mut bytes);
        let uuid = uuid::Builder::from_bytes(bytes)
            .with_variant(uuid::Variant::RFC4122)
            .with_version(uuid::Version::Random)
            .into_uuid();

        gpt_disk.update_guid(Some(uuid));

        let mut partitions = gpt_disk.partitions().clone();
        for (_id, partition) in &mut partitions {
            rng.fill_bytes(&mut bytes);
            let uuid = uuid::Builder::from_bytes(bytes)
                .with_variant(uuid::Variant::RFC4122)
                .with_version(uuid::Version::Random)
                .into_uuid();
            partition.part_guid = uuid;
        }
        gpt_disk.update_partitions(partitions)?;
    }

    gpt_disk.write()?;

    write_protective_mbr(&mut disk, disk_size, block_size)?;

    let search_path = if let Some(build_dir) = &args.fuchsia_build_dir {
        // Use tools from the build directory over $PATH.
        let host_path = match args.host_arch {
            Arch::X64 => "host_x64",
            Arch::Arm64 => "host_arm64",
        };
        format!("{}/{}:{}", build_dir, host_path, std::env::var("PATH").unwrap_or(String::new()))
    } else {
        String::new()
    };

    let mut command = if let Some(p) = &args.mkfs_msdosfs {
        Command::new(p)
    } else {
        Command::new("mkfs-msdosfs")
    };

    // If a seed is provided, use a fixed timestamp.
    if args.seed.is_some() {
        command.arg("-T").arg("1661998826");
    }

    let output = command
        .arg("-@")
        .arg(format!("{}", efi_part.start))
        .arg("-S")
        .arg(format!("{}", args.efi_size + efi_part.start))
        .arg("-F")
        .arg("32")
        .arg("-L")
        .arg("ESP")
        .arg("-O")
        .arg("Fuchsia")
        .arg("-b")
        .arg(format!("{}", block_size))
        .arg(&args.disk_path)
        .env("PATH", &search_path)
        .output()
        .context("Failed to run mkfs-msdosfs")?;

    if !output.status.success() {
        bail!(
            "mkfs-msdosfs failed, stdout={}, stderr={}",
            std::str::from_utf8(&output.stdout).unwrap(),
            std::str::from_utf8(&output.stderr).unwrap()
        );
    }

    if args.seed.is_some() {
        write_esp_content(
            Shift::new(&mut disk, efi_part.start),
            &args,
            FsOptions::new().time_provider(NullTimeProvider::new()),
        )?;
    } else {
        write_esp_content(Shift::new(&mut disk, efi_part.start), &args, FsOptions::new())?;
    }

    if let Some((partition, contents)) = qemu_commandline {
        // Bootloader expects a nul-terminated string.
        let contents = CString::new(contents.as_bytes())?;
        let contents = contents.as_bytes_with_nul();
        let part_size = partition.end - partition.start;
        if contents.len() > part_size.try_into()? {
            bail!("QEMU commandline contents too large (max {})", part_size);
        }
        disk.write_all_at(contents, partition.start)?;
    }

    if let Some(partitions) = abr_partitions {
        copy_partition(&mut disk, partitions.zircon_a, &args.zircon_a.unwrap())?;
        copy_partition(&mut disk, partitions.vbmeta_a, &args.vbmeta_a.unwrap())?;
        copy_partition(&mut disk, partitions.zircon_b, &args.zircon_b.unwrap())?;
        copy_partition(&mut disk, partitions.vbmeta_b, &args.vbmeta_b.unwrap())?;
        copy_partition(&mut disk, partitions.zircon_r, &args.zircon_r.unwrap())?;
        copy_partition(&mut disk, partitions.vbmeta_r, &args.vbmeta_r.unwrap())?;
        write_abr(&mut disk, partitions.abr_meta.start, args.abr_boot)?;
    }

    if let Some(fvm_part) = fvm_part {
        if args.verbose {
            println!("Populating FVM in GPT image");
        }
        if args.use_sparse_fvm {
            copy_partition(&mut disk, fvm_part, &args.sparse_fvm.unwrap())?;
        } else {
            let status = Command::new("fvm")
                .arg(&args.disk_path)
                .arg("create")
                .arg("--offset")
                .arg(format!("{}", fvm_part.start))
                .arg("--length")
                .arg(format!("{}", fvm_part.end - fvm_part.start))
                .arg("--slice")
                .arg("8388608")
                .arg("--blob")
                .arg(args.blob.unwrap())
                .arg("--with-empty-minfs")
                .env("PATH", &search_path)
                .status()
                .context("Failed to run fvm tool")?;
            ensure!(status.success(), "fvm tool failed");
        }
    }

    if let Some(fxfs_part) = fxfs_part {
        copy_partition(&mut disk, fxfs_part, &args.fxfs.unwrap())?;
    }

    Ok(())
}

fn check_args(args: &mut TopLevel) -> Result<(), Error> {
    if args.ramdisk_only {
        if args.blob.is_some() || args.sparse_fvm.is_some() || args.use_sparse_fvm {
            bail!(
                "--ramdisk_only incompatible with --blob, --sparse-fvm and \
                   --use-sparse-fvm"
            );
        }
    } else if (args.blob.is_some()) && (args.sparse_fvm.is_some() || args.use_sparse_fvm) {
        bail!("--blob incompatbile with --use-sparse-fvm|--sparse-fvm");
    }

    if args.sparse_fvm.is_some() {
        args.use_sparse_fvm = true
    }

    if args.fuchsia_build_dir.is_none() {
        if let Ok(build_dir) = std::env::var("FUCHSIA_BUILD_DIR") {
            args.fuchsia_build_dir = Some(build_dir.into())
        }
    }

    let mut dependencies = vec![];

    if args.product_bundle.is_none() {
        args.product_bundle = get_build_product_bundle_path(args, &mut dependencies)?;
    };

    let product_bundle_path = if let Some(product_bundle_dir) = &args.product_bundle {
        Some(product_bundle_dir.join("product_bundle.json"))
    } else {
        None
    };
    // This is a separate statement so that 'product_bundle_path' lives long enough to provide a
    // value that can be referenced by 'dependencies' until the end of the fn.
    if let Some(path) = &product_bundle_path {
        dependencies.push(path.as_path());
    }

    // The order of precedence for file paths is as follows:
    //
    //  1) --zircon_[a|b], --vbmeta_[a|b], and fvm/fxfs image args first.
    //  2) --zbi, for both zircon_a and zircon_b.
    //  3) the images in the product bundle, and if using fxfs:
    //      a)  the sparse image
    //      b)  the full image (as it's being removed as an output)

    // Apply the --zbi argument to the zircon_[a|b] args, if it's been specified.
    if let Some(path) = &args.zbi {
        args.zircon_a.get_or_insert_with(|| path.clone());
        args.zircon_b.get_or_insert_with(|| path.clone());
    }

    // If there's a product bundle, use that for all remaining, unset, image paths.
    if let Some(product_bundle_dir) = &args.product_bundle {
        match LoadedProductBundle::try_load_from(product_bundle_dir)?.into() {
            ProductBundle::V2(product_bundle) => {
                // If the bundle contains images for slot b, use them for slot b args that haven't
                // been provided.
                if let Some(system_b) = product_bundle.system_b {
                    for image in &system_b {
                        match image {
                            Image::ZBI { path, .. } => {
                                args.zircon_b.get_or_insert_with(|| path.clone());
                            }
                            Image::VBMeta(utf8_path_buf) => {
                                args.vbmeta_b.get_or_insert_with(|| utf8_path_buf.clone());
                            }
                            _ => {}
                        }
                    }
                }

                // If the bundle contains images for slot a, use them for for slot a AND slot b args
                // that haven't been provided.
                if let Some(system_a) = product_bundle.system_a {
                    for image in &system_a {
                        match image {
                            Image::ZBI { path, .. } => {
                                args.zbi.get_or_insert_with(|| path.clone());
                                args.zircon_a.get_or_insert_with(|| path.clone());
                                args.zircon_b.get_or_insert_with(|| path.clone());
                            }
                            Image::VBMeta(utf8_path_buf) => {
                                args.vbmeta_a.get_or_insert_with(|| utf8_path_buf.clone());
                                args.vbmeta_b.get_or_insert_with(|| utf8_path_buf.clone());
                            }

                            // Always set these paths if present in the product bundle, but they
                            // won't be used if args.ramdisk_only is true.
                            Image::FVMSparse(utf8_path_buf) => {
                                args.sparse_fvm.get_or_insert_with(|| utf8_path_buf.clone());
                            }
                            Image::BlobFS { path, .. } => {
                                args.blob.get_or_insert_with(|| path.clone());
                            }
                            Image::FxfsSparse { path, .. } => {
                                if args.use_fxfs {
                                    args.fxfs.get_or_insert_with(|| path.clone());
                                }
                            }
                            _ => {}
                        }
                    }

                    // fall back to the full (non-sparse) Fxfs image if we didn't find a sparse one.
                    if args.use_fxfs && args.fxfs.is_none() {
                        args.fxfs = system_a.iter().find_map(|i| match i {
                            Image::Fxfs(path) => Some(path.clone()),
                            _ => None,
                        });
                    }
                }

                // And the recovery zbi
                if let Some(system_r) = product_bundle.system_r {
                    for image in &system_r {
                        match image {
                            Image::ZBI { path, .. } => {
                                args.zedboot.get_or_insert_with(|| path.clone());
                                args.zircon_r.get_or_insert_with(|| path.clone());
                            }
                            Image::VBMeta(utf8_path_buf) => {
                                args.vbmeta_r.get_or_insert_with(|| utf8_path_buf.clone());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    };

    if args.bootloader.is_none() {
        if let Some(build_dir) = &args.fuchsia_build_dir {
            let bootloader_dir = match args.arch {
                Arch::X64 => build_dir.join("kernel.efi_x64"),
                Arch::Arm64 => build_dir.join("kernel.efi_arm64"),
            };
            let candidate = bootloader_dir.join("fuchsia-efi.efi");
            if candidate.exists() || args.product_bundle.is_none() {
                args.bootloader = Some(candidate);
            }
        } else if args.product_bundle.is_none() {
            bail!("Missing --bootloader");
        }
    }

    if let Some(bootloader) = &args.bootloader {
        dependencies.push(bootloader);
    }

    ensure!(args.zbi.is_some(), "Missing --zbi");
    dependencies.push(args.zbi.as_ref().unwrap());
    ensure!(args.zedboot.is_some(), "Missing --zedboot");
    dependencies.push(args.zedboot.as_ref().unwrap());

    if !args.no_abr {
        ensure!(args.zircon_a.is_some(), "Missing --zircon_a");
        dependencies.push(args.zircon_a.as_ref().unwrap());
        ensure!(args.vbmeta_a.is_some(), "Missing --vbmeta_a");
        dependencies.push(args.vbmeta_a.as_ref().unwrap());
        ensure!(args.zircon_b.is_some(), "Missing --zircon_b");
        dependencies.push(args.zircon_b.as_ref().unwrap());
        ensure!(args.vbmeta_b.is_some(), "Missing --vbmeta_b");
        dependencies.push(args.vbmeta_b.as_ref().unwrap());
        ensure!(args.zircon_r.is_some(), "Missing --zircon_r");
        dependencies.push(args.zircon_r.as_ref().unwrap());
        ensure!(args.vbmeta_r.is_some(), "Missing --vbmeta_r");
        dependencies.push(args.vbmeta_r.as_ref().unwrap());
    }

    if !args.ramdisk_only {
        if args.use_sparse_fvm {
            ensure!(args.sparse_fvm.is_some(), "Missing --sparse-fvm");
            dependencies.push(args.sparse_fvm.as_ref().unwrap());
        } else if args.use_fxfs {
            ensure!(args.fxfs.is_some(), "Missing --fxfs");
            dependencies.push(args.fxfs.as_ref().unwrap());
        } else {
            ensure!(args.blob.is_some(), "Missing --blob");
            dependencies.push(args.blob.as_ref().unwrap());
        }
    }

    if args.depfile {
        // Write a dependency file
        // The output file and dependencies needs to be relative to the build dir.
        let mut disk_path = args.disk_path.as_path();
        if let Some(build_dir) = &args.fuchsia_build_dir {
            if let Ok(dir) = disk_path.strip_prefix(build_dir) {
                disk_path = dir;
            }

            for dep in dependencies.iter_mut() {
                if let Ok(d) = dep.strip_prefix(build_dir) {
                    *dep = d;
                }
            }
        }
        let depfile = format!("{}.d", args.disk_path);
        std::fs::write(
            &depfile,
            format!(
                "{}: {}",
                disk_path,
                dependencies.iter().map(|dep| dep.as_str()).collect::<Vec<_>>().join(" ")
            ),
        )
        .context(format!("Failed to write {}", depfile))?;
    }

    Ok(())
}

fn get_build_product_bundle_path(
    args: &TopLevel,
    dependencies: &mut Vec<&Utf8Path>,
) -> Result<Option<Utf8PathBuf>, Error> {
    let Some(build_dir) = &args.fuchsia_build_dir else { return Ok(None) };
    let Some(name) = &args.product_bundle_name else { return Ok(None) };

    dependencies.push(Utf8Path::new("product_bundles.json"));
    let product_bundles_path = &build_dir.join(dependencies.last().unwrap());
    let f = match File::open(product_bundles_path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            bail!("{product_bundles_path} not found");
        }
        Err(err) => {
            return Err(err.into());
        }
    };

    let product_bundles: Vec<ProductBundleMetadata> = serde_json::from_reader(BufReader::new(f))?;
    match product_bundles.iter().find(|&pb| &pb.name == name) {
        None => Ok(None),
        Some(pb) => Ok(Some(build_dir.join(&pb.path))),
    }
}

fn read_file(path: impl AsRef<std::path::Path>) -> Result<Vec<u8>, Error> {
    std::fs::read(&path).context(format!("Failed to read {}", path.as_ref().display()))
}

// Adds a partition and returns the partition byte range.
fn add_partition<D: DiskDevice>(
    disk: &mut GptDisk<D>,
    name: &str,
    size: u64,
    part_type: PartType,
) -> Result<Range<u64>, Error> {
    let part_id = disk
        .add_partition(name, size, part_type, 0, None)
        .context(format!("Failed to add {} partition (size={})", name, size))?;
    Ok(part_range(disk, part_id))
}

fn read_bootloader_from_product_bundle(
    product_bundle_dir: &Utf8Path,
    bootloader_name: &str,
) -> Result<Vec<u8>, Error> {
    let ProductBundle::V2(pb) = LoadedProductBundle::try_load_from(product_bundle_dir)?.into();
    if pb.partitions.bootloader_partitions.is_empty() {
        bail!("product bundle has no bootloader partitions");
    }
    for bootloader_part in &pb.partitions.bootloader_partitions {
        let Ok(esp_file) = File::open(&bootloader_part.image) else {
            continue;
        };
        let Ok(fs) = fatfs::FileSystem::new(esp_file, FsOptions::new()) else {
            continue;
        };
        if let Ok(mut file) = fs.root_dir().open_file(&format!("EFI/BOOT/{}", bootloader_name)) {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    bail!("Could not find EFI/BOOT/{} in any bootloader partition", bootloader_name);
}

fn write_esp_content<TP: TimeProvider, OCC: OemCpConverter>(
    part: impl Read + Write + Seek,
    args: &TopLevel,
    options: FsOptions<TP, OCC>,
) -> Result<(), Error> {
    // If a seed is provided, all timestamps used will be the epoch.
    let fs = fatfs::FileSystem::new(part, options)?;

    {
        let root_dir = fs.root_dir();

        let bootloader_name = match args.arch {
            Arch::X64 => "bootx64.efi",
            Arch::Arm64 => "bootaa64.efi",
        };

        let bootloader_bytes = if let Some(bootloader) = &args.bootloader {
            read_file(bootloader)?
        } else if let Some(product_bundle_dir) = &args.product_bundle {
            read_bootloader_from_product_bundle(product_bundle_dir, bootloader_name)?
        } else {
            bail!("Missing --bootloader");
        };

        root_dir.create_dir("EFI")?;
        root_dir.create_dir("EFI/Google")?;
        root_dir.create_dir("EFI/Google/GSetup")?;
        root_dir
            .create_file("EFI/Google/GSetup/Boot")?
            .write_all(format!("efi\\boot\\{}", bootloader_name).as_bytes())?;

        root_dir.create_dir("EFI/BOOT")?;
        root_dir
            .create_file(&format!("EFI/BOOT/{}", bootloader_name))?
            .write_all(&bootloader_bytes)?;

        if args.no_abr {
            root_dir
                .create_file("zircon.bin")?
                .write_all(&read_file(args.zbi.as_ref().unwrap())?)?;
            root_dir
                .create_file("zedboot.bin")?
                .write_all(&read_file(args.zedboot.as_ref().unwrap())?)?;
        }

        if let Some(cmdline) = &args.cmdline {
            root_dir.create_file("cmdline")?.write_all(&read_file(cmdline)?)?;
        }
    }

    fs.unmount()?;

    Ok(())
}

// Copies the file at `source` path to the disk. `range` is the partition byte range.
fn copy_partition(disk: &mut File, range: Range<u64>, source: &Utf8Path) -> Result<(), Error> {
    let source_file = std::fs::File::open(source).context(format!("Failed to read {}", source))?;
    let mut reader = std::io::BufReader::new(source_file);

    let contents = if sparse::is_sparse_image(&mut reader) {
        let mut writer = std::io::Cursor::new(Vec::new());
        sparse::unsparse(&mut reader, &mut writer)
            .map_err(|e| anyhow!("cannot inflate sparse image: {e}"))?;
        writer.into_inner()
    } else {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map_err(|e| anyhow!("cannot read image: {e}"))?;
        buf
    };

    let max_len = (range.end - range.start) as usize;
    let contents = if contents.len() > max_len {
        println!("{} too big", source);
        &contents[..max_len]
    } else {
        &contents
    };
    disk.write_all_at(contents, range.start)?;
    Ok(())
}

// Returns the partition range given a partition ID.
fn part_range<D: DiskDevice>(disk: &GptDisk<D>, part_id: u32) -> Range<u64> {
    let lbs = u64::from(disk.logical_block_size().clone());
    let part = &disk.partitions()[&part_id];
    part.first_lba * lbs..(part.last_lba + 1) * lbs
}

// Routines for writing the ABR.
const MAX_TRIES: u8 = 7;
const MAX_PRIORITY: u8 = 15;

#[derive(IntoBytes, Immutable)]
#[repr(C, packed)]
#[derive(Default)]
struct AbrData {
    magic: [u8; 4],
    version_major: u8,
    version_minor: u8,
    reserved1: u16,
    slot_data: [AbrSlotData; 2],
    one_shot_recovery_boot: u8,
    reserved2: [u8; 11],
    // A CRC32 comes next.
}

#[derive(IntoBytes, Immutable)]
#[repr(C, packed)]
#[derive(Default)]
struct AbrSlotData {
    priority: u8,
    tries_remaining: u8,
    successful_boot: u8,
    reserved: u8,
}

impl AbrSlotData {
    fn with_priority(priority: u8) -> AbrSlotData {
        AbrSlotData { tries_remaining: MAX_TRIES, priority, ..Default::default() }
    }
}

// Writes the ABR to the disk at `offset`.
fn write_abr(disk: &mut File, offset: u64, boot_part: BootPart) -> Result<(), Error> {
    let data = AbrData {
        magic: b"\0AB0".clone(),
        version_major: 2,
        version_minor: 1,
        slot_data: match boot_part {
            BootPart::BootA => {
                [AbrSlotData::with_priority(MAX_PRIORITY), AbrSlotData::with_priority(1)]
            }
            BootPart::BootB => {
                [AbrSlotData::with_priority(1), AbrSlotData::with_priority(MAX_PRIORITY)]
            }
            BootPart::BootR => [AbrSlotData::with_priority(0), AbrSlotData::with_priority(0)],
        },
        ..Default::default()
    };

    disk.seek(SeekFrom::Start(offset))?;
    disk.write_all(data.as_bytes())?;
    disk.write_u32::<BigEndian>(
        crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(data.as_bytes()),
    )?;
    Ok(())
}

/// Sets up a Fuchsia disk with the required partitions and an imaged UEFI partition.
pub fn make_empty_disk_with_uefi(disk_path: &Path, efi_data: &[u8]) -> anyhow::Result<()> {
    // TODO(http://b/369045905): Some of this can probably be deduplicated with run()
    const EFI_SIZE: u64 = 63 * 1024 * 1024;
    const VBMETA_SIZE: u64 = 64 * 1024;
    const ABR_SIZE: u64 = 256 * 1024 * 1024;

    // For x64, the FVM size is hardcoded to 16GB.
    // If the partition size is less than 16GB, it fails to mount.
    const DISK_SIZE: u64 = 20 * 1024 * 1024 * 1024;

    let mut disk = OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(true)
        .create(true)
        .open(disk_path)
        .context(format!("Failed to open output file {}", disk_path.display()))?;

    disk.set_len(DISK_SIZE)?;

    let config = gpt::GptConfig::new().writable(true);
    let mut gpt_disk = config.create_from_device(Box::new(&mut disk), None)?;
    gpt_disk.update_partitions(std::collections::BTreeMap::new())?;

    struct Partitions {
        efi: Range<u64>,
        _zircon_a: Range<u64>,
        _vbmeta_a: Range<u64>,
        _zircon_b: Range<u64>,
        _vbmeta_b: Range<u64>,
        _zircon_r: Range<u64>,
        _vbmeta_r: Range<u64>,
        _abr_meta: Range<u64>,
    }

    let part = Partitions {
        efi: add_partition(&mut gpt_disk, "bootloader", EFI_SIZE, GUID_BOOTLOADER_STRING)?,
        _zircon_a: add_partition(&mut gpt_disk, "zircon_a", ABR_SIZE, GPT_ZIRCON_ABR_TYPE_GUID)?,
        _vbmeta_a: add_partition(&mut gpt_disk, "vbmeta_a", VBMETA_SIZE, GPT_VBMETA_ABR_TYPE_GUID)?,
        _zircon_b: add_partition(&mut gpt_disk, "zircon_b", ABR_SIZE, GPT_ZIRCON_ABR_TYPE_GUID)?,
        _vbmeta_b: add_partition(&mut gpt_disk, "vbmeta_b", VBMETA_SIZE, GPT_VBMETA_ABR_TYPE_GUID)?,
        _zircon_r: add_partition(&mut gpt_disk, "zircon_r", ABR_SIZE, GPT_VBMETA_ABR_TYPE_GUID)?,
        _vbmeta_r: add_partition(&mut gpt_disk, "vbmeta_r", VBMETA_SIZE, GPT_VBMETA_ABR_TYPE_GUID)?,
        _abr_meta: add_partition(
            &mut gpt_disk,
            "durable_boot",
            VBMETA_SIZE,
            GPT_DURABLE_BOOT_TYPE_GUID,
        )?,
    };

    let block_size: u64 = gpt_disk.logical_block_size().clone().into();

    let fvm_size =
        gpt_disk.find_free_sectors().iter().map(|(_offset, length)| length).max().unwrap()
            * block_size;

    let _fvm_part = add_partition(&mut gpt_disk, "fvm", fvm_size, GPT_FVM_TYPE_GUID)?;

    gpt_disk.write()?;

    write_protective_mbr(&mut disk, DISK_SIZE, block_size)?;

    disk.write_all_at(efi_data, part.efi.start)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use argh::FromArgs;
    use gpt::disk::read_disk;
    use std::str::from_utf8;
    use tempfile::TempDir;

    fn compare_golden(test_data_dir: &Utf8Path, image_path: &Utf8Path) {
        let image = std::fs::read(&image_path).expect("Unable to read image");
        let file =
            std::fs::File::open(test_data_dir.join("golden")).expect("Unable to read golden");
        let golden = zstd::decode_all(file).expect("Unable to decompress");

        // If this fails, here are some tips for debugging:
        //
        // Create a loopback device:
        //
        //   sudo losetup -fP /tmp/image
        //
        // Inspect the partition map:
        //
        //   fdisk /tmp/image
        //
        // Verify the ESP partition:
        //
        //   sudo fsck sudo fsck /dev/loop0p1
        //
        // Mount the ESP partition:
        //
        //   mkdir /tmp/esp
        //   sudo mount -t vfat -o loop /dev/loop0p1 /tmp/esp
        //
        // To overwrite the golden image:
        //
        //   zstd /tmp/image -o golden
        //
        // View the contents of one of the partitions e.g. the AbrMeta partition:
        //
        //   sudo dd if=/dev/loop0p8 | hexdump -C
        assert!(image == golden);
    }

    #[test]
    fn test_with_product_bundle_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        for i in 0..11 {
            std::fs::write(dir.join(format!("placeholder.{}", i)), vec![i; 8192]).unwrap();
        }

        let current_exe = std::env::current_exe().unwrap();

        let test_data_dir = Utf8PathBuf::from_path_buf(
            current_exe.parent().unwrap().join("make-fuchsia-vol_test_data"),
        )
        .unwrap();

        {
            let f = File::create(test_data_dir.join("product_bundles.json")).unwrap();
            let pbs: [ProductBundleMetadata; 0] = [];
            serde_json::to_writer(&f, &pbs).unwrap();
        }

        const IMAGE_SIZE: usize = 67108864;

        let image_path = dir.join("image");
        run(TopLevel::from_args(
            &["make-fuchsia-vol"],
            &[
                image_path.as_str(),
                "--abr-size",
                "8192",
                "--resize",
                &format!("{}", IMAGE_SIZE),
                "--seed",
                "test_compare_golden",
                "--efi-size",
                "40000000",
                "--bootloader",
                dir.join("placeholder.0").as_str(),
                "--zbi",
                dir.join("placeholder.1").as_str(),
                "--zedboot",
                dir.join("placeholder.2").as_str(),
                "--cmdline",
                dir.join("placeholder.3").as_str(),
                "--sparse-fvm",
                dir.join("placeholder.4").as_str(),
                "--zircon-a",
                dir.join("placeholder.5").as_str(),
                "--vbmeta-a",
                dir.join("placeholder.6").as_str(),
                "--zircon-b",
                dir.join("placeholder.7").as_str(),
                "--vbmeta-b",
                dir.join("placeholder.8").as_str(),
                "--zircon-r",
                dir.join("placeholder.9").as_str(),
                "--vbmeta-r",
                dir.join("placeholder.10").as_str(),
                "--fuchsia-build-dir",
                test_data_dir.as_str(),
                "--product-bundle-name",
                "my-product-bundle",
            ],
        )
        .unwrap())
        .expect("run failed");

        compare_golden(&test_data_dir, &image_path);
    }

    #[test]
    fn test_with_product_bundle_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        for i in 0..11 {
            std::fs::write(dir.join(format!("placeholder.{}", i)), vec![i; 8192]).unwrap();
        }

        let current_exe = std::env::current_exe().unwrap();

        let test_data_dir = Utf8PathBuf::from_path_buf(
            current_exe.parent().unwrap().join("make-fuchsia-vol_test_data"),
        )
        .unwrap();

        const IMAGE_SIZE: usize = 67108864;

        let image_path = dir.join("image");
        run(TopLevel::from_args(
            &["make-fuchsia-vol"],
            &[
                image_path.as_str(),
                "--abr-size",
                "8192",
                "--resize",
                &format!("{}", IMAGE_SIZE),
                "--seed",
                "test_compare_golden",
                "--efi-size",
                "40000000",
                "--bootloader",
                dir.join("placeholder.0").as_str(),
                "--zbi",
                dir.join("placeholder.1").as_str(),
                "--zedboot",
                dir.join("placeholder.2").as_str(),
                "--cmdline",
                dir.join("placeholder.3").as_str(),
                "--sparse-fvm",
                dir.join("placeholder.4").as_str(),
                "--zircon-a",
                dir.join("placeholder.5").as_str(),
                "--vbmeta-a",
                dir.join("placeholder.6").as_str(),
                "--zircon-b",
                dir.join("placeholder.7").as_str(),
                "--vbmeta-b",
                dir.join("placeholder.8").as_str(),
                "--zircon-r",
                dir.join("placeholder.9").as_str(),
                "--vbmeta-r",
                dir.join("placeholder.10").as_str(),
                "--fuchsia-build-dir",
                test_data_dir.as_str(),
                "--product-bundle",
                test_data_dir.as_str(),
            ],
        )
        .unwrap())
        .expect("run failed");

        compare_golden(&test_data_dir, &image_path);
    }

    /// Calls `make-fuchsia-vol` with common args plus a QEMU commandline string.
    ///
    /// Returns a (temp_directory, image_path) tuple on success.
    fn make_vol_with_commandline(commandline: &str) -> Result<(TempDir, Utf8PathBuf), Error> {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let image_path = dir.join("image");

        for i in 0..11 {
            std::fs::write(dir.join(format!("placeholder.{}", i)), vec![i; 8192]).unwrap();
        }

        let current_exe = std::env::current_exe().unwrap();

        let test_data_dir = Utf8PathBuf::from_path_buf(
            current_exe.parent().unwrap().join("make-fuchsia-vol_test_data"),
        )
        .unwrap();

        const IMAGE_SIZE: usize = 67108864;

        // Same args as the above tests, but with `--qemu-commandline` also.
        run(TopLevel::from_args(
            &["make-fuchsia-vol"],
            &[
                image_path.as_str(),
                "--abr-size",
                "8192",
                "--resize",
                &format!("{}", IMAGE_SIZE),
                "--seed",
                "test_compare_golden",
                "--efi-size",
                "40000000",
                "--bootloader",
                dir.join("placeholder.0").as_str(),
                "--zbi",
                dir.join("placeholder.1").as_str(),
                "--zedboot",
                dir.join("placeholder.2").as_str(),
                "--cmdline",
                dir.join("placeholder.3").as_str(),
                "--sparse-fvm",
                dir.join("placeholder.4").as_str(),
                "--zircon-a",
                dir.join("placeholder.5").as_str(),
                "--vbmeta-a",
                dir.join("placeholder.6").as_str(),
                "--zircon-b",
                dir.join("placeholder.7").as_str(),
                "--vbmeta-b",
                dir.join("placeholder.8").as_str(),
                "--zircon-r",
                dir.join("placeholder.9").as_str(),
                "--vbmeta-r",
                dir.join("placeholder.10").as_str(),
                "--fuchsia-build-dir",
                test_data_dir.as_str(),
                "--product-bundle",
                test_data_dir.as_str(),
                "--qemu-commandline",
                commandline,
            ],
        )
        .unwrap())?;

        Ok((tmp, image_path))
    }

    #[test]
    fn test_qemu_commandline() {
        let (_tmp, image_path) = make_vol_with_commandline("boot_mode=fastboot").unwrap();

        // Find the partition location from the GPT.
        let disk = read_disk(&image_path).unwrap();
        let partition = disk
            .partitions()
            .iter()
            .find(|p| p.1.name == QEMU_COMMANDLINE_PARTITION_NAME)
            .unwrap()
            .1;

        // Make sure the partition contains our expected contents.
        let file = File::open(&image_path).unwrap();
        let mut buffer = [0u8; 19];
        assert!(
            file.read_exact_at(
                &mut buffer,
                partition.bytes_start(*disk.logical_block_size()).unwrap()
            )
            .is_ok()
        );
        assert_eq!(&buffer, b"boot_mode=fastboot\0");
    }

    #[test]
    fn test_qemu_commandline_too_long() {
        // The commandline plus a nul-terminator needs to fit in the partition, so trying to
        // provide contents equal to the partition size should fail.
        let too_long_commandline = vec![b'a'; QEMU_COMMANDLINE_PARTITION_SIZE as usize];
        assert!(make_vol_with_commandline(from_utf8(&too_long_commandline).unwrap()).is_err());
    }
}
