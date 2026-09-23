// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use block_server::async_interface::SessionManager;
use block_server::{BlockInfo, BlockServer, DeviceInfo};
#[cfg(feature = "for-testing")]
use fidl::endpoints::RequestStream;
use fidl::endpoints::{ClientEnd, FromClient, ServerEnd};
#[cfg(feature = "for-testing")]
use fidl_fuchsia_hardware_inlineencryption::{DeviceMarker, DeviceRequest, DeviceRequestStream};
use fidl_fuchsia_storage_block as fblock;
use fs_management::filesystem::BlockConnector;
#[cfg(feature = "for-testing")]
use futures::StreamExt;
use std::sync::Arc;

#[cfg(not(feature = "for-testing"))]
mod data;
#[cfg(not(feature = "for-testing"))]
use data::Data;

#[cfg(feature = "for-testing")]
mod data_for_testing;
#[cfg(feature = "for-testing")]
use data_for_testing::{Data, FscryptKeys};
#[cfg(feature = "for-testing")]
pub use data_for_testing::{Observer, WriteAction, WriteCache};

/// A local server backed by a VMO.
pub struct VmoBackedServer {
    server: BlockServer<SessionManager<Data>>,
}

impl VmoBackedServer {
    /// Handles `requests`.  The future will resolve when the stream terminates.
    pub async fn serve(&self, requests: fblock::BlockRequestStream) -> Result<(), Error> {
        let res = self.server.handle_requests(requests).await;

        #[cfg(feature = "for-testing")]
        self.server.session_manager().interface().client_closed()?;

        res
    }

    pub fn new(block_count: u64, block_size: u32, initial_content: &[u8]) -> Result<Self, Error> {
        VmoBackedServerOptions {
            block_size,
            initial_contents: InitialContents::FromCapacityAndBuffer(block_count, initial_content),
            ..Default::default()
        }
        .build()
    }

    pub fn from_vmo(block_size: u32, vmo: zx::Vmo) -> Result<Self, Error> {
        VmoBackedServerOptions {
            block_size,
            initial_contents: InitialContents::FromVmo(vmo),
            ..Default::default()
        }
        .build()
    }
}

#[cfg(feature = "for-testing")]
impl VmoBackedServer {
    pub fn from_file(block_size: u32, path: &str) -> Self {
        let contents = std::fs::read(path).expect("Failed to read file");
        VmoBackedServerOptions {
            block_size,
            initial_contents: InitialContents::FromBuffer(&contents),
            ..Default::default()
        }
        .build()
        .expect("Failed to create VmoBackedServer.")
    }

    pub fn connect<R: BlockClient>(self: &Arc<Self>) -> R {
        let (client, server) = fidl::endpoints::create_endpoints::<R::Protocol>();
        let this = self.clone();
        fuchsia_async::Task::spawn(async move {
            let _ = this.serve(server.into_stream().cast_stream()).await;
        })
        .detach();
        R::from_client(client)
    }

    pub fn connect_mapper(self: &Arc<Self>) -> fblock::MapperProxy {
        let (proxy, server) = fidl::endpoints::create_proxy::<fblock::MapperMarker>();
        let this = self.clone();
        fuchsia_async::Task::spawn(async move {
            let _ = this.server.handle_mapper_requests(server.into_stream()).await;
        })
        .detach();
        proxy
    }

    pub fn connect_insecure_inline_encryption_server(
        self: &Arc<Self>,
        server: ServerEnd<DeviceMarker>,
        uuid: [u8; 16],
    ) -> impl Future<Output = ()> + Send {
        let this = self.clone();
        this.serve_insecure_inline_encryption(server.into_stream(), uuid)
    }

    /// Evict key slot for software ciphers.
    pub fn evict_key_slot(&self, slot: u8) -> Result<(), zx::Status> {
        self.server.session_manager().interface().fscrypt_keys().evict_key(slot)
    }

    /// Implements software-fallback for fuchsia_hardware_inlineencryption.ProgramKey. There is a
    /// maximum of 256 keyslots. Insert keyslot at the next available slot.
    fn program_key(&self, xts_key: &[u8; 64]) -> Result<u8, zx::Status> {
        self.server.session_manager().interface().fscrypt_keys().program_key(xts_key)
    }

