// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(feature = "rseq_backend")]

use fuchsia_rseq::{Rseq, RseqCriticalSection};
use std::arch::global_asm;
use std::cell::UnsafeCell;
use zx::sys::{zx_membarrier_sync_process_data, zx_rseq_t, zx_system_get_num_cpus};

/// Counters for a single CPU.
///
/// This struct is `repr(C)` to ensure a stable layout, which is important for RSEQ operations that
/// might access these fields from assembly or specific offsets.
#[derive(Debug)]
#[repr(C)]
struct PerCpuCount {
    /// The number of times a reader has started a critical section.
    begin: UnsafeCell<usize>,

    /// The number of times a reader has ended a critical section.
    end: UnsafeCell<usize>,
}

impl PerCpuCount {
    const fn new() -> Self {
        Self { begin: UnsafeCell::new(0), end: UnsafeCell::new(0) }
    }
}

/// Per-CPU state containing the counters.
#[derive(Debug)]
#[repr(C)]
struct PerCpuState {
    /// Two sets of counters are maintained, one for even generations and one for odd generations.
    counts: [PerCpuCount; 2],
}

impl PerCpuState {
    /// Creates a new `PerCpuState` with zeroed counters.
    const fn new() -> Self {
        Self { counts: [PerCpuCount::new(), PerCpuCount::new()] }
    }

    /// Returns a pointer to the `begin` counter for the given index.
    fn begin_counter(&self, index: usize) -> *mut usize {
        self.counts[index].begin.get()
    }

    /// Returns a pointer to the `end` counter for the given index.
    fn end_counter(&self, index: usize) -> *mut usize {
        self.counts[index].end.get()
    }
}

/// The maximum number of CPUs that the RCU implementation supports.
///
/// This is a compile-time constant because the `per_cpu_counts` array is statically allocated.
const MAX_CPUS: usize = 32;

/// Manages RCU read-side critical section counters across all CPUs.
///
/// This struct is `Sync` because the `UnsafeCell`s are only written by the owning CPU (guaranteed
/// by RSEQ) and read by other CPUs using volatile reads, which produces a consistent view of the
/// counters (with appropriate barriers).
unsafe impl Sync for RcuReadCounters {}
pub(crate) struct RcuReadCounters {
    /// Array of per-CPU states.
    per_cpu_counts: [PerCpuState; MAX_CPUS as usize],
}

impl RcuReadCounters {
    /// Creates a new `RcuReadCounters`.
    pub(crate) const fn new() -> Self {
        Self { per_cpu_counts: [const { PerCpuState::new() }; MAX_CPUS as usize] }
    }

    /// Returns the state for a specific CPU.
    fn get_state(&self, cpu: u32) -> &PerCpuState {
        &self.per_cpu_counts[cpu as usize]
    }

    /// Signals the start of a read-side critical section.
    ///
    /// This increments the `begin` counter for the current CPU. It uses RSEQ to ensure the
    /// increment is atomic with respect to the current CPU.
    pub(crate) fn begin(&self, index: usize) {
        unsafe {
            let rseq = Rseq::get();
            loop {
                let cpu = rseq.current_cpu();
                let counter = self.get_state(cpu).begin_counter(index);
                if rseq_add(&rseq, counter, 1, cpu) {
                    break;
                }
            }
        }
    }

    /// Signals the end of a read-side critical section.
    ///
    /// This increments the `end` counter for the current CPU. It uses RSEQ to ensure the increment
    /// is atomic with respect to the current CPU.
    pub(crate) fn end(&self, index: usize) {
        unsafe {
            let rseq = Rseq::get();
            loop {
                let cpu = rseq.current_cpu();
                let counter = self.get_state(cpu).end_counter(index);
                if rseq_add(&rseq, counter, 1, cpu) {
                    break;
                }
            }
        }
    }

    /// Checks if there are any active readers for a given index.
    ///
    /// This aggregates the counters across all CPUs. It reads the `end` counters first, executes a
    /// memory barrier, and then reads the `begin` counters. If the sum of `begin - end` is
    /// non-zero, it means there are active readers.
    ///
    /// This function can have false positives, which causes grace periods to take longer than
    /// necessary. However, it will never have false negatives.
    pub(crate) fn has_active(&self, index: usize) -> bool {
        let num_cpus = unsafe { zx_system_get_num_cpus() };

        debug_assert!(
            num_cpus <= MAX_CPUS as u32,
            "Number of CPUs ({}) exceeds MAX_CPUS ({})",
            num_cpus,
            MAX_CPUS
        );

        let mut sum = 0usize;

        // This barrier ensures that either a Reader in rcu_read_unlock sees our
        // advancer.store(WAITING) or we see the Reader's `end` store. If both writes are missed,
        // we'll sleep here due to seeing active readers and the Reader won't wake us due to seeing
        // advancer = IDLE.
        //
        // With the barrier there are two possibilities:
        //
        // i)  A Reader's `end` store is before the barrier. We'll see the end store and consider
        //     the reader no longer active. We won't seelp
        // ii) The Reader's `end` store is after the barrier. Then the Reader's advancer
        //     load is also after the barrier. So then the reader must then observe the WAITING
        //     that the advancer wrote before the barrier and will wake us.
        unsafe { zx_membarrier_sync_process_data() };

        // Phase 1: Subtract Ends (read before barrier)
        for cpu in 0..num_cpus {
            let end_ptr = self.get_state(cpu).counts[index].end.get();
            // SAFETY: Use volatile read to ensure a single instruction loads the data from memory
            // where it is being written to by another thread without atomic operations.
            sum = sum.wrapping_sub(unsafe { std::ptr::read_volatile(end_ptr) });
        }

        // This barrier ensures that if we see an increment to `end` (in Phase 1), we must also see
        // the corresponding increment to `begin` (in Phase 2). The `begin` increment is sequenced
        // before the `end` increment in the reader thread. This barrier forces all stores sequenced
        // before the interrupt point in the reader to be visible to us after the barrier.
        // Therefore, we will never underestimate the number of active readers.

        // SAFETY: this is a basic FFI call with no pre- or post-conditions.
        unsafe { zx_membarrier_sync_process_data() };

        // Phase 2: Add Begins (read after barrier)
        for cpu in 0..num_cpus {
            let begin_ptr = self.get_state(cpu).counts[index].begin.get();
            // SAFETY: Use volatile read to ensure a single instruction loads the data from memory
            // where it is being written to by another thread without atomic operations.
            sum = sum.wrapping_add(unsafe { std::ptr::read_volatile(begin_ptr) });
        }

        sum != 0
    }
}

