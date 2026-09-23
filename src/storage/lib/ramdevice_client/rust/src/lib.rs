// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A safe rust wrapper for creating and using ramdisks.

#![deny(missing_docs)]
use anyhow::Error;
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_hardware_ramdisk as framdisk;
use fidl_fuchsia_hardware_ramdisk::Guid;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block as fblock;
use fs_management::filesystem::{BlockConnector, DirBasedBlockConnector};
use fuchsia_component_client::Service;

const GUID_LEN: usize = 16;

/// A type to help construct a [`RamdeviceClient`] optionally from a VMO.
pub struct RamdiskClientBuilder {
    ramdisk_source: RamdiskSource,
    block_size: u64,
    max_transfer_blocks: Option<u32>,
    guid: Option<[u8; GUID_LEN]>,
    ramdisk_service: Option<fio::DirectoryProxy>,
    device_flags: Option<fblock::DeviceFlag>,

    // Whether to publish this ramdisk as a fuchsia.hardware.block.volume.Service service.
    publish: bool,
}

enum RamdiskSource {
    Vmo { vmo: zx::Vmo },
    Size { block_count: u64 },
}

impl RamdiskClientBuilder {
    /// Create a new ramdisk builder
    pub fn new(block_size: u64, block_count: u64) -> Self {
        Self {
            ramdisk_source: RamdiskSource::Size { block_count },
            block_size,
            max_transfer_blocks: None,
            guid: None,
            ramdisk_service: None,
            publish: false,
            device_flags: None,
        }
    }

    /// Create a new ramdisk builder with a vmo
    pub fn new_with_vmo(vmo: zx::Vmo, block_size: Option<u64>) -> Self {
        Self {
            ramdisk_source: RamdiskSource::Vmo { vmo },
            block_size: block_size.unwrap_or(0),
            max_transfer_blocks: None,
            guid: None,
            ramdisk_service: None,
            publish: false,
            device_flags: None,
        }
    }

    /// Initialize the ramdisk with the given GUID, which can be queried from the ramdisk instance.
    pub fn guid(mut self, guid: [u8; GUID_LEN]) -> Self {
        self.guid = Some(guid);
        self
    }

    /// Sets the maximum transfer size.
    pub fn max_transfer_blocks(mut self, value: u32) -> Self {
        self.max_transfer_blocks = Some(value);
        self
    }

    /// Specifies the ramdisk service.
    pub fn ramdisk_service(mut self, service: fio::DirectoryProxy) -> Self {
        self.ramdisk_service = Some(service);
        self
    }

    /// Publish this ramdisk as a fuchsia.hardware.block.volume.Service service.
    pub fn publish(mut self) -> Self {
        self.publish = true;
        self
    }

    /// Use the provided device flags.
    pub fn device_flags(mut self, device_flags: fblock::DeviceFlag) -> Self {
        self.device_flags = Some(device_flags);
        self
    }

    /// Create the ramdisk.
    pub async fn build(self) -> Result<RamdiskClient, Error> {
        let Self {
            ramdisk_source,
            block_size,
            max_transfer_blocks,
            guid,
            ramdisk_service,
            publish,
            device_flags,
        } = self;

        // Pick the first service instance we find.
        let service = match ramdisk_service {
            Some(s) => {
                Service::from_service_dir_proxy(s, fidl_fuchsia_hardware_ramdisk::ServiceMarker)
            }
            None => Service::open(fidl_fuchsia_hardware_ramdisk::ServiceMarker)?,
        };
        let ramdisk_controller = service.watch_for_any().await?.connect_to_controller()?;

        let type_guid = guid.map(|guid| Guid { value: guid });

        let options = match ramdisk_source {
            RamdiskSource::Vmo { vmo } => framdisk::Options {
                vmo: Some(vmo),
                block_size: if block_size == 0 {
                    None
                } else {
                    Some(block_size.try_into().unwrap())
                },
                type_guid,
                publish: Some(publish),
                max_transfer_blocks,
                device_flags,
                ..Default::default()
            },
            RamdiskSource::Size { block_count } => framdisk::Options {
                block_count: Some(block_count),
                block_size: Some(block_size.try_into().unwrap()),
                type_guid,
                publish: Some(publish),
                max_transfer_blocks,
                device_flags,
                ..Default::default()
            },
        };

        let (outgoing, event) =
            ramdisk_controller.create(options).await?.map_err(zx::Status::err_from_raw)?;

        let outgoing = outgoing.into_proxy();

        RamdiskClient::new(outgoing, event)
    }
}

/// A client for managing a ramdisk. This can be created with the [`RamdiskClient::create`]
/// function or through the type returned by [`RamdiskClient::builder`] to specify additional
/// options.
pub struct RamdiskClient {
    /// The outgoing directory for the ram-disk.
    outgoing: fio::DirectoryProxy,

    /// The event that keeps the ramdisk alive.
    _event: zx::EventPair,
}

impl RamdiskClient {
    fn new(outgoing: fio::DirectoryProxy, event: zx::EventPair) -> Result<Self, Error> {
        Ok(Self { outgoing, _event: event })
    }

