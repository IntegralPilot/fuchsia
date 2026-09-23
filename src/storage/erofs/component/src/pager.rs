// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::file::ErofsFile;
use fuchsia_async as fasync;
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};
use zx::sys::zx_page_request_command_t::{ZX_PAGER_VMO_COMPLETE, ZX_PAGER_VMO_READ};

const READAHEAD_ALIGNMENT: u64 = 128 * 1024;

/// Manages the in-memory lifecycle of an active pager-backed file.
///
/// To prevent memory leaks, the pager receiver initially holds files as `Weak` references. When a
/// client checks out a VMO (e.g., via `get_backing_memory`), this is upgraded to `Strong` so the
/// file and its metadata are not dropped while actively mapped. Once the client closes all VMO
/// handles, Zircon's `VMO_ZERO_CHILDREN` signal downgrades this back to `Weak`, allowing cleanup
/// if no other system handles are open.
pub enum FileHolder {
    Strong(Arc<ErofsFile>),
    Weak(Weak<ErofsFile>),
}

/// A wrapper around Zircon's `zx::Pager`.
pub struct ErofsPager {
    pager: Arc<zx::Pager>,
}

impl ErofsPager {
    /// Creates a new ErofsPager.
    pub fn new() -> Result<Self, zx::Status> {
        let pager = Arc::new(zx::Pager::create(zx::PagerOptions::empty())?);
        Ok(Self { pager })
    }

    /// Creates a pager-backed VMO and registers its packet receiver.
    pub fn create_vmo(
        &self,
        file: Weak<ErofsFile>,
        initial_size: u64,
    ) -> Result<(zx::Vmo, fasync::ReceiverRegistration<ErofsPacketReceiver>), zx::Status> {
        let receiver = ErofsPacketReceiver {
            file: Mutex::new(FileHolder::Weak(file)),
            pager: self.pager.clone(),
        };

        let registration = fasync::EHandle::local().register_receiver(receiver);

        let vmo = self
            .pager
            .create_vmo(
                zx::VmoOptions::empty(),
                fasync::EHandle::local().port(),
                registration.key(),
                initial_size,
            )
            .map_err(|e| {
                log::error!("self.pager.create_vmo failed: {:?}", e);
                e
            })?;

        Ok((vmo, registration))
    }
}

/// Receives pager packets and serves page requests from the kernel.
pub struct ErofsPacketReceiver {
    pub(crate) file: Mutex<FileHolder>,
    pager: Arc<zx::Pager>,
}

impl ErofsPacketReceiver {
    fn get_file(&self) -> Result<Arc<ErofsFile>, zx::Status> {
        let holder = self.file.lock().unwrap();
        match &*holder {
            FileHolder::Strong(strong) => Ok(strong.clone()),
            FileHolder::Weak(weak) => weak.upgrade().ok_or(zx::Status::BAD_STATE),
        }
    }

    fn page_in(&self, range: Range<u64>) -> Result<(), zx::Status> {
        let file = self.get_file()?;

        // Align the read to 128 KiB slots (readahead).
        let readahead_start = (range.start / READAHEAD_ALIGNMENT) * READAHEAD_ALIGNMENT;
        let mut readahead_end =
            ((range.end + READAHEAD_ALIGNMENT - 1) / READAHEAD_ALIGNMENT) * READAHEAD_ALIGNMENT;

        // Clamp to VMO size to avoid supplying pages out of bounds.
        let vmo_size = file.vmo().get_size()?;
        readahead_end = std::cmp::min(readahead_end, vmo_size);

        if readahead_end <= readahead_start {
            return Ok(());
        }

        let len = readahead_end - readahead_start;

        // TODO(https://fxbug.dev/521911087): Use a pre-allocated mmapped transfer buffer to page
        // in data instead of allocating a buffer and copying data multiple times.
        let mut buf = vec![0u8; len as usize];
        let read_bytes =
            file.fs().read_file_range(file.node(), readahead_start, &mut buf).map_err(|e| {
                log::error!("Read EROFS file range failed: {:?}", e);
                e.to_status()
            })?;

        if read_bytes < buf.len() {
            buf[read_bytes..].fill(0);
        }

        let aux_vmo = zx::Vmo::create(len)?;
        aux_vmo.write(&buf, 0)?;

        self.pager.supply_pages(file.vmo(), readahead_start..readahead_end, &aux_vmo, 0)?;
        Ok(())
    }

    /// Handles the `VMO_ZERO_CHILDREN` signal from Zircon on the pager VMO.
    ///
    /// If `num_children` is 0, the `FileHolder` is downgraded from `Strong` to `Weak` to break the
    /// cyclic reference and allow the file resources to be cleaned up if unused. If a new child
    /// was created between the signal being emitted and us handling it, we re-register the
    /// watcher so it will trigger again.
    fn receive_signal_packet(&self, signals: zx::SignalPacket) {
        assert!(signals.observed().contains(zx::Signals::VMO_ZERO_CHILDREN));

        let mut file_holder = self.file.lock().unwrap();
        let strong = match &*file_holder {
            FileHolder::Strong(strong) => strong.clone(),
            FileHolder::Weak(_) => return,
        };

        match strong.vmo().info() {
            Ok(info) => {
                if info.num_children == 0 {
                    let weak = FileHolder::Weak(Arc::downgrade(&strong));
                    *file_holder = weak;
                } else {
                    // Re-register the wait.
                    if let Err(e) = strong.register_zero_children_wait() {
                        log::error!("Failed to re-register VMO_ZERO_CHILDREN wait: {:?}", e);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to query VMO info: {:?}", e);
            }
        }
    }

    fn receive_pager_packet(&self, contents: zx::PagerPacket) {
        let command = contents.command();
        if command == ZX_PAGER_VMO_COMPLETE {
            return;
        }

        let range = contents.range();
        // If the command is anything except read, fail the request.
        if command != ZX_PAGER_VMO_READ {
            if let Ok(file) = self.get_file() {
                let _ = self.pager.op_range(
                    zx::PagerOp::Fail(zx::Status::NOT_SUPPORTED),
                    file.vmo(),
                    range,
                );
            }
            return;
        }

        if let Err(e) = self.page_in(range.clone()) {
            log::error!("Page fault handler failed: {:?}", e);
            if let Ok(file) = self.get_file() {
                let _ = self.pager.op_range(zx::PagerOp::Fail(e), file.vmo(), range);
            }
        }
    }
}

impl fasync::PacketReceiver for ErofsPacketReceiver {
    fn receive_packet(&self, packet: zx::Packet) {
        match packet.contents() {
            zx::PacketContents::Pager(contents) => {
                self.receive_pager_packet(contents);
            }
            zx::PacketContents::SignalOne(signals) => {
                self.receive_signal_packet(signals);
            }
            _ => {}
        }
    }
}
