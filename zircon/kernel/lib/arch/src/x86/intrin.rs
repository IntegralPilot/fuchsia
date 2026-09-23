// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::arch::asm;

#[cfg(target_arch = "x86")]
use core::arch::x86::{__cpuid, _rdtsc};
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{__cpuid, _rdtsc};

/// Yield the processor momentarily. This should be used in busy waits.
#[inline(always)]
pub fn r#yield() {
    // SAFETY: The `pause` instruction provides a hint to improve spin-wait loop performance
    // and does not modify register state or memory.
    unsafe {
        asm!("pause", options(nomem, nostack, preserves_flags));
    }
}

/// Convenience alias for [`r#yield`].
#[inline(always)]
pub fn yield_processor() {
    r#yield();
}

// TODO(https://fxbug.dev/42126965): Improve the docs on the barrier APIs, maybe rename/refine.

/// Synchronize all memory accesses of all kinds.
#[inline(always)]
pub fn device_memory_barrier() {
    // SAFETY: `mfence` serializes all memory load and store operations that precede the instruction.
    unsafe {
        asm!("mfence", options(nostack, preserves_flags));
    }
}

/// Synchronize the ordering of all memory accesses wrt other CPUs.
#[inline(always)]
pub fn thread_memory_barrier() {
    device_memory_barrier();
}

/// Ensure all stores that appear before this barrier (in program order) complete before any stores
/// that appear after this barrier.
#[inline(always)]
pub fn store_memory_barrier() {
    // No need to emit a fence instruction. Stores will not be re-ordered with other stores.
    // [intel/vol3]: 8.2.2 Memory Ordering in P6 and More Recent Processor Families
    // [amd/vol2]: 7.2 Multiprocessor Memory Access Ordering
    // SAFETY: A compiler barrier prevents compiler reordering across this point without emitting
    // machine instructions.
    unsafe {
        asm!("", options(nostack, preserves_flags));
    }
}

/// Force the processor to complete all modifications to register state and
/// memory by previous instructions (including draining any buffered writes)
/// before the next instruction is fetched.
///
/// [intel/vol3]: 8.3  Serializing Instructions.
/// [amd/vol2]: 7.6.4  Serializing Instructions.
///
/// `cpuid` is a serializing instruction.
#[inline(always)]
pub fn serialize_instructions() {
    let _ = __cpuid(0);
}

/// Return the current CPU cycle count.
#[inline(always)]
pub fn cycles() -> u64 {
    // SAFETY: `_rdtsc` reads the current CPU timestamp counter.
    unsafe { _rdtsc() }
}