    /// Create a new ramdisk builder with the given block_size and block_count.
    pub fn builder(block_size: u64, block_count: u64) -> RamdiskClientBuilder {
        RamdiskClientBuilder::new(block_size, block_count)
    }

    /// Create a new ramdisk.
    pub async fn create(block_size: u64, block_count: u64) -> Result<Self, Error> {
        Self::builder(block_size, block_count).build().await
    }

    /// Returns the directory proxy for the ramdisk's outgoing directory.
    pub fn outgoing(&self) -> &fio::DirectoryProxy {
        &self.outgoing
    }

    /// Get an open channel to the underlying ramdevice.
    pub fn open(&self) -> Result<fidl::endpoints::ClientEnd<fblock::BlockMarker>, Error> {
        let (client, server_end) = fidl::endpoints::create_endpoints();
        self.connect(server_end)?;
        Ok(client)
    }

    /// Gets a connector for the Block protocol of the ramdisk.
    pub fn connector(&self) -> Result<Box<dyn BlockConnector>, Error> {
        let block_dir = fuchsia_fs::directory::clone(&self.outgoing)?;
        Ok(Box::new(DirBasedBlockConnector::new(
            block_dir,
            format!("svc/{}", fblock::BlockMarker::PROTOCOL_NAME),
        )))
    }

    /// Get an open channel to the underlying ramdevice.
    pub fn connect(
        &self,
        server_end: fidl::endpoints::ServerEnd<fblock::BlockMarker>,
    ) -> Result<(), Error> {
        Ok(self.outgoing.open(
            &format!("svc/{}", fblock::BlockMarker::PROTOCOL_NAME),
            fio::Flags::PROTOCOL_SERVICE,
            &fio::Options::default(),
            server_end.into_channel(),
        )?)
    }

    /// Get an open channel to the Ramdisk protocol.
    pub fn open_ramdisk(&self) -> Result<framdisk::RamdiskProxy, Error> {
        let (client, server) = fidl::endpoints::create_proxy::<framdisk::RamdiskMarker>();
        self.outgoing.open(
            &format!("svc/{}", framdisk::RamdiskMarker::PROTOCOL_NAME),
            fio::Flags::PROTOCOL_SERVICE,
            &fio::Options::default(),
            server.into_channel(),
        )?;
        Ok(client)
    }

    /// Get an open channel to the Mapper protocol.
    pub fn open_mapper(&self) -> Result<fblock::MapperProxy, Error> {
        let (client, server) = fidl::endpoints::create_proxy::<fblock::MapperMarker>();
        self.outgoing.open(
            &format!("svc/{}", fblock::MapperMarker::PROTOCOL_NAME),
            fio::Flags::PROTOCOL_SERVICE,
            &fio::Options::default(),
            server.into_channel(),
        )?;
        Ok(client)
    }

    /// Consume the client and return the event that keeps the ramdisk alive.
    pub fn into_event(self) -> zx::EventPair {
        self._event
    }
}

impl BlockConnector for RamdiskClient {
    fn connect_channel_to_block(
        &self,
        server_end: fidl::endpoints::ServerEnd<fidl_fuchsia_storage_block::BlockMarker>,
    ) -> Result<(), Error> {
        self.connect(server_end.into_channel().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note that if these tests flake, all downstream tests that depend on this crate may too.

    const TEST_GUID: [u8; GUID_LEN] = [
        0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
        0x10,
    ];

    #[fuchsia::test]
    async fn create_get_dir_proxy_destroy() {
        // just make sure all the functions are hooked up properly.
        let ramdisk =
            RamdiskClient::builder(512, 2048).build().await.expect("failed to create ramdisk");
        let ramdisk_dir = &ramdisk.outgoing;
        fuchsia_fs::directory::readdir(ramdisk_dir).await.expect("failed to readdir");
    }

    #[fuchsia::test]
    async fn create_with_guid_get_dir_proxy_destroy() {
        let ramdisk = RamdiskClient::builder(512, 2048)
            .guid(TEST_GUID)
            .build()
            .await
            .expect("failed to create ramdisk");
        let ramdisk_dir = &ramdisk.outgoing;
        fuchsia_fs::directory::readdir(ramdisk_dir).await.expect("failed to readdir");
    }

    #[fuchsia::test]
    async fn create_open_destroy() {
        let ramdisk = RamdiskClient::create(512, 2048).await.unwrap();
        let client = ramdisk.open().unwrap().into_proxy();
        client.get_info().await.expect("get_info failed").unwrap();
        // The ramdisk will be scheduled to be unbound, so `client` may be valid for some time.
    }

    #[fuchsia::test]
    async fn create_open_into_event() {
        let ramdisk = RamdiskClient::create(512, 2048).await.unwrap();
        let client = ramdisk.open().unwrap().into_proxy();
        client.get_info().await.expect("get_info failed").unwrap();
        let _event = ramdisk.into_event();
        // We should succeed calling `get_info` as the ramdisk should still exist.
        client.get_info().await.expect("get_info failed").unwrap();
    }
}
