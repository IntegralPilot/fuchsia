// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BootloaderType;
use anyhow::{Context as _, Error};
use block_client::{BlockClient, MutableBufferSlice, RemoteBlockClient};
use fidl::endpoints::Proxy;
use fidl_fuchsia_mem::Buffer;
use fidl_fuchsia_paver::{Asset, Configuration, DynamicDataSinkProxy};
use fidl_fuchsia_storage_block::{BlockMarker, BlockProxy};

use recovery_util_block::BlockDevice;
use std::cmp::min;
use std::fmt;

#[derive(Debug, PartialEq)]
pub enum PartitionPaveType {
    Asset { r#type: Asset, config: Configuration },
    Volume,
    Bootloader,
}

/// Represents a partition that will be paved to the disk.
pub struct Partition {
    pave_type: PartitionPaveType,
    src: String,
    size: u64,
    block_size: u64,
}

/// This GUID is used by the installer to identify partitions that contain
/// data that will be installed to disk. The `fx mkinstaller` tool generates
/// images containing partitions with this GUID.
static WORKSTATION_INSTALLER_GPT: [u8; 16] = [
    0xce, 0x98, 0xce, 0x4d, 0x7e, 0xe7, 0xc1, 0x45, 0xa8, 0x63, 0xca, 0xf9, 0x2f, 0x13, 0x30, 0xc1,
];

/// These GUIDs are used by the installer to identify partitions that contain
/// data that will be installed to disk from a usb disk. The `fx make-fuchsia-vol`
/// tool generates images containing partitions with these GUIDs.
static WORKSTATION_PARTITION_GPTS: [[u8; 16]; 5] = [
    [
        0xfe, 0x94, 0xce, 0x5e, 0x86, 0x4c, 0xe8, 0x11, 0xa1, 0x5b, 0x48, 0x0f, 0xcf, 0x35, 0xf8,
        0xe6,
    ], // bootloader
    [
        0x6b, 0xe1, 0x09, 0xa4, 0xaa, 0x78, 0xcc, 0x4a, 0x5c, 0x99, 0x41, 0x1a, 0x62, 0x52, 0x23,
        0x30,
    ], // durable_boot
    [
        0xf6, 0xff, 0x37, 0x9b, 0x58, 0x2e, 0x6a, 0x46, 0x3a, 0x98, 0xe0, 0x04, 0x0b, 0x6d, 0x92,
        0xf7,
    ], // zircon_a
    [
        0xf6, 0xff, 0x37, 0x9b, 0x58, 0x2e, 0x6a, 0x46, 0x3a, 0x98, 0xe0, 0x04, 0x0b, 0x6d, 0x92,
        0xf7,
    ], // zircon_b
    [
        0xf6, 0xff, 0x37, 0x9b, 0x58, 0x2e, 0x6a, 0x46, 0x3a, 0x98, 0xe0, 0x04, 0x0b, 0x6d, 0x92,
        0xf7,
    ], // zircon_r
];

impl Partition {
    /// Creates a new partition. Returns `None` if the partition is not
    /// a partition that should be paved to the disk.
    ///
    /// # Arguments
    /// * `src` - path to a block device that represents this partition.
    /// * `part` - a |BlockProxy| that is connected to this partition.
    /// * `bootloader` - the |BootloaderType| of this device.
    ///
    async fn new(
        src: String,
        part: BlockProxy,
        bootloader: BootloaderType,
    ) -> Result<Option<Self>, Error> {
        let (status, guid) = part.get_type_guid().await.context("Get type guid failed")?;
        if let None = guid {
            return Err(Error::new(zx::Status::err_from_raw(status)));
        }

        let (_status, name) = part.get_name().await.context("Get name failed")?;
        let pave_type;
        if let Some(string) = name {
            let guid = guid.unwrap();
            if guid.value != WORKSTATION_INSTALLER_GPT
                && !(src.contains("usb-bus") && WORKSTATION_PARTITION_GPTS.contains(&guid.value))
            {
                return Ok(None);
            }
            // TODO(https://fxbug.dev/42121026) support any other partitions that might be needed
            if string == "storage-sparse" {
                pave_type = Some(PartitionPaveType::Volume);
            } else if bootloader == BootloaderType::Efi {
                pave_type = Partition::get_efi_pave_type(&string.to_lowercase());
            } else if bootloader == BootloaderType::Coreboot {
                pave_type = Partition::get_coreboot_pave_type(&string);
            } else {
                pave_type = None;
            }
        } else {
            return Ok(None);
        }

        if let Some(pave_type) = pave_type {
            let info = part
                .get_info()
                .await
                .context("Get info failed")?
                .map_err(zx::Status::err_from_raw)?;
            let block_size = info.block_size.into();
            let size = info.block_count * block_size;

            Ok(Some(Partition { pave_type, src, size, block_size }))
        } else {
            Ok(None)
        }
    }

