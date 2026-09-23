// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceMode;
use crate::device::block::canonicalize_ioctl_request;
use crate::device::kobject::DeviceMetadata;
use crate::fs::sysfs::{BlockDeviceInfo, build_block_device_directory};
use crate::mm::MemoryAccessorExt;
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use crate::task::{CurrentTask, Kernel, KernelThreads};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{FileObject, FileOps, FsString, NamespaceNode, SeekTarget, default_seek};
use anyhow::{Context as _, Error};
use block_client::{BlockClient, BufferSlice, MutableBufferSlice, RemoteBlockClient};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_storage_block::BlockMarker;
use futures::channel::oneshot;
use futures::executor::block_on;
use starnix_sync::{LockDepMutex, RemoteBlockDeviceRegistryDevicesLock};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::device_id::{BLOCK_EXTENDED_MAJOR, DeviceId};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{MultiArchUserRef, UserRef};
use starnix_uapi::{BLKGETSIZE, BLKGETSIZE64, errno, error, from_status_like_fdio, off_t};
use std::collections::btree_map::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// A block device, backed by a partition hosted by Fuchsia.
pub struct RemoteBlockDevice {
    block_client: Arc<SyncBlockClient>,
}

impl RemoteBlockDevice {
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.block_client
            .read_at(MutableBufferSlice::Memory(buf), offset as u64)
            .context("read_at failed")
    }

    fn new(
        kernel: &Kernel,
        minor: u32,
        name: &str,
        block: ClientEnd<BlockMarker>,
    ) -> Result<Arc<Self>, Errno> {
        let registry = &kernel.device_registry;
        let device_name = FsString::from(name);
        let virtual_block_class = registry.objects.virtual_block_class();
        let block_client = SyncBlockClient::new(&kernel.kthreads, block)?;
        let device = Arc::new(Self { block_client });
        let device_weak = Arc::<RemoteBlockDevice>::downgrade(&device);
        registry.add_device(
            kernel,
            device_name.as_ref(),
            DeviceMetadata::new(
                device_name.clone(),
                DeviceId::new(BLOCK_EXTENDED_MAJOR, minor),
                DeviceMode::Block,
            )
            // It is not generally true that all remote block devices are disks, but it is true for
            // the ones we currently support. Future work should allow us to set this dynamically.
            .with_devtype("disk"),
            virtual_block_class,
            |device, dir| build_block_device_directory(device, device_weak, dir),
        )?;
        Ok(device)
    }

    pub fn create_file_ops(&self) -> Box<dyn FileOps> {
        Box::new(RemoteBlockDeviceFile { block_client: self.block_client.clone() })
    }
}

impl BlockDeviceInfo for RemoteBlockDevice {
    fn size(&self) -> Result<usize, Errno> {
        (self.block_client.block_count() as usize)
            .checked_mul(self.block_client.block_size() as usize)
            .ok_or_else(|| errno!(EINVAL))
    }
}

pub struct SyncBlockClient {
    client: Arc<RemoteBlockClient>,
    terminate_tx: Option<oneshot::Sender<()>>,
}

impl SyncBlockClient {
    fn new(kthreads: &KernelThreads, block: ClientEnd<BlockMarker>) -> Result<Arc<Self>, Errno> {
        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let (terminate_tx, terminate_rx) = oneshot::channel();

        // Spawn a thread to run the executor.
        let closure = move |_: &CurrentTask| async move {
            let proxy = block.into_proxy();
            match RemoteBlockClient::new(proxy).await {
                Ok(client) => {
                    let _ = init_tx.send(Ok(Arc::new(Self {
                        client: Arc::new(client),
                        terminate_tx: Some(terminate_tx),
                    })));
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return;
                }
            }
            // RemoteBlockClient::new() spawns a future on this closure's executor to handle the
            // block fifo. This keeps the executor alive and handling the fifo until the client is
            // dropped.
            let _ = terminate_rx.await;
        };

        let req = SpawnRequestBuilder::new()
            .with_debug_name("remote-block-client")
            .with_async_closure(closure)
            .build();
        kthreads.spawner().spawn_from_request(req);

        match init_rx.recv() {
            Ok(Ok(client)) => Ok(client),
            Ok(Err(status)) => Err(from_status_like_fdio!(status)),
            Err(_) => Err(errno!(EINVAL)),
        }
    }

    fn block_size(&self) -> u32 {
        self.client.block_size()
    }