    pub async fn serve_insecure_inline_encryption(
        self: Arc<Self>,
        mut requests: DeviceRequestStream,
        uuid: [u8; 16],
    ) {
        while let Some(Ok(request)) = requests.next().await {
            match request {
                DeviceRequest::ProgramKey { wrapped_key, data_unit_size: _, responder } => {
                    responder
                        .send(
                            self.program_key(&fscrypt::to_xts_key(&wrapped_key, uuid))
                                .map_err(zx::Status::into_raw),
                        )
                        .unwrap_or_else(|e| {
                            log::error!("failed to send ProgramKey response. error: {:?}", e);
                        });
                }
                DeviceRequest::DeriveRawSecret { mut wrapped_key, responder } => {
                    // Swap the nibbles.
                    for b in &mut wrapped_key {
                        *b = *b >> 4 | *b << 4;
                    }
                    responder.send(Ok(&wrapped_key)).unwrap();
                }
            }
        }
    }
}

/// The initial contents of the VMO.  This also determines the size of the block device.
pub enum InitialContents<'a> {
    /// An empty VMO will be created with capacity for this many *blocks*.
    FromCapacity(u64),
    /// A VMO is created with capacity for this many *blocks* and the buffer's contents copied into
    /// it.
    FromCapacityAndBuffer(u64, &'a [u8]),
    /// A VMO is created which is exactly large enough for the initial contents (rounded up to block
    /// size), and the buffer's contents copied into it.
    FromBuffer(&'a [u8]),
    /// The provided VMO is used.  If its size is not block-aligned, the data will be truncated.
    FromVmo(zx::Vmo),
}

pub struct VmoBackedServerOptions<'a> {
    /// NB: `block_count` is ignored as that comes from `initial_contents`.
    pub info: DeviceInfo,
    pub block_size: u32,
    pub initial_contents: InitialContents<'a>,
    #[cfg(feature = "for-testing")]
    pub observer: Option<Box<dyn Observer>>,
    /// Enables write tracking so [`Observer::flush`] and [`Observer::barrier`] will be provided
    /// with [`WriteCache`]. Note that this is expensive.
    #[cfg(feature = "for-testing")]
    pub write_tracking: bool,
    /// If set, each operation will be delayed by a random duration <= this value, which is useful
    /// for testing race conditions due to out-of-order block requests.
    #[cfg(feature = "for-testing")]
    pub max_jitter_usec: Option<u64>,
}

impl Default for VmoBackedServerOptions<'_> {
    fn default() -> Self {
        VmoBackedServerOptions {
            info: DeviceInfo::Block(BlockInfo {
                device_flags: fblock::DeviceFlag::empty(),
                block_count: 0,
                max_transfer_blocks: None,
            }),
            block_size: 512,
            initial_contents: InitialContents::FromCapacity(0),
            #[cfg(feature = "for-testing")]
            observer: None,
            #[cfg(feature = "for-testing")]
            write_tracking: false,
            #[cfg(feature = "for-testing")]
            max_jitter_usec: None,
        }
    }
}

impl VmoBackedServerOptions<'_> {
    pub fn build(self) -> Result<VmoBackedServer, Error> {
        let (data, block_count) = match self.initial_contents {
            InitialContents::FromCapacity(block_count) => {
                (zx::Vmo::create(block_count * self.block_size as u64)?, block_count)
            }
            InitialContents::FromCapacityAndBuffer(block_count, buf) => {
                let needed =
                    buf.len()
                        .checked_next_multiple_of(self.block_size as usize)
                        .ok_or_else(|| anyhow!("Invalid buffer size"))? as u64
                        / self.block_size as u64;
                if needed > block_count {
                    return Err(anyhow!("Not enough capacity: {needed} vs {block_count}"));
                }
                let vmo = zx::Vmo::create(block_count * self.block_size as u64)?;
                if !buf.is_empty() {
                    vmo.write(buf, 0)?;
                }
                (vmo, block_count)
            }
            InitialContents::FromBuffer(buf) => {
                let block_count =
                    buf.len()
                        .checked_next_multiple_of(self.block_size as usize)
                        .ok_or_else(|| anyhow!("Invalid buffer size"))? as u64
                        / self.block_size as u64;
                let vmo = zx::Vmo::create(block_count * self.block_size as u64)?;
                if !buf.is_empty() {
                    vmo.write(buf, 0)?;
                }
                (vmo, block_count)
            }
            InitialContents::FromVmo(vmo) => {
                let size = vmo.get_size()?;
                let block_count = size / self.block_size as u64;
                (vmo, block_count)
            }
        };

        let info = match self.info {
            DeviceInfo::Block(mut info) => {
                info.block_count = block_count;
                DeviceInfo::Block(info)
            }
            DeviceInfo::Partition(mut info) => {
                info.block_count = block_count;
                DeviceInfo::Partition(info)
            }
            DeviceInfo::Volume(info) => DeviceInfo::Volume(info),
        };
        Ok(VmoBackedServer {
            server: BlockServer::new(
                self.block_size,
                Arc::new(Data {
                    info,
                    block_size: self.block_size,
                    data,
                    #[cfg(feature = "for-testing")]
                    observer: self.observer,
                    #[cfg(feature = "for-testing")]
                    write_cache: self
                        .write_tracking
                        .then(|| fuchsia_sync::Mutex::new(WriteCache::new(self.block_size as u64))),
                    #[cfg(feature = "for-testing")]
                    max_jitter_usec: self.max_jitter_usec,
                    #[cfg(feature = "for-testing")]
                    fscrypt_keys: fuchsia_sync::Mutex::new(FscryptKeys::new()),
                }),
            ),
        })
    }
}

