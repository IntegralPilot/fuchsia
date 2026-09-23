// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Error;
use arbitrary::Unstructured;
use async_trait::async_trait;
use f2fs_reader::{BLOCK_SIZE, F2fsReader};
use fuchsia_runtime as _;
use fuzz::fuzz;
use std::ops::Range;
use std::sync::{Arc, LazyLock};
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ReadOptions, WriteOptions};

// Allocate the base image once in a VMO (~260MiB)
static BASE_IMAGE_VMO: LazyLock<zx::Vmo> = LazyLock::new(|| {
    let file =
        std::fs::File::open("/pkg/testdata/f2fs.img.zst").expect("failed to open f2fs.img.zst");
    let data = zstd::decode_all(file).expect("failed to decode f2fs.img.zst");
    let vmo = zx::Vmo::create(data.len() as u64).expect("failed to create VMO");
    vmo.write(&data, 0).expect("failed to write to VMO");
    vmo.set_name(&zx::Name::new("f2fs-base-image").unwrap()).ok();
    vmo
});

struct VmoBackedDevice {
    vmo: zx::Vmo,
    allocator: BufferAllocator,
}

#[async_trait]
impl Device for VmoBackedDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn block_count(&self) -> u64 {
        self.vmo.get_size().unwrap() / BLOCK_SIZE as u64
    }

    async fn read_with_opts(
        &self,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let mut vec = vec![0; buffer.len()];
        self.vmo.read(&mut vec, offset)?;
        buffer.copy_from_slice(&vec);
        Ok(())
    }

    async fn write_with_opts(
        &self,
        offset: u64,
        buffer: BufferRef<'_>,
        _write_opts: WriteOptions,
    ) -> Result<(), Error> {
        self.vmo.write(&buffer.to_vec(), offset)?;
        Ok(())
    }

    async fn trim(&self, _range: Range<u64>) -> Result<(), Error> {
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn supports_trim(&self) -> bool {
        false
    }
}
#[fuzz]
fn f2fs_reader_fuzzer(bytes: &[u8]) {
    if bytes.len() > 1024 * 1024 {
        return; // Oversized payload
    }

    // Create a CoW snapshot of the base image VMO.
    let base_vmo = &*BASE_IMAGE_VMO;
    let vmo_size = base_vmo.get_size().expect("failed to get VMO size");
    let child_vmo = base_vmo
        .create_child(zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, vmo_size)
        .expect("failed to create CoW VMO");

    let mut u = Unstructured::new(bytes);

    // Allow the fuzzer to specify a few block overlays.
    // We only take a maximum of 16 blocks to keep memory usage reasonable.
    for _ in 0..16 {
        let Ok(offset) = u.arbitrary::<u32>() else { break };
        let Ok(data) = u.arbitrary::<Vec<u8>>() else { break };
        if data.is_empty() || data.len() > BLOCK_SIZE {
            continue;
        }

        let write_offset = (offset as u64 / BLOCK_SIZE as u64) * BLOCK_SIZE as u64
            + (offset as u64 % BLOCK_SIZE as u64);
        if write_offset + data.len() as u64 <= vmo_size {
            let _ = child_vmo.write(&data, write_offset);
        }
    }

    let device = VmoBackedDevice {
        vmo: child_vmo,
        allocator: BufferAllocator::new(BLOCK_SIZE, BufferSource::new(16 * 1024 * 1024)),
    };

    let reader_result = futures::executor::block_on(F2fsReader::open_device(Arc::new(device)));
    if let Ok(reader) = reader_result {
        // We successfully parsed the SuperBlock and Checkpoint!

        // Let's do a few simple operations to fuzz the data structures:
        let _ = reader.root_ino();
        let _ = reader.max_ino();
        let _ = reader.summary_block_addr();

        // Always try to hit the root inode (guarantees directory coverage if the FS is vaguely valid),
        // plus one arbitrary fuzzer-chosen inode.
        let mut inodes_to_test = vec![reader.root_ino()];
        if let Ok(random_inode) = u.arbitrary::<u32>() {
            inodes_to_test.push(random_inode);
        }

        for target_inode in inodes_to_test {
            // Note that read_inode will try to read crypto context and xattr if they exist, so
            // while it looks like we don't test these here, we are still getting some coverage.
            if let Ok(_inode) = futures::executor::block_on(reader.read_inode(target_inode)) {
                // It was a valid inode!
                if let Ok(entries) = futures::executor::block_on(reader.readdir(target_inode)) {
                    for _ in entries {}
                }
                if let Ok(symlink) = reader.read_symlink(&_inode) {
                    let _ = symlink;
                }

                // Exercise the xattr parsing
                let _ = &_inode.xattr;

                // Try reading a few arbitrary data blocks instead of just one
                for _ in 0..4 {
                    if let Ok(block_idx) = u.arbitrary::<u32>() {
                        if let Ok(data) =
                            futures::executor::block_on(reader.read_data(&_inode, block_idx))
                        {
                            let _ = data;
                        }
                    }
                }
            }
        }
    }
}