/// Adds a value to a counter using Restartable Sequences (RSEQ).
///
/// This function attempts to atomically add `value` to the memory location pointed to by `counter`.
/// It succeeds only if the current CPU matches the expected `cpu` throughout the critical section.
///
/// # Safety
///
/// The caller must ensure that `counter` points to a per-CPU counter for the given CPU.
unsafe fn rseq_add(rseq: &Rseq, counter: *mut usize, value: usize, cpu: u32) -> bool {
    unsafe {
        let start = &rcu_rseq_add_start as *const u8 as u64;
        let post = &rcu_rseq_add_post as *const u8 as u64;
        let abort = &rcu_rseq_add_abort as *const u8 as u64;

        let cs = RseqCriticalSection::new(start, post - start, abort);
        let _scope = rseq.activate(cs);
        rcu_rseq_add(counter, value, rseq.as_ptr(), cpu)
    }
}

// These symbols are defined in assembly below.
unsafe extern "C" {
    static rcu_rseq_add_start: u8;
    static rcu_rseq_add_post: u8;
    static rcu_rseq_add_abort: u8;
    // This function must be declared `extern "C"` to ensure the compiler treats it as an opaque
    // call. This forces the compiler to respect memory ordering by preventing it from assuming
    // the function doesn't modify memory (i.e., treating it as a compiler barrier).
    fn rcu_rseq_add(counter: *mut usize, val: usize, rseq_ptr: *mut zx_rseq_t, cpu: u32) -> bool;
}

#[cfg(target_arch = "x86_64")]
global_asm!(
    ".pushsection .text.rcu_rseq_add,\"ax\",@progbits",
    ".globl rcu_rseq_add",
    ".globl rcu_rseq_add_start",
    ".globl rcu_rseq_add_post",
    ".globl rcu_rseq_add_abort",
    ".type rcu_rseq_add, @function",
    "rcu_rseq_add:",
    // RDI=counter, RSI=val, RDX=rseq_ptr, RCX=cpu
    "rcu_rseq_add_start:",
    "mov r8d, [rdx]",
    "cmp r8d, ecx",
    "jne rcu_rseq_add_abort",
    "add [rdi], rsi",
    "rcu_rseq_add_post:",
    "mov eax, 1",
    "ret",
    "rcu_rseq_add_abort:",
    "xor eax, eax",
    "ret",
    ".size rcu_rseq_add, . - rcu_rseq_add",
    ".popsection"
);

#[cfg(target_arch = "aarch64")]
global_asm!(
    ".pushsection .text.rcu_rseq_add,\"ax\",@progbits",
    ".globl rcu_rseq_add",
    ".globl rcu_rseq_add_start",
    ".globl rcu_rseq_add_post",
    ".globl rcu_rseq_add_abort",
    ".type rcu_rseq_add, @function",
    "rcu_rseq_add:",
    // x0=counter, x1=val, x2=rseq, x3=cpu
    "rcu_rseq_add_start:",
    "ldr w4, [x2]",
    "cmp w4, w3",
    "b.ne rcu_rseq_add_abort",
    "ldr x5, [x0]",
    "add x5, x5, x1",
    "str x5, [x0]",
    "rcu_rseq_add_post:",
    "mov w0, 1",
    "ret",
    "rcu_rseq_add_abort:",
    "mov w0, 0",
    "ret",
    ".size rcu_rseq_add, . - rcu_rseq_add",
    ".popsection"
);

#[cfg(target_arch = "riscv64")]
global_asm!(
    ".pushsection .text.rcu_rseq_add,\"ax\",@progbits",
    ".globl rcu_rseq_add",
    ".globl rcu_rseq_add_start",
    ".globl rcu_rseq_add_post",
    ".globl rcu_rseq_add_abort",
    ".type rcu_rseq_add, @function",
    "rcu_rseq_add:",
    // a0=counter, a1=val, a2=rseq, a3=cpu
    "rcu_rseq_add_start:",
    "lw t0, 0(a2)",
    "bne t0, a3, rcu_rseq_add_abort",
    "ld t1, 0(a0)",
    "add t1, t1, a1",
    "sd t1, 0(a0)",
    "rcu_rseq_add_post:",
    "li a0, 1",
    "ret",
    "rcu_rseq_add_abort:",
    "li a0, 0",
    "ret",
    ".size rcu_rseq_add, . - rcu_rseq_add",
    ".popsection"
);
