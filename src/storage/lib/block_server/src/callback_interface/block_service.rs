// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::callback_interface::{Interface, Request, SessionManager};
use crate::{Operation, RequestId};
use anyhow::{Error, ensure};
use mapping::reader::{BlockService, MAX_READ_BUFFER_SIZE};
use std::borrow::Borrow;
use std::sync::{Arc, Weak};
use storage_device::buffer::OwnedBuffer;
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};

const DEFAULT_BUFFER_POOL_CAPACITY: usize = 64 * 1024;

/// Default fallback implementation of [`BlockService`] for
/// [`callback_interface::Interface`] backends (such as C++ drivers / virtio-block)
/// that return the default implementation from `into_block_service`.
pub struct DefaultCallbackBlockService<I: Interface + ?Sized> {
    orchestrator: Weak<I::Orchestrator>,
    allocator: Arc<BufferAllocator>,
    block_size: u32,
}

impl<I: Interface + ?Sized> DefaultCallbackBlockService<I> {
    pub fn new(orchestrator: &Arc<I::Orchestrator>) -> Self {
        Self::new_with_pool_capacity(orchestrator, DEFAULT_BUFFER_POOL_CAPACITY)
    }

    pub fn new_with_pool_capacity(
        orchestrator: &Arc<I::Orchestrator>,
        pool_capacity: usize,
    ) -> Self {
        let sm: &SessionManager<I> = orchestrator.as_ref().borrow();
        let block_size = sm.block_size();
        let source = BufferSource::new(pool_capacity);
        let allocator = Arc::new(BufferAllocator::new(
            std::cmp::max(block_size as usize, zx::system_get_page_size() as usize),
            source,
        ));
        Self { orchestrator: Arc::downgrade(orchestrator), allocator, block_size }
    }

    pub fn orchestrator(&self) -> Option<Arc<I::Orchestrator>> {
        self.orchestrator.upgrade()
    }

    pub fn allocator(&self) -> &Arc<BufferAllocator> {
        &self.allocator
    }
}

impl<I: Interface + ?Sized> BlockService for DefaultCallbackBlockService<I> {
    fn allocate_buffer(&self, max_len: usize) -> OwnedBuffer {
        let max_len = std::cmp::min(
            std::cmp::min(max_len, MAX_READ_BUFFER_SIZE),
            self.allocator.buffer_source().size(),
        );
        self.allocator.allocate_buffer_sync_owned(max_len)
    }

    fn read_blocks(
        &self,
        device_offset: u64,
        dest_buffer: OwnedBuffer,
        on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
    ) -> Result<(), Error> {
        let orchestrator =
            self.orchestrator.upgrade().ok_or_else(|| anyhow::anyhow!("Orchestrator dropped"))?;
        let sm: &SessionManager<I> = orchestrator.as_ref().borrow();
        let block_size = self.block_size as u64;
        ensure!(
            device_offset % block_size == 0
                && dest_buffer.len() as u64 % block_size == 0
                && dest_buffer.range().start as u64 % block_size == 0,
            "Unaligned read request: device_offset={device_offset}, len={}, vmo_offset={}",
            dest_buffer.len(),
            dest_buffer.range().start
        );
        let device_block_offset = device_offset / block_size;
        let block_count = (dest_buffer.len() as u64 / block_size) as u32;

        let vmo_offset = dest_buffer.range().start as u64;
        let vmo = self.allocator.buffer_source().vmo().clone();

        sm.submit_internal_request(
            |request_id: RequestId| Request {
                request_id,
                operation: Operation::Read {
                    device_block_offset,
                    block_count,
                    _unused: 0,
                    vmo_offset,
                    options: block_protocol::ReadOptions::default(),
                },
                trace_flow_id: None,
                vmo: Some(vmo),
            },
            Box::new(move |status| {
                if status.is_ok() {
                    on_complete(Ok(dest_buffer));
                } else {
                    on_complete(Err(anyhow::anyhow!("Block read failed: {:?}", status)));
                }
            }),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callback_interface::{Session, SessionManager};
    use crate::{BlockInfo, DeviceInfo};
    use fidl_fuchsia_storage_block as fblock;
    use fuchsia_sync::Mutex;
    use mapping::Extents;
    use mapping::reader::read_aligned_range;
    use std::borrow::Cow;
    use std::ops::ControlFlow;

    const BLOCK_SIZE: u32 = 512;

    use crate::testing::MockInterface;

    #[test]
    fn test_into_block_service_memoization() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let service1 = session_manager.into_block_service(&session_manager);
        let service2 = session_manager.into_block_service(&session_manager);
        assert!(Arc::ptr_eq(&service1, &service2));
    }

    struct CustomBlockService;
    impl BlockService for CustomBlockService {
        fn allocate_buffer(&self, _max_len: usize) -> OwnedBuffer {
            unimplemented!()
        }
        fn read_blocks(
            &self,
            _device_offset: u64,
            _dest_buffer: OwnedBuffer,
            _on_complete: Box<dyn FnOnce(Result<OwnedBuffer, Error>) + Send>,
        ) -> Result<(), Error> {
            unimplemented!()
        }
    }

    struct CustomInterface {
        custom_service: Arc<CustomBlockService>,
    }

    impl Interface for CustomInterface {
        type Orchestrator = SessionManager<Self>;

        fn get_info(&self) -> Cow<'_, DeviceInfo> {
            Cow::Owned(DeviceInfo::Block(BlockInfo {
                block_count: 100,
                max_transfer_blocks: None,
                device_flags: fblock::DeviceFlag::empty(),
            }))
        }

        fn spawn_session(&self, _session: Arc<Session<Self>>) {}

        fn on_requests(&self, _requests: &[Request]) {}

        fn into_block_service(
            self: Arc<Self>,
            _orchestrator: &Arc<Self::Orchestrator>,
        ) -> Arc<dyn BlockService> {
            self.custom_service.clone()
        }
    }