    fn block_count(&self) -> u64 {
        self.client.block_count()
    }

    fn read_at(
        &self,
        buffer_slice: MutableBufferSlice<'_>,
        device_offset: u64,
    ) -> Result<(), zx::Status> {
        // TODO(https://fxbug.dev/475530917): block_on is uninterruptible. Once there is an
        // interruptible block_on, switch to that. For now this is okay because we expect the block
        // to be brief in most cases.
        block_on(self.client.read_at(buffer_slice, device_offset))
    }

    fn write_at(
        &self,
        buffer_slice: BufferSlice<'_>,
        device_offset: u64,
    ) -> Result<(), zx::Status> {
        // TODO(https://fxbug.dev/475530917): block_on is uninterruptible. Once there is an
        // interruptible block_on, switch to that. For now this is okay because we expect the block
        // to be brief in most cases.
        block_on(self.client.write_at(buffer_slice, device_offset))
    }

    fn flush(&self) -> Result<(), zx::Status> {
        // TODO(https://fxbug.dev/475530917): block_on is uninterruptible. Once there is an
        // interruptible block_on, switch to that. For now this is okay because we expect the block
        // to be brief in most cases.
        block_on(self.client.flush())
    }
}

impl Drop for SyncBlockClient {
    fn drop(&mut self) {
        if let Some(tx) = self.terminate_tx.take() {
            let _ = tx.send(());
        }
    }
}

struct RemoteBlockDeviceFile {
    block_client: Arc<SyncBlockClient>,
}

impl RemoteBlockDeviceFile {
    fn size(&self) -> Result<usize, Errno> {
        (self.block_client.block_count() as usize)
            .checked_mul(self.block_client.block_size() as usize)
            .ok_or_else(|| errno!(EINVAL))
    }
}

impl FileOps for RemoteBlockDeviceFile {
    fn has_persistent_offsets(&self) -> bool {
        true
    }

    fn is_seekable(&self) -> bool {
        true
    }

    // Manually implement seek, because default_eof_offset uses st_size (which is not used for block
    // devices).
    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        default_seek(current_offset, target, || self.size()?.try_into().map_err(|_| errno!(EINVAL)))
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        mut offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let size = self.size()?;
        let block_size = self.block_client.block_size() as usize;
        const MAX_CHUNK_SIZE: usize = 32 * 1024; // 32KB

        let mut total_read = 0;
        while data.available() > 0 && offset < size {
            let chunk_len = std::cmp::min(data.available(), MAX_CHUNK_SIZE);
            let chunk_len = std::cmp::min(chunk_len, size - offset);
            if chunk_len == 0 {
                break;
            }

            let aligned_offset = offset - offset % block_size;
            let end_offset = offset + chunk_len;
            let aligned_end_offset = std::cmp::min(
                end_offset.checked_next_multiple_of(block_size).ok_or_else(|| errno!(EINVAL))?,
                size,
            );
            let aligned_data_length = aligned_end_offset - aligned_offset;

            let mut read_data = vec![0u8; aligned_data_length];
            self.block_client
                .read_at(MutableBufferSlice::Memory(&mut read_data), aligned_offset as u64)
                .map_err(|status| from_status_like_fdio!(status))?;

            let read_offset = offset - aligned_offset;
            let read_end = read_offset + chunk_len;
            let bytes_written = data.write(&read_data[read_offset..read_end])?;

            offset += bytes_written;
            total_read += bytes_written;

            if bytes_written < chunk_len {
                break;
            }
        }
        Ok(total_read)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        mut offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let size = self.size()?;
        let block_size = self.block_client.block_size() as usize;
        const MAX_CHUNK_SIZE: usize = 32 * 1024; // 32KB