/// Implements `BlockConnector` to vend connections to a VmoBackedServer.
pub struct VmoBackedServerConnector {
    scope: fuchsia_async::ScopeHandle,
    server: Arc<VmoBackedServer>,
}

impl VmoBackedServerConnector {
    /// New connections will served on the current scope.
    pub fn new(server: Arc<VmoBackedServer>) -> Self {
        Self { scope: fuchsia_async::Scope::current(), server }
    }

    /// New connections will served on the provided scope.
    pub fn new_with_scope(server: Arc<VmoBackedServer>, scope: fuchsia_async::ScopeHandle) -> Self {
        Self { scope, server }
    }
}

impl BlockConnector for VmoBackedServerConnector {
    fn connect_channel_to_block(
        &self,
        server_end: ServerEnd<fblock::BlockMarker>,
    ) -> Result<(), Error> {
        let server = self.server.clone();
        let _ = self.scope.spawn(async move {
            let _ = server.serve(server_end.into_stream()).await;
        });
        Ok(())
    }
}

pub trait BlockClient: FromClient {}

impl BlockClient for fblock::BlockProxy {}
impl BlockClient for fblock::BlockSynchronousProxy {}
impl BlockClient for ClientEnd<fblock::BlockMarker> {}

#[cfg(test)]
mod tests {
    use super::*;
    use block_server::async_interface::Interface;
    use block_server::{InlineCryptoOptions, ReadOptions, WriteOptions};

    #[fuchsia::test]
    async fn test_program_and_evict_key_slot() {
        let block_size = 4096;
        let server =
            VmoBackedServer::new(100, block_size, &[]).expect("Failed to create VmoBackedServer");

        let key = [0xaa; 64];
        let slot = server.program_key(&key).expect("program_key failed");
        assert_eq!(slot, 0);

        // Use the internal interface to avoid FIDL complexity for this test.
        let block_interface = server.server.session_manager().interface();
        // Verify that we can write and read using the programmed key.
        let vmo = Arc::new(zx::Vmo::create(block_size as u64).expect("Vmo::create failed"));
        let original_data = vec![0xbb; block_size as usize];
        vmo.write(&original_data, 0).expect("Vmo::write failed");
        let write_opts = WriteOptions {
            inline_crypto: InlineCryptoOptions::enabled(slot, 0),
            ..Default::default()
        };
        block_interface.write(0, 1, &vmo, 0, write_opts, None).await.expect("write failed");

        // Verify we can read it back.
        let vmo_read = Arc::new(zx::Vmo::create(block_size as u64).expect("Vmo::create failed"));
        let read_opts = ReadOptions {
            inline_crypto: InlineCryptoOptions::enabled(slot, 0),
            ..Default::default()
        };
        block_interface.read(0, 1, &vmo_read, 0, read_opts, None).await.expect("read failed");
        let mut read_data = vec![0u8; block_size as usize];
        vmo_read.read(&mut read_data, 0).expect("Vmo::read failed");
        assert_eq!(read_data, original_data);

        server.evict_key_slot(slot).expect("evict_key_slot failed");
        assert_eq!(server.evict_key_slot(slot), Err(zx::Status::INVALID_ARGS));

        // Writing and reading from file after the key has been evicted should fail.
        assert_eq!(
            block_interface.read(0, 1, &vmo_read, 0, read_opts, None).await,
            Err(zx::Status::IO)
        );

        assert_eq!(
            block_interface.write(0, 1, &vmo, 0, write_opts, None).await,
            Err(zx::Status::IO)
        );
    }

    #[fuchsia::test]
    async fn test_program_key_out_of_slots() {
        let server = VmoBackedServer::new(100, 512, &[]).expect("Failed to create VmoBackedServer");

        let key = [0xaa; 64];
        for expected_slot in 0..=u8::MAX {
            let slot = server.program_key(&key).expect("program_key failed");
            assert_eq!(slot, expected_slot);
        }
        assert_eq!(server.program_key(&key), Err(zx::Status::NO_RESOURCES));
    }
}
