// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow, bail};
use fuchsia_sync::Mutex;
use mapping::{DELIVERY_VMO_SIZE, DeliveryCommand, RawDeliveryCommand};
use std::collections::HashMap;
use std::sync::Arc;
use storage_ptr_slice::PtrByteSlice;
use zx;

pub use mapping::DELIVERY_DATA_SIZE;

/// Unverified data pages delivered from the driver to be verified.
#[derive(Copy, Clone)]
pub struct UnverifiedPages<'a> {
    slice: PtrByteSlice<'a>,
    /// Byte offset within the shared delivery VMO.
    delivery_offset: u64,
}

impl<'a> UnverifiedPages<'a> {
    pub(crate) fn new(slice: PtrByteSlice<'a>, delivery_offset: u64) -> Result<Self, zx::Status> {
        let page_size = zx::system_get_page_size() as usize;
        if slice.len() % page_size != 0 || delivery_offset % (page_size as u64) != 0 {
            return Err(zx::Status::INVALID_ARGS);
        }
        Ok(Self { slice, delivery_offset })
    }

    /// Returns the length of the payload in bytes.
    pub fn len_in_bytes(&self) -> usize {
        self.slice.len()
    }

    /// Returns the length of the payload in pages.
    pub fn len_in_pages(&self) -> usize {
        self.slice.len() / (zx::system_get_page_size() as usize)
    }

    /// Returns the byte offset within the shared delivery VMO.
    pub fn delivery_offset(&self) -> u64 {
        self.delivery_offset
    }

    /// Returns the underlying raw pointer byte slice to the unverified data.
    pub fn as_ptr_byte_slice(&self) -> PtrByteSlice<'a> {
        self.slice
    }
}

/// Trait providing data delivery and blob metadata registration for incoming delivery commands.
pub trait DeliveryQueueProvider: Send + Sync + 'static {
    /// Delivers data directly from the delivery VMO into the target blob.
    fn deliver_pages(
        &self,
        key: u64,
        target_offset: u64,
        unverified_pages: UnverifiedPages<'_>,
    ) -> Result<(), Error>;

    /// Handles a RegisterBlob command using a pointer slice to shared memory.
    fn register_blob(&self, key: u64, leaf_data: PtrByteSlice<'_>) -> Result<(), Error> {
        let _ = (key, leaf_data);
        Ok(())
    }
}

/// A simple in-memory `DeliveryQueueProvider` for testing delivery without Merkle verification.
pub struct TestVmoProvider {
    pager: Arc<zx::Pager>,
    delivery_vmo: zx::Vmo,
    vmos: Mutex<HashMap<u64, zx::Vmo>>,
}

impl TestVmoProvider {
    pub fn new(pager: Arc<zx::Pager>, delivery_vmo: zx::Vmo) -> Self {
        Self { pager, delivery_vmo, vmos: Mutex::new(HashMap::new()) }
    }

    pub fn delivery_vmo(&self) -> &zx::Vmo {
        &self.delivery_vmo
    }

    pub fn register_vmo(&self, key: u64, vmo: zx::Vmo) {
        self.vmos.lock().insert(key, vmo);
    }

    pub fn unregister_vmo(&self, key: u64) {
        self.vmos.lock().remove(&key);
    }
}

impl DeliveryQueueProvider for TestVmoProvider {
    fn deliver_pages(
        &self,
        key: u64,
        target_offset: u64,
        unverified_pages: UnverifiedPages<'_>,
    ) -> Result<(), Error> {
        let vmos = self.vmos.lock();
        if let Some(target_vmo) = vmos.get(&key) {
            let length = unverified_pages.len_in_bytes() as u64;
            if length == 0 {
                return Ok(());
            }
            self.pager.supply_pages(
                target_vmo,
                target_offset..target_offset + length,
                &self.delivery_vmo,
                unverified_pages.delivery_offset(),
            )?;
            Ok(())
        } else {
            bail!("Unknown or expired key {key} in deliver_pages");
        }
    }
}

