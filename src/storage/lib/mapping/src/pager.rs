// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::reader::BlockService;
use crate::{DeliveryHandler, Files};
use std::sync::Arc;
use zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ;
use zx::{Packet, PacketContents, Port, Rights, UserPacket};

/// Runs a synchronous event loop that listens on `port` for pager page requests
/// and dispatches them to `files.handle_page_request`.
pub fn run_pager_loop<S: BlockService + ?Sized, D: DeliveryHandler>(
    port: &Port,
    files: &Files<S, D>,
) {
    loop {
        match port.wait(zx::MonotonicInstant::INFINITE) {
            Ok(packet) => match packet.contents() {
                PacketContents::User(_) => break,
                PacketContents::Pager(pager_packet) => {
                    if pager_packet.command() == ZX_PAGER_VMO_READ {
                        let offset = pager_packet.range().start;
                        let length = pager_packet.range().end - offset;
                        files.handle_page_request(packet.key(), offset..offset + length);
                    }
                }
                _ => {}
            },
            Err(_) => break,
        }
    }
}

/// A handle to a spawned background thread running `run_pager_loop`.
/// Dropping this handle sends a shutdown packet to the port and cleanly joins the thread.
pub struct PagerThread {
    port: Port,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PagerThread {
    /// Spawns a background thread running `run_pager_loop`.
    pub fn spawn<S: BlockService + ?Sized + 'static, D: DeliveryHandler>(
        port: Port,
        files: Arc<Files<S, D>>,
    ) -> Self {
        let thread_port = port.duplicate_handle(Rights::SAME_RIGHTS).expect("duplicate port");
        let thread = std::thread::spawn(move || {
            run_pager_loop(&thread_port, &files);
        });
        Self { port, thread: Some(thread) }
    }
}

impl Drop for PagerThread {
    fn drop(&mut self) {
        let packet = Packet::from_user_packet(0, 0, UserPacket::from_u8_array([0; 32]));
        let _ = self.port.queue(&packet);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::tests::FakeBlockService;
    use crate::testing::{TestDeliveryHandler, TestVecBuffer};

    #[fuchsia::test]
    fn test_pager_thread_lifecycle() {
        let port = Port::create();
        let service = Arc::new(FakeBlockService::new(vec![0u8; 4096]));
        let files = Arc::new(Files::new(
            service,
            TestDeliveryHandler(|_key, _range| TestVecBuffer::new(4096).0),
        ));
        let thread = PagerThread::spawn(port, files);
        drop(thread);
    }

    #[fuchsia::test]
    fn test_pager_packet() {
        let port = Port::create();
        let pager = zx::Pager::create(zx::PagerOptions::empty()).expect("create pager");
        let vmo = pager.create_vmo(zx::VmoOptions::empty(), &port, 1234, 4096).expect("create vmo");

        let vmo_clone = vmo.duplicate_handle(Rights::SAME_RIGHTS).expect("duplicate vmo");
        let _reader_thread = std::thread::spawn(move || {
            let mut b = [0u8; 1];
            let _ = vmo_clone.read(&mut b, 0);
        });

        let packet = port.wait(zx::MonotonicInstant::INFINITE).expect("wait packet");
        assert_eq!(packet.key(), 1234);
        if let PacketContents::Pager(pager_packet) = packet.contents() {
            assert_eq!(pager_packet.command(), ZX_PAGER_VMO_READ);
            assert_eq!(pager_packet.range(), 0..4096);
        } else {
            panic!("Expected pager packet");
        }
    }
}
