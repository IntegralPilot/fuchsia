// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffers::{BufferGuard, BufferPool, TOTAL_SIZE};
use block_client::{BlockClient, BlockDeviceFlag, RemoteBlockClient, VmoId};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::num::NonZero;
use std::ops::Deref;
use std::sync::{Arc, Weak};

pub type Device = DeviceImpl<RemoteBlockClient>;

/// Wraps a `BlockClient` impl and manages registered VMO ids.
pub struct DeviceImpl<C: BlockClient + 'static> {
    client: C,
    vmo_ids: Mutex<HashMap<usize, Arc<VmoIdWrapperImpl<C>>>>,
    buffers: Arc<BufferPool>,
    private_buffers: Arc<BufferPool>,
    shared_vmo_id: VmoId,
}

impl<C: BlockClient> DeviceImpl<C> {
    pub async fn new(client: C) -> Result<Self, zx::Status> {
        let shared_vmo = zx::Vmo::create(TOTAL_SIZE as u64)?;

        // SAFETY: The shared VMO is newly created and only attached once here. All accesses to it
        // are managed by `BufferPool` and its guards, ensuring no aliasing issues during I/O.
        let shared_vmo_id = unsafe { client.attach_vmo(&shared_vmo) }.await?;

        let buffers = Arc::new(BufferPool::new(shared_vmo)?);

        let private_vmo = zx::Vmo::create(TOTAL_SIZE as u64)?;
        let private_buffers = Arc::new(BufferPool::new(private_vmo)?);

        Ok(Self { client, vmo_ids: Mutex::default(), buffers, private_buffers, shared_vmo_id })
    }

    pub fn shared_vmo_id(&self) -> &VmoId {
        &self.shared_vmo_id
    }

    pub fn block_flags(&self) -> BlockDeviceFlag {
        self.client.block_flags()
    }

    pub fn max_transfer_blocks(&self) -> Option<NonZero<u32>> {
        self.client.max_transfer_blocks()
    }

    /// Ataches `vmo`.  NOTE: This assumes that the pointer &zx::Vmo will remain stable.
    pub async fn attach_vmo(self: &Arc<Self>, vmo: &zx::Vmo) -> Result<(), zx::Status> {
        // SAFETY: FVM does not map this VMO, so it cannot violate Rust's aliasing rules. The
        // client is responsible for ensuring no references are held during I/O.
        let vmo_id = unsafe { self.client.attach_vmo(vmo) }.await?;

        assert!(
            self.vmo_ids
                .lock()
                .insert(
                    vmo as *const _ as usize,
                    Arc::new(VmoIdWrapperImpl(Arc::downgrade(self), vmo_id))
                )
                .is_none(),
            "VMO already attached!"
        );
        Ok(())
    }

    /// Deteaches `vmo`.  NOTE: The pointer `&zx::Vmo` must match that used in `attach_vmo`.
    pub fn detach_vmo(&self, vmo: &zx::Vmo) {
        // This won't immediately detach because it might still be in-use, but as soon as all uses
        // finish, it will get detached.
        self.vmo_ids.lock().remove(&(vmo as *const _ as usize));
    }

    /// Returns the VMO ID registered the given vmo.
    ///
    /// # Panics
    ///
    /// Panics if we don't know about `vmo` i.e. `attach_vmo` above was not called.
    pub fn get_vmo_id(&self, vmo: &zx::Vmo) -> Arc<VmoIdWrapperImpl<C>> {
        self.vmo_ids.lock()[&(vmo as *const _ as usize)].clone()
    }

    pub async fn get_buffer(&self) -> BufferGuard {
        self.buffers.get_buffer().await
    }

    pub async fn get_private_buffer(&self) -> BufferGuard {
        self.private_buffers.get_buffer().await
    }
}

impl<C: BlockClient + 'static> Drop for DeviceImpl<C> {
    fn drop(&mut self) {
        // Prevent VmoId drop panic. Safe to leak because the session client (C)
        // is being dropped, closing the connection to the block device.
        let _ = self.shared_vmo_id.take().into_id();
    }
}

impl Deref for Device {
    type Target = RemoteBlockClient;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

pub struct VmoIdWrapperImpl<C: BlockClient + 'static>(Weak<DeviceImpl<C>>, VmoId);

impl<C: BlockClient + 'static> Drop for VmoIdWrapperImpl<C> {
    fn drop(&mut self) {
        // Turn it into an ID so that if the spawned task is dropped, an assertion doesn't fire.  It
        // will mean the ID is leaked, but it's most likely that the server is being shut down
        // anyway so it shouldn't matter.
        let vmo_id = self.1.take().into_id();
        if let Some(device) = self.0.upgrade() {
            fasync::Task::spawn(async move {
                let _ = device.client.detach_vmo(VmoId::new(vmo_id)).await;
            })
            .detach();
        }
    }
}

impl<C: BlockClient> Deref for VmoIdWrapperImpl<C> {
    type Target = VmoId;
    fn deref(&self) -> &Self::Target {
        &self.1
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceImpl;
    use crate::buffers::{BUFFER_COUNT, BUFFER_SIZE};
    use assert_matches::assert_matches;
    use fake_block_client::FakeBlockClient;
    use fuchsia_async::{self as fasync, TestExecutor};
    use futures::FutureExt;
    use futures::future::select_all;
    use std::iter::repeat_with;
    use std::sync::Arc;
    use std::task::Poll;

    #[fuchsia::test(allow_stalls = false)]
    async fn test_get_buffer() {
        let fake_block_client = FakeBlockClient::new(1024, 1024);
        let device = Arc::new(DeviceImpl::new(fake_block_client).await.unwrap());
        let mut bufs = Vec::new();
        let mut offsets = Vec::new();

        fn check_no_overlap(offsets: &[u64], offset: u64) {
            for &o in offsets {
                assert!(offset >= o + BUFFER_SIZE as u64 || offset + BUFFER_SIZE as u64 <= o);
            }
        }

        for i in 0..BUFFER_COUNT {
            let mut buf = device.get_buffer().now_or_never().unwrap();
            check_no_overlap(&offsets, buf.vmo_offset());
            offsets.push(buf.vmo_offset());
            buf.as_mut_ptr_slice().fill(i as u8);
            bufs.push(buf);
        }

        // Check that all the buffers contain the expected fill.
        for (i, buf) in bufs.iter().enumerate() {
            assert_eq!(buf.as_ptr_slice().to_vec(), vec![i as u8; BUFFER_SIZE]);
        }

        // The next buffer we get should stall.
        let scope = fasync::Scope::new();
        let mut tasks: Vec<_> = repeat_with(|| {
            let device = device.clone();
            scope.compute(async move { device.get_buffer().await.vmo_offset() })
        })
        .take(2)
        .collect();

        assert!(TestExecutor::poll_until_stalled(select_all(&mut tasks)).await.is_pending());

        // Pop one buffer, and it should unblock the pending buffer.
        bufs.pop();
        offsets.pop();

        // Popping the buffer should have woken one of the tasks, but we don't know which one.
        // Randomly drop one of the tasks to test that dropping it still causes the other task to
        // wake up.
        let mut task = tasks.into_iter().skip(rand::random_range(0..2)).next().unwrap();

        assert_matches!(
            TestExecutor::poll_until_stalled(&mut task).await,
            Poll::Ready(offset) => check_no_overlap(&offsets, offset)
        );
    }
}