pub struct DeliveryQueueProcessor {
    delivery_vmo: zx::Vmo,
    delivery_thread: Option<std::thread::JoinHandle<()>>,
}

impl DeliveryQueueProcessor {
    pub fn spawn(
        mut receiver: vmo_fifo::Receiver<RawDeliveryCommand>,
        provider: Arc<dyn DeliveryQueueProvider>,
        delivery_vmo: zx::Vmo,
    ) -> Result<Self, Error> {
        let delivery_vmo_clone = delivery_vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;

        let thread = std::thread::spawn(move || {
            while let Ok(msg) = receiver.peek() {
                let raw_cmd = RawDeliveryCommand {
                    opcode: msg.opcode,
                    _padding: msg._padding,
                    key: msg.key,
                    target_offset: msg.target_offset,
                    length: msg.length,
                    offset: msg.offset,
                };

                match DeliveryCommand::try_from(raw_cmd) {
                    Ok(cmd) => {
                        if let Err(e) = Self::handle_delivery(cmd, &msg, &*provider) {
                            log::warn!("Failed to handle delivery command: {:?}", e);
                        }
                    }
                    Err(e) => {
                        log::warn!("Invalid delivery command opcode: {:?}", e);
                    }
                }
                if let Err(e) = msg.pop() {
                    log::warn!("Failed to pop delivery message: {:?}", e);
                    break;
                }
            }
        });

        Ok(Self { delivery_vmo: delivery_vmo_clone, delivery_thread: Some(thread) })
    }

    pub fn delivery_vmo(&self) -> &zx::Vmo {
        &self.delivery_vmo
    }

    fn handle_delivery(
        cmd: DeliveryCommand,
        raw_msg: &vmo_fifo::Message<'_, RawDeliveryCommand>,
        provider: &dyn DeliveryQueueProvider,
    ) -> Result<(), Error> {
        match cmd {
            DeliveryCommand::Data { key, target_offset, length, offset } => {
                if offset.checked_add(length).unwrap_or(u32::MAX) > DELIVERY_VMO_SIZE as u32 {
                    bail!("Data payload out of bounds");
                }
                let vmo_offset = raw_msg.payload_region_offset() as u64 + offset as u64;
                // The sender is expected to push page-aligned payloads. If the payload is the
                // final chunk of a blob and the data does not end on a page boundary, the sender
                // must extend `length` to the next page boundary and pad the trailing bytes with
                // zeros. This is required because `fuchsia-merkle`'s `verify_aligned` expects
                // the buffer length to be a multiple of the system page size.
                let page_size = zx::system_get_page_size() as u32;
                if offset % page_size != 0 {
                    bail!("Delivery payload offset must be page aligned: {offset}");
                }
                if length % page_size != 0 {
                    bail!("Delivery payload length must be page aligned: {length}");
                }
                if target_offset % (page_size as u64) != 0 {
                    bail!("Delivery payload target_offset must be page aligned: {target_offset}");
                }
                let unverified_pages =
                    UnverifiedPages::new(raw_msg.payload_slice(offset, length), vmo_offset)
                        .map_err(|s| anyhow!("Invalid unverified pages: {s}"))?;
                provider.deliver_pages(key, target_offset, unverified_pages)?;
            }
            DeliveryCommand::RegisterBlob { key, offset, length } => {
                if offset.checked_add(length).unwrap_or(u32::MAX) > DELIVERY_VMO_SIZE as u32 {
                    bail!("RegisterBlob payload out of bounds");
                }
                let leaf_data = raw_msg.payload_slice(offset, length);
                provider.register_blob(key, leaf_data)?;
            }
        }
        Ok(())
    }
}

impl Drop for DeliveryQueueProcessor {
    fn drop(&mut self) {
        let _ = self.delivery_vmo.signal(zx::Signals::empty(), vmo_fifo::SIG_SHUTDOWN);
        if let Some(thread) = self.delivery_thread.take() {
            let _ = thread.join();
        }
    }
}
