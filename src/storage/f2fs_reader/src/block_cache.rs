// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use lru_cache::LruCache;
use storage_device::Device;
use storage_device::buffer::Buffer;

pub struct BlockCache {
    cache: Mutex<LruCache<u32, Vec<u8>>>,
    block_size: usize,
}

impl BlockCache {
    pub fn new(capacity: usize, block_size: usize) -> Self {
        Self { cache: Mutex::new(LruCache::new(capacity)), block_size }
    }

    /// Read a block into a freshly allocated device buffer, if block is available.
    pub async fn get_buffer<'a>(
        &self,
        block_addr: u32,
        device: &'a dyn Device,
    ) -> Option<Buffer<'a>> {
        let has_block = {
            let mut cache = self.cache.lock();
            cache.get_mut(&block_addr).is_some()
        };
        if has_block {
            let mut block = device.allocate_buffer(self.block_size).await;
            let mut cache = self.cache.lock();
            if let Some(data) = cache.get_mut(&block_addr) {
                block.copy_from_slice(&*data);
                return Some(block);
            }
        }
        None
    }

    pub fn insert(&self, block_addr: u32, data: Vec<u8>) {
        if data.len() != self.block_size {
            // Don't cache blocks of the wrong size.
            return;
        }
        let mut cache = self.cache.lock();
        cache.insert(block_addr, data);
    }
}