        let mut total_written = 0;
        while data.available() > 0 && offset < size {
            let chunk_len = std::cmp::min(data.available(), MAX_CHUNK_SIZE);
            let chunk_len = std::cmp::min(chunk_len, size - offset);
            if chunk_len == 0 {
                break;
            }

            let aligned_offset = offset - offset % block_size;
            let end_offset = offset + chunk_len;
            let aligned_end_offset = std::cmp::min(
                end_offset.checked_next_multiple_of(block_size).ok_or_else(|| errno!(EINVAL))?,
                size,
            );
            let aligned_data_length = aligned_end_offset - aligned_offset;

            let mut write_buf = vec![0u8; aligned_data_length];

            // Read-Modify-Write: If the write is not block-aligned at the start or end,
            // we need to read the existing data for the first and/or last block to preserve it.
            let head_unaligned = offset > aligned_offset;
            let tail_unaligned = end_offset < aligned_end_offset;

            if head_unaligned {
                self.block_client
                    .read_at(
                        MutableBufferSlice::Memory(&mut write_buf[..block_size]),
                        aligned_offset as u64,
                    )
                    .map_err(|status| from_status_like_fdio!(status))?;
            }

            if tail_unaligned {
                let last_block_start = aligned_data_length - block_size;
                // If we already read the first block and it's the same as the last block, don't
                // read again.
                if !head_unaligned || last_block_start > 0 {
                    self.block_client
                        .read_at(
                            MutableBufferSlice::Memory(&mut write_buf[last_block_start..]),
                            (aligned_offset + last_block_start) as u64,
                        )
                        .map_err(|status| from_status_like_fdio!(status))?;
                }
            }

            let write_offset = offset - aligned_offset;
            let write_slice = &mut write_buf[write_offset..write_offset + chunk_len];
            // SAFETY: We are writing to a buffer of u8, which is always initialized.
            // We can safely cast &mut [u8] to &mut [MaybeUninit<u8>].
            let write_slice_uninit = unsafe {
                std::slice::from_raw_parts_mut(
                    write_slice.as_mut_ptr() as *mut std::mem::MaybeUninit<u8>,
                    write_slice.len(),
                )
            };
            let bytes_read = data.read(write_slice_uninit)?;

            self.block_client
                .write_at(BufferSlice::Memory(&write_buf), aligned_offset as u64)
                .map_err(|status| from_status_like_fdio!(status))?;

            offset += bytes_read;
            total_written += bytes_read;

            if bytes_read < chunk_len {
                break;
            }
        }
        Ok(total_written)
    }

    fn sync(&self, _file: &FileObject, _current_task: &CurrentTask) -> Result<(), Errno> {
        self.block_client.flush().map_err(|status| from_status_like_fdio!(status))
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match canonicalize_ioctl_request(current_task, request) {
            BLKGETSIZE => {
                let user_size = MultiArchUserRef::<u64, u32>::new(current_task, arg);
                let size = self.block_client.block_count();
                current_task.write_multi_arch_object(user_size, size)?;
                Ok(SUCCESS)
            }
            BLKGETSIZE64 => {
                let user_size = UserRef::<u64>::from(arg);
                let size = self.size()? as u64;
                current_task.write_object(user_size, &size)?;
                Ok(SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}

fn open_remote_block_device(
    current_task: &CurrentTask,
    id: DeviceId,
    _node: &NamespaceNode,
    _flags: OpenFlags,
) -> Result<Box<dyn FileOps>, Errno> {
    Ok(current_task.kernel().remote_block_device_registry.open(id.minor())?.create_file_ops())
}

pub fn remote_block_device_init(kernel: &Kernel) {
    kernel
        .device_registry
        .register_major(
            "remote-block".into(),
            DeviceMode::Block,
            BLOCK_EXTENDED_MAJOR,
            open_remote_block_device,
        )
        .expect("remote block device register failed.");
}

#[derive(Default)]
pub struct RemoteBlockDeviceRegistry {
    devices:
        LockDepMutex<BTreeMap<u32, Arc<RemoteBlockDevice>>, RemoteBlockDeviceRegistryDevicesLock>,
    next_minor: AtomicU32,
}

impl RemoteBlockDeviceRegistry {
    pub fn create_remote_block_device(
        &self,
        kernel: &Kernel,
        name: &str,
        block: ClientEnd<BlockMarker>,
    ) -> Result<(), Error> {
        let mut devices = self.devices.lock();
        let minor = self.next_minor.fetch_add(1, Ordering::Relaxed);
        let device = RemoteBlockDevice::new(kernel, minor, name, block)?;
        devices.insert(minor, device);
        Ok(())
    }

    pub fn open(&self, minor: u32) -> Result<Arc<RemoteBlockDevice>, Errno> {
        self.devices.lock().get(&minor).ok_or_else(|| errno!(ENODEV)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{anon_test_file, map_object_anywhere, spawn_kernel_and_run};
    use crate::vfs::{SeekTarget, VecInputBuffer, VecOutputBuffer};
    use starnix_uapi::open_flags::OpenFlags;
    use starnix_uapi::{BLKGETSIZE, BLKGETSIZE64};
    use vmo_backed_block_server::VmoBackedServer;
    use zerocopy::FromBytes as _;

    #[::fuchsia::test]
    async fn test_remote_block_device_registry() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            remote_block_device_init(kernel);
            let registry = kernel.remote_block_device_registry.clone();
            let server =
                VmoBackedServer::new(2, 512, &[]).expect("Failed to create VmoBackedServer");
            let (client, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
            std::thread::spawn(move || {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async move {
                    use fidl::endpoints::RequestStream;
                    server.serve(server_end.into_stream().cast_stream()).await.unwrap();
                });
            });

            registry
                .create_remote_block_device(kernel, "test", client)
                .expect("create_remote_block_device failed.");

            let device = registry.open(0).expect("open failed.");
            let file = anon_test_file(&current_task, device.create_file_ops(), OpenFlags::RDWR);

            let arg_addr = map_object_anywhere(&current_task, &0u64);
            let mut arg = [0u8; 8];

            file.ioctl(&current_task, BLKGETSIZE64, arg_addr.into()).expect("ioctl failed");
            current_task.read_memory_to_slice(arg_addr, &mut arg).unwrap();
            assert_eq!(u64::read_from_bytes(&arg).unwrap(), 1024);

            file.ioctl(&current_task, BLKGETSIZE, arg_addr.into()).expect("ioctl failed");
            current_task.read_memory_to_slice(arg_addr, &mut arg).unwrap();
            assert_eq!(u64::read_from_bytes(&arg).unwrap(), 2);

            // Deliberately read with a non-block-aligned buffer size. These reads come from
            // uncontrolled sources so we need to be able to handle the alignment ourselves.
            let mut buf = VecOutputBuffer::new(256);
            file.read(&current_task, &mut buf).expect("read failed.");
            assert_eq!(buf.data(), &[0u8; 256]);

            let mut buf = VecInputBuffer::from(vec![1u8; 256]);
            file.seek(&current_task, SeekTarget::Set(0)).expect("seek failed");
            file.write(&current_task, &mut buf).expect("write failed.");

            let mut buf = VecOutputBuffer::new(256);
            file.seek(&current_task, SeekTarget::Set(0)).expect("seek failed");
            file.read(&current_task, &mut buf).expect("read failed.");
            assert_eq!(buf.data(), &[1u8; 256]);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_write_past_eof() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            remote_block_device_init(kernel);
            let registry = kernel.remote_block_device_registry.clone();
            let server =
                VmoBackedServer::new(2, 512, &[]).expect("Failed to create VmoBackedServer");
            let (client, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
            std::thread::spawn(move || {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async move {
                    use fidl::endpoints::RequestStream;
                    server.serve(server_end.into_stream().cast_stream()).await.unwrap();
                });
            });

            registry
                .create_remote_block_device(kernel, "test", client)
                .expect("create_remote_block_device failed.");

            let device = registry.open(0).expect("open failed.");
            let file = anon_test_file(&current_task, device.create_file_ops(), OpenFlags::RDWR);

            file.seek(&current_task, SeekTarget::End(0)).expect("seek failed");
            let mut buf = VecOutputBuffer::new(512);
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 0);

            let mut buf = VecInputBuffer::from(vec![1u8; 512]);
            assert_eq!(file.write(&current_task, &mut buf).expect("write failed."), 0);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_unaligned_read_write_spanning_blocks() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            remote_block_device_init(kernel);
            let registry = kernel.remote_block_device_registry.clone();
            // 3 blocks of 512 bytes = 1536 bytes
            let server =
                VmoBackedServer::new(3, 512, &[]).expect("Failed to create VmoBackedServer");
            let (client, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
            std::thread::spawn(move || {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async move {
                    use fidl::endpoints::RequestStream;
                    server.serve(server_end.into_stream().cast_stream()).await.unwrap();
                });
            });

            registry
                .create_remote_block_device(kernel, "test", client)
                .expect("create_remote_block_device failed.");

            let device = registry.open(0).expect("open failed.");
            let file = anon_test_file(&current_task, device.create_file_ops(), OpenFlags::RDWR);

            // Write spanning across block boundaries (e.g., from 510 to 1026)
            // Start at 510 (2 bytes before end of 1st block)
            // Write 516 bytes (2 bytes in 1st block, 512 bytes in 2nd block, 2 bytes in 3rd block)
            let mut buf = VecInputBuffer::from(vec![0xAAu8; 516]);
            file.seek(&current_task, SeekTarget::Set(510)).expect("seek failed");
            assert_eq!(file.write(&current_task, &mut buf).expect("write failed."), 516);

            // Read back the data
            let mut buf = VecOutputBuffer::new(516);
            file.seek(&current_task, SeekTarget::Set(510)).expect("seek failed");
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 516);
            assert_eq!(buf.data(), &[0xAAu8; 516]);

            // Verify surrounding data is still 0
            let mut buf = VecOutputBuffer::new(1);
            file.seek(&current_task, SeekTarget::Set(509)).expect("seek failed");
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 1);
            assert_eq!(buf.data(), &[0u8]);

            let mut buf = VecOutputBuffer::new(1);
            file.seek(&current_task, SeekTarget::Set(1026)).expect("seek failed");
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 1);
            assert_eq!(buf.data(), &[0u8]);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_exact_eof_boundary() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            remote_block_device_init(kernel);
            let registry = kernel.remote_block_device_registry.clone();
            // 2 blocks of 512 bytes = 1024 bytes
            let server =
                VmoBackedServer::new(2, 512, &[]).expect("Failed to create VmoBackedServer");
            let (client, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
            std::thread::spawn(move || {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async move {
                    use fidl::endpoints::RequestStream;
                    server.serve(server_end.into_stream().cast_stream()).await.unwrap();
                });
            });

            registry
                .create_remote_block_device(kernel, "test", client)
                .expect("create_remote_block_device failed.");

            let device = registry.open(0).expect("open failed.");
            let file = anon_test_file(&current_task, device.create_file_ops(), OpenFlags::RDWR);

            // Write ending exactly at EOF (1024)
            // Start at 1020, write 4 bytes
            let mut buf = VecInputBuffer::from(vec![0xBBu8; 4]);
            file.seek(&current_task, SeekTarget::Set(1020)).expect("seek failed");
            assert_eq!(file.write(&current_task, &mut buf).expect("write failed."), 4);

            // Read back
            let mut buf = VecOutputBuffer::new(4);
            file.seek(&current_task, SeekTarget::Set(1020)).expect("seek failed");
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 4);
            assert_eq!(buf.data(), &[0xBBu8; 4]);

            // Try to read past EOF from 1020 (request 5 bytes)
            let mut buf = VecOutputBuffer::new(5);
            file.seek(&current_task, SeekTarget::Set(1020)).expect("seek failed");
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 4);
            assert_eq!(buf.data()[..4], [0xBBu8; 4]);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_rmw_preserves_data() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            remote_block_device_init(kernel);
            let registry = kernel.remote_block_device_registry.clone();
            // 3 blocks of 512 bytes = 1536 bytes
            // Initialize with a known pattern (0xFF)
            let server = VmoBackedServer::new(3, 512, &[0xFFu8; 1536])
                .expect("Failed to create VmoBackedServer");
            let (client, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
            std::thread::spawn(move || {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async move {
                    use fidl::endpoints::RequestStream;
                    server.serve(server_end.into_stream().cast_stream()).await.unwrap();
                });
            });

            registry
                .create_remote_block_device(kernel, "test", client)
                .expect("create_remote_block_device failed.");

            let device = registry.open(0).expect("open failed.");
            let file = anon_test_file(&current_task, device.create_file_ops(), OpenFlags::RDWR);

            // Write a small chunk in the middle of the second block (offset 600, length 100)
            // Block 1 is 512-1024. 600 is inside.
            // This should trigger RMW for the second block.
            let mut buf = VecInputBuffer::from(vec![0xAAu8; 100]);
            file.seek(&current_task, SeekTarget::Set(600)).expect("seek failed");
            assert_eq!(file.write(&current_task, &mut buf).expect("write failed."), 100);

            // Read back the entire second block to verify
            let mut buf = VecOutputBuffer::new(512);
            file.seek(&current_task, SeekTarget::Set(512)).expect("seek failed");
            assert_eq!(file.read(&current_task, &mut buf).expect("read failed."), 512);
            let data = buf.data();

            // 512 to 600 (88 bytes) should be 0xFF
            assert_eq!(&data[0..88], &[0xFFu8; 88]);
            // 600 to 700 (100 bytes) should be 0xAA
            assert_eq!(&data[88..188], &[0xAAu8; 100]);
            // 700 to 1024 (324 bytes) should be 0xFF
            assert_eq!(&data[188..512], &[0xFFu8; 324]);
        })
        .await;
    }
}