    fn get_efi_pave_type(label: &str) -> Option<PartitionPaveType> {
        if label.starts_with("zircon_") && label.len() == "zircon_x".len() {
            let configuration = Partition::letter_to_configuration(label.chars().last().unwrap());
            Some(PartitionPaveType::Asset { r#type: Asset::Kernel, config: configuration })
        } else if label.starts_with("vbmeta_") && label.len() == "vbmeta_x".len() {
            let configuration = Partition::letter_to_configuration(label.chars().last().unwrap());
            Some(PartitionPaveType::Asset {
                r#type: Asset::VerifiedBootMetadata,
                config: configuration,
            })
        } else if label.starts_with("efi")
            || label.starts_with("fuchsia.esp")
            || label.starts_with("bootloader")
        {
            Some(PartitionPaveType::Bootloader)
        } else {
            None
        }
    }

    fn get_coreboot_pave_type(label: &str) -> Option<PartitionPaveType> {
        if let Ok(re) = regex_lite::Regex::new(r"^zircon_(.)\.signed$") {
            if let Some(captures) = re.captures(label) {
                let config = Partition::letter_to_configuration(
                    captures.get(1).unwrap().as_str().chars().last().unwrap(),
                );
                Some(PartitionPaveType::Asset { r#type: Asset::Kernel, config: config })
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Gather all partitions that are children of the given block device,
    /// and return them.
    ///
    /// # Arguments
    /// * `block_device` - the |BlockDevice| to get partitions from.
    /// * `all_devices` - All known block devices in the system.
    /// * `bootloader` - the |BootloaderType| of this device.
    pub async fn get_partitions(
        block_device: &BlockDevice,
        all_devices: &Vec<BlockDevice>,
        bootloader: BootloaderType,
    ) -> Result<Vec<Self>, Error> {
        let mut partitions = Vec::new();

        for entry in all_devices {
            if !entry.topo_path.starts_with(&block_device.topo_path) || entry == block_device {
                // Skip partitions that are not children of this block device, and skip the block
                // device itself.
                continue;
            }
            let (local, remote) = zx::Channel::create();
            fdio::service_connect(&entry.class_path, remote).context("Connecting to partition")?;
            let local = fidl::AsyncChannel::from_channel(local);

            let proxy = BlockProxy::from_channel(local);
            if let Some(partition) = Partition::new(entry.class_path.clone(), proxy, bootloader)
                .await
                .context(format!(
                    "Creating partition for block device at {} ({})",
                    entry.topo_path, entry.class_path
                ))?
            {
                partitions.push(partition);
            }
        }
        Ok(partitions)
    }

    /// Pave this partition to disk, using the given |DynamicDataSinkProxy|.
    pub async fn pave<F>(
        &self,
        data_sink: &DynamicDataSinkProxy,
        progress_callback: &F,
    ) -> Result<(), Error>
    where
        F: Send + Sync + Fn(usize, usize) -> (),
    {
        match self.pave_type {
            PartitionPaveType::Asset { r#type: asset, config } => {
                let fidl_buf = self.read_data().await?;
                data_sink.write_asset(config, asset, fidl_buf).await?;
            }
            PartitionPaveType::Bootloader => {
                let fidl_buf = self.read_data().await?;
                // Currently we only store the bootloader in slot A, we don't use an A/B/R scheme.
                data_sink.write_firmware(Configuration::A, "", fidl_buf).await?;
            }
            PartitionPaveType::Volume => {
                self.pave_volume(data_sink, progress_callback).await?;
            }
        };
        Ok(())
    }

    async fn pave_volume<F>(
        &self,
        _data_sink: &DynamicDataSinkProxy,
        _progress_callback: &F,
    ) -> Result<(), Error>
    where
        F: Send + Sync + Fn(usize, usize) -> (),
    {
        Err(Error::from(zx::Status::NOT_SUPPORTED))
    }

    /// Pave this A/B partition to its 'B' slot.
    /// Will return an error if the partition is not an A/B partition.
    pub async fn pave_b(&self, data_sink: &DynamicDataSinkProxy) -> Result<(), Error> {
        if !self.is_ab() {
            return Err(Error::from(zx::Status::NOT_SUPPORTED));
        }

        let fidl_buf = self.read_data().await?;
        match self.pave_type {
            PartitionPaveType::Asset { r#type: asset, config: _ } => {
                // pave() will always pave to A, so this always paves to B.
                // The A/B config from the partition is not respected because on a fresh
                // install we want A/B to be identical, so we install the same thing to both.
                data_sink.write_asset(Configuration::B, asset, fidl_buf).await?;
                Ok(())
            }
            _ => Err(Error::from(zx::Status::NOT_SUPPORTED)),
        }
    }

    /// Returns true if this partition has A/B variants when installed.
    pub fn is_ab(&self) -> bool {
        if let PartitionPaveType::Asset { r#type: _, config } = self.pave_type {
            // We only check against the A configuration because |letter_to_configuration|
            // returns A for 'A' and 'B' configurations.
            return config == Configuration::A;
        }
        return false;
    }

    /// Read this partition into a FIDL buffer.
    async fn read_data(&self) -> Result<Buffer, Error> {
        let mut rounded_size = self.size;
        let page_size = u64::from(zx::system_get_page_size());
        if rounded_size % page_size != 0 {
            rounded_size += page_size;
            rounded_size -= rounded_size % page_size;
        }

        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, rounded_size)?;

        let proxy =
            fuchsia_component::client::connect_to_protocol_at_path::<BlockMarker>(&self.src)
                .with_context(|| format!("Connecting to block device {}", self.src))?;
        let block_device = RemoteBlockClient::new(proxy).await?;
        // SAFETY: We only attach this VMO once, and we do not hold any Rust references to its
        // memory (in fact, we do not even map it).
        let vmo_id = unsafe { block_device.attach_vmo(&vmo) }.await?;

        // Reading too much at a time causes the UMS driver to return an error.
        let max_read_length: u64 = self.block_size * 100;
        let mut read: u64 = 0;
        while read < self.size {
            let read_size = min(self.size - read, max_read_length);
            if let Err(e) = block_device
                .read_at(MutableBufferSlice::new_with_vmo_id(&vmo_id, read, read_size), read)
                .await
                .context("Reading from partition to VMO")
            {
                // Need to detach before returning.
                block_device.detach_vmo(vmo_id).await?;
                return Err(e);
            }

            read += read_size;
        }

        block_device.detach_vmo(vmo_id).await?;

        return Ok(Buffer { vmo: fidl::Vmo::from(vmo), size: self.size });
    }

    /// Return the |Configuration| that is represented by the given
    /// character. Returns 'Recovery' for the letters 'R' and 'r', and 'A' for
    /// anything else.
    fn letter_to_configuration(letter: char) -> Configuration {
        // Note that we treat 'A' and 'B' the same, as the installer will install
        // the same image to both A and B.
        match letter {
            'A' | 'a' => Configuration::A,
            'B' | 'b' => Configuration::A,
            'R' | 'r' => Configuration::Recovery,
            _ => Configuration::A,
        }
    }
}

impl fmt::Debug for Partition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.pave_type {
            PartitionPaveType::Asset { r#type, config } => write!(
                f,
                "Partition[src={}, pave_type={:?}, asset={:?}, config={:?}]",
                self.src, self.pave_type, r#type, config
            ),
            _ => write!(f, "Partition[src={}, pave_type={:?}]", self.src, self.pave_type),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_storage_block::{
        BlockInfo, BlockMarker, BlockRequest, BlockRequestStream, DeviceFlag, Guid,
    };
    use fuchsia_async as fasync;
    use futures::{TryFutureExt, TryStreamExt};

    async fn serve_partition(
        label: &str,
        block_size: u32,
        block_count: u64,
        guid: [u8; 16],
        mut stream: BlockRequestStream,
    ) -> Result<(), Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                BlockRequest::GetName { responder } => responder.send(0, Some(label))?,
                BlockRequest::GetInfo { responder } => responder.send(Ok(&BlockInfo {
                    block_count,
                    block_size,
                    max_transfer_size: 0,
                    flags: DeviceFlag::empty(),
                }))?,
                BlockRequest::GetTypeGuid { responder } => {
                    responder.send(0, Some(&Guid { value: guid }))?
                }
                _ => panic!("Expected a GetInfo/GetName request, but did not get one."),
            }
        }
        Ok(())
    }

    fn mock_partition(
        label: &'static str,
        block_size: usize,
        block_count: usize,
        guid: [u8; 16],
    ) -> Result<BlockProxy, Error> {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<BlockMarker>();
        fasync::Task::local(
            serve_partition(
                label,
                block_size.try_into().unwrap(),
                block_count.try_into().unwrap(),
                guid,
                stream,
            )
            .unwrap_or_else(|e| panic!("Error while serving fake block device: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn test_new_partition_bad_guid() -> Result<(), Error> {
        let proxy = mock_partition("zircon_a", 512, 1000, [0xaa; 16])?;
        let part = Partition::new("zircon_a".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_zircona() -> Result<(), Error> {
        let proxy = mock_partition("zircon_a", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zircon_a".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::A }
        );
        assert_eq!(part.size, 512 * 1000);
        assert_eq!(part.src, "zircon_a");
        assert!(part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_zirconb() -> Result<(), Error> {
        let proxy = mock_partition("zircon_b", 20, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zircon_b".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::A }
        );
        assert_eq!(part.size, 20 * 1000);
        assert_eq!(part.src, "zircon_b");
        assert!(part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_zirconr() -> Result<(), Error> {
        let proxy = mock_partition("zircon_r", 40, 200, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zircon_r".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::Recovery }
        );
        assert_eq!(part.size, 40 * 200);
        assert_eq!(part.src, "zircon_r");
        assert!(!part.is_ab());
        Ok(())
    }

    async fn new_partition_vbmetax_test_helper(
        name: &'static str,
        expected_config: Configuration,
    ) -> Result<(), Error> {
        let proxy = mock_partition(name, 40, 200, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new(name.to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset {
                r#type: Asset::VerifiedBootMetadata,
                config: expected_config
            }
        );
        assert_eq!(part.size, 40 * 200);
        assert_eq!(part.src, name);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_vbmetaa() -> Result<(), Error> {
        new_partition_vbmetax_test_helper("vbmeta_a", Configuration::A).await
    }

    #[fuchsia::test]
    async fn test_new_partition_vbmetab() -> Result<(), Error> {
        // 'A' and 'B' are treated the same, as the installer will install
        // the same image to both A and B.
        new_partition_vbmetax_test_helper("vbmeta_b", Configuration::A).await
    }

    #[fuchsia::test]
    async fn test_new_partition_vbmetar() -> Result<(), Error> {
        new_partition_vbmetax_test_helper("vbmeta_r", Configuration::Recovery).await
    }

    #[fuchsia::test]
    async fn test_new_partition_efi() -> Result<(), Error> {
        let proxy = mock_partition("efi", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("efi".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(part.pave_type, PartitionPaveType::Bootloader);
        assert_eq!(part.size, 512 * 1000);
        assert_eq!(part.src, "efi");
        assert!(!part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_fvm() -> Result<(), Error> {
        let proxy = mock_partition("storage-sparse", 2048, 4097, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("storage-sparse".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(part.pave_type, PartitionPaveType::Volume);
        assert_eq!(part.size, 2048 * 4097);
        assert_eq!(part.src, "storage-sparse");
        assert!(!part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_zircona_unsigned_coreboot() -> Result<(), Error> {
        let proxy = mock_partition("zircon_a", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zircon_a".to_string(), proxy, BootloaderType::Coreboot).await?;
        assert!(part.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_zircona_signed_coreboot() -> Result<(), Error> {
        let proxy = mock_partition("zircon_a.signed", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part =
            Partition::new("zircon_a.signed".to_string(), proxy, BootloaderType::Coreboot).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::A }
        );
        assert_eq!(part.size, 512 * 1000);
        assert_eq!(part.src, "zircon_a.signed");
        assert!(part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_unknown() -> Result<(), Error> {
        let proxy = mock_partition("unknown-label", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("unknown-label".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_zedboot_efi() -> Result<(), Error> {
        let proxy = mock_partition("zedboot-efi", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zedboot-efi".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_invalid_partitions_coreboot() -> Result<(), Error> {
        let proxy = mock_partition("zircon_.signed", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part =
            Partition::new("zircon_.signed".to_string(), proxy, BootloaderType::Coreboot).await?;
        assert!(part.is_none());

        let proxy = mock_partition("zircon_aa.signed", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part =
            Partition::new("zircon_aa.signed".to_string(), proxy, BootloaderType::Coreboot).await?;
        assert!(part.is_none());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_invalid_partitions_efi() -> Result<(), Error> {
        let proxy = mock_partition("zircon_", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zircon_".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());

        let proxy = mock_partition("zircon_aa", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part = Partition::new("zircon_aa".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());

        let proxy = mock_partition("zircon_a.signed", 512, 1000, WORKSTATION_INSTALLER_GPT)?;
        let part =
            Partition::new("zircon_a.signed".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_usb_bad_guid() -> Result<(), Error> {
        let proxy = mock_partition("zircon_a", 512, 1000, [0xaa; 16])?;
        let part = Partition::new("/dev/usb-bus".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_usb_zircona() -> Result<(), Error> {
        let proxy = mock_partition("zircon_a", 512, 1000, WORKSTATION_PARTITION_GPTS[2])?;
        let part = Partition::new("/dev/usb-bus".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::A }
        );
        assert_eq!(part.size, 512 * 1000);
        assert_eq!(part.src, "/dev/usb-bus");
        assert!(part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_usb_zirconb() -> Result<(), Error> {
        let proxy = mock_partition("zircon_b", 20, 1000, WORKSTATION_PARTITION_GPTS[3])?;
        let part = Partition::new("/dev/usb-bus".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::A }
        );
        assert_eq!(part.size, 20 * 1000);
        assert_eq!(part.src, "/dev/usb-bus");
        assert!(part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_usb_zirconr() -> Result<(), Error> {
        let proxy = mock_partition("zircon_r", 40, 200, WORKSTATION_PARTITION_GPTS[4])?;
        let part = Partition::new("/dev/usb-bus".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(
            part.pave_type,
            PartitionPaveType::Asset { r#type: Asset::Kernel, config: Configuration::Recovery }
        );
        assert_eq!(part.size, 40 * 200);
        assert_eq!(part.src, "/dev/usb-bus");
        assert!(!part.is_ab());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_partition_usb_efi() -> Result<(), Error> {
        let proxy = mock_partition("efi-system", 512, 1000, WORKSTATION_PARTITION_GPTS[0])?;
        let part = Partition::new("/dev/usb-bus".to_string(), proxy, BootloaderType::Efi).await?;
        assert!(part.is_some());
        let part = part.unwrap();
        assert_eq!(part.pave_type, PartitionPaveType::Bootloader);
        assert_eq!(part.size, 512 * 1000);
        assert_eq!(part.src, "/dev/usb-bus");
        assert!(!part.is_ab());
        Ok(())
    }
}
