// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use heapdump_vmo::allocations_table_v1::{AllocationsTableWriter, ResourceKey};
/// We cap the size of our backing VMO at 2 GiB, then preallocate it and map it entirely.
/// Actual memory for each page will only be committed when we first write to that page.
const VMO_SIZE: usize = 1 << 31;

const VMO_NAME: zx::Name = zx::Name::new_lossy("heapdump-allocations");

/// Tracks live allocations by storing their metadata in a dedicated VMO.
pub struct AllocationsTable {
    vmo: zx::Vmo,
    writer: AllocationsTableWriter,
}

impl Default for AllocationsTable {
    fn default() -> AllocationsTable {
        let vmo = zx::Vmo::create(VMO_SIZE as u64).expect("failed to create allocations VMO");
        vmo.set_name(&VMO_NAME).expect("failed to set VMO name");

        // SAFETY: Nobody else will directly access this VMO. Even when we share it with the
        // collector, it will always take a snapshot first and then read the snapshot instead of the
        // original one.
        let writer = unsafe { AllocationsTableWriter::new(&vmo).expect("failed to create writer") };
        AllocationsTable { vmo, writer }
    }
}

impl AllocationsTable {
    /// Duplicate the handle to the underlying VMO.
    pub fn share_vmo(&self) -> zx::Vmo {
        self.vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("failed to share allocations VMO")
    }

    // Take a snapshot of the underlying VMO.
    pub fn snapshot_vmo(&self) -> zx::Vmo {
        self.vmo
            .create_child(
                zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE,
                0,
                VMO_SIZE as u64,
            )
            .expect("failed to snapshot allocations VMO")
    }

    pub fn try_record_allocation(
        &mut self,
        address: u64,
        size: u64,
        thread_info_key: ResourceKey,
        stack_trace_key: ResourceKey,
        timestamp: zx::MonotonicInstant,
    ) -> bool {
        self.writer
            .insert_allocation(address, size, thread_info_key, stack_trace_key, timestamp)
            .expect("out of space")
    }

    pub fn update_allocation(
        &mut self,
        address: u64,
        size: u64,
        thread_info_key: ResourceKey,
        stack_trace_key: ResourceKey,
        timestamp: zx::MonotonicInstant,
    ) -> u64 {
        if let Some(old_size) = self
            .writer
            .replace_allocation(address, size, thread_info_key, stack_trace_key, timestamp)
            .expect("out of space")
        {
            old_size
        } else {
            panic!("Block 0x{:x} was not allocated", address);
        }
    }

    pub fn forget_allocation(&mut self, address: u64) -> u64 {
        if let Some(size) = self.writer.erase_allocation(address) {
            size
        } else {
            panic!("Block 0x{:x} was not allocated", address);
        }
    }
}
