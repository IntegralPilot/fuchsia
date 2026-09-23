// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx;

pub enum BarrierType {
    /// Issues a data memory barrier on all running threads
    DataMemory,

    /// Issues a memory barrier and serializes the instruction stream on all running threads.
    InstructionStream,
}

/// Issues a barrier of the requested type on all running threads.
///
/// Wraps the `zx_system_barrier` syscall.
pub fn system_barrier(barrier_type: BarrierType) {
    match barrier_type {
        BarrierType::DataMemory => {
            // SAFETY: This wraps the zx_membarrier_sync_process_data() syscall which is safe.
            unsafe { zx::sys::zx_membarrier_sync_process_data() }
        }
        BarrierType::InstructionStream => {
            // SAFETY: This wraps the zx_membarrier_sync_process_insn() syscall which is safe.
            unsafe { zx::sys::zx_membarrier_sync_process_insn() }
        }
    }
}
