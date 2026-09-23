// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 cache operations and instruction synchronization.

use super::feature;

unsafe extern "C" {
    fn cpp_is_kernel_address(addr: usize) -> bool;
    fn cpp_arch_sync_cache_shootdown();
}

/// Perform a cache block operation over an address range using Zicbom instructions.
#[inline(always)]
fn cache_op(mut start: usize, len: usize, op: impl Fn(usize)) {
    if len == 0 {
        return;
    }
    // If the Zicbom feature is enabled, use the cbo* instructions.
    // If there is no Zicbom feature, the CPU is assumed to be coherent with
    // external DMA and not need any sort of cache flushing.
    if feature::has_zicbom() {
        let stride = feature::cbom_size() as usize;
        if stride == 0 {
            return;
        }
        let end = start.saturating_add(len);

        // Align the start address down to the stride (stride must be a power of 2).
        start &= !(stride - 1);

        while start < end {
            op(start);
            start += stride;
        }
    }
}

/// Execute a `cbo.clean` instruction on the cache block containing `addr`.
#[inline(always)]
fn cbo_clean(addr: usize) {
    // SAFETY: Low-level RISC-V Zicbom cache clean instruction.
    unsafe {
        core::arch::asm!("cbo.clean 0({0})", in(reg) addr, options(nostack, preserves_flags));
    }
}

/// Execute a `cbo.flush` instruction on the cache block containing `addr`.
#[inline(always)]
fn cbo_flush(addr: usize) {
    // SAFETY: Low-level RISC-V Zicbom cache flush instruction.
    unsafe {
        core::arch::asm!("cbo.flush 0({0})", in(reg) addr, options(nostack, preserves_flags));
    }
}

/// Execute a `cbo.inval` instruction on the cache block containing `addr`.
#[inline(always)]
fn cbo_inval(addr: usize) {
    // SAFETY: Low-level RISC-V Zicbom cache invalidate instruction.
    unsafe {
        core::arch::asm!("cbo.inval 0({0})", in(reg) addr, options(nostack, preserves_flags));
    }
}

/// Execute a `fence.i` instruction to synchronize instruction and data streams.
#[inline(always)]
fn fence_i() {
    // SAFETY: RISC-V instruction-fetch fence instruction.
    unsafe {
        core::arch::asm!("fence.i", options(nostack, preserves_flags));
    }
}

/// Using Zicbom instructions, clean the data cache over a range of memory.
#[unsafe(no_mangle)]
pub extern "C" fn arch_clean_cache_range(start: usize, len: usize) {
    debug_assert!(unsafe { cpp_is_kernel_address(start) });
    cache_op(start, len, cbo_clean);
}

/// Using Zicbom instructions, clean and invalidate the data cache over a range of memory.
#[unsafe(no_mangle)]
pub extern "C" fn arch_clean_invalidate_cache_range(start: usize, len: usize) {
    debug_assert!(unsafe { cpp_is_kernel_address(start) });
    cache_op(start, len, cbo_flush);
}

/// Using Zicbom instructions, invalidate the data cache over a range of memory.
#[unsafe(no_mangle)]
pub extern "C" fn arch_invalidate_cache_range(start: usize, len: usize) {
    debug_assert!(unsafe { cpp_is_kernel_address(start) });
    cache_op(start, len, cbo_inval);
}

/// Synchronize the instruction and data cache across all CPUs.
#[unsafe(no_mangle)]
pub extern "C" fn arch_sync_cache_range(start: usize, len: usize) {
    if unsafe { cpp_is_kernel_address(start) } {
        arch_clean_cache_range(start, len);
    }

    // Shootdown on all cores via cross-CPU IPI.
    unsafe { cpp_arch_sync_cache_shootdown() };
}
