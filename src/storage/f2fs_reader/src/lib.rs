// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
mod block_cache;
mod checkpoint;
mod crypto;
mod dir;
mod fsverity;
mod inode;
mod nat;
mod reader;
mod superblock;
mod xattr;

pub use checkpoint::{
    CKPT_FLAG_COMPACT_SUMMARY, CP_ORPHAN_PRESENT_FLAG, CheckpointHeader, ORPHANS_PER_BLOCK,
    OrphanBlock,
};
pub use dir::{DirEntry, FileType};
pub use fsverity::FsVerityDescriptor;
pub use inode::{AdviseFlags, Flags, InlineFlags, Inode, Mode};
pub use reader::{F2fsReader, NEW_ADDR, NULL_ADDR};
pub use superblock::{
    BLOCK_SIZE, F2FS_MAGIC, FEATURE_CASEFOLD, FEATURE_ENCRYPT, FEATURE_EXTRA_ATTR,
    FEATURE_PROJECT_QUOTA, FEATURE_QUOTA_INO, FEATURE_SB_CHKSUM, FEATURE_VERITY, SUPERBLOCK_OFFSET,
    SUPPORTED_FEATURES, SuperBlock, f2fs_crc32,
};
pub use xattr::{Index as XattrIndex, XattrEntry};

#[cfg(test)]
fn open_f2fs_test_image() -> storage_device::fake_device::FakeDevice {
    let compressed_image =
        std::fs::read("/pkg/testdata/f2fs.img.zst").expect("failed to read f2fs image");
    let decompressed_image = zstd::bulk::decompress(&compressed_image, 256 * 1024 * 1024)
        .expect("failed to decompress f2fs image");
    storage_device::fake_device::FakeDevice::from_vec(decompressed_image, BLOCK_SIZE as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Reader;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[fuchsia::test]
    async fn test_readahead() {
        let mut device = open_f2fs_test_image();
        let read_count = Arc::new(AtomicUsize::new(0));
        let read_count_clone = read_count.clone();

        device.set_op_callback(move |op| {
            if let storage_device::fake_device::Op::Read = op {
                read_count_clone.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        });

        let f2fs = F2fsReader::open_device(Arc::new(device)).await.expect("open ok");

        // Reset counter after initialization (initialization does some reads)
        read_count.store(0, Ordering::SeqCst);

        // Block 0x1000 = 4096.
        let start_block = 0x1000;

        // Read start_block. Should trigger readahead for start_block + 0..16.
        // Total 16 blocks.
        f2fs.read_raw_block(start_block).await.expect("read start_block");
        assert_eq!(read_count.load(Ordering::SeqCst), 1, "First read should trigger 1 device read");

        // Read next 3 blocks. Should be cached.
        for i in 1..4 {
            f2fs.read_raw_block(start_block + i).await.expect("read cached block");
            assert_eq!(
                read_count.load(Ordering::SeqCst),
                1,
                "Read {} should be cached",
                start_block + i
            );
        }

        // Read 16th block. Should trigger new readahead.
        f2fs.read_raw_block(start_block + 16).await.expect("read start_block + 16");
        assert_eq!(read_count.load(Ordering::SeqCst), 2, "Read should trigger 2nd device read");
    }
}