    #[test]
    fn test_custom_into_block_service_override() {
        let custom_service = Arc::new(CustomBlockService);
        let interface = Arc::new(CustomInterface { custom_service: custom_service.clone() });
        let session_manager = Arc::new(SessionManager::new(interface, BLOCK_SIZE));
        let service = session_manager.into_block_service(&session_manager);
        assert!(Arc::ptr_eq(&service, &(custom_service as Arc<dyn BlockService>)));
    }

    #[test]
    fn test_default_callback_block_service_weak_ref_no_cycle() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface, BLOCK_SIZE));
        let service = DefaultCallbackBlockService::<MockInterface>::new(&session_manager);
        assert!(service.orchestrator().is_some());

        let weak_sm = Arc::downgrade(&session_manager);
        drop(session_manager);
        assert!(weak_sm.upgrade().is_none());
        assert!(service.orchestrator().is_none());
    }

    #[fuchsia::test]
    async fn test_default_callback_block_service_read_aligned_range() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));
        let service = session_manager.into_block_service(&session_manager);

        let test_data = vec![0xaa_u8; 4096];
        let encoded = (8u64 << 32) | 0u64;
        let extents = Extents::from_encoded([encoded], 0).unwrap();

        let (send, recv) = futures::channel::oneshot::channel();
        let send = Mutex::new(Some(send));
        let read_buf = Mutex::new(Vec::new());

        let service_clone = service.clone();
        read_aligned_range(&extents, 0..4096, &*service_clone, move |buffer_result| {
            let buffer = buffer_result.unwrap();
            let mut read_guard = read_buf.lock();
            buffer.as_ref().append_to(&mut *read_guard);
            if read_guard.len() == 4096 {
                if let Some(s) = send.lock().take() {
                    let _ = s.send(read_guard.clone());
                }
            }
            ControlFlow::Continue(())
        });

        let req = rx.recv().unwrap();
        if let Operation::Read { vmo_offset, .. } = &req.operation {
            req.vmo.as_ref().unwrap().write(&test_data, *vmo_offset).unwrap();
        } else {
            panic!("Expected Operation::Read");
        }
        session_manager.complete_request(req.request_id, Ok(()));

        let result = recv.await.unwrap();
        assert_eq!(result, test_data);
    }

    #[fuchsia::test]
    async fn test_default_callback_block_service_read_aligned_range_failure() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));
        let service = session_manager.into_block_service(&session_manager);

        let encoded = (8u64 << 32) | 0u64;
        let extents = Extents::from_encoded([encoded], 0).unwrap();

        let (send, recv) = futures::channel::oneshot::channel();
        let send = Mutex::new(Some(send));

        let service_clone = service.clone();
        read_aligned_range(&extents, 0..4096, &*service_clone, move |buffer_result| {
            assert!(buffer_result.is_err());
            if let Some(s) = send.lock().take() {
                let _ = s.send(());
            }
            ControlFlow::Break(())
        });

        let req = rx.recv().unwrap();
        session_manager.complete_request(req.request_id, Err(zx::Status::IO));

        recv.await.unwrap();
    }
}
