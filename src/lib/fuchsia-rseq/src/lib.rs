// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use std::mem;
use std::ptr::NonNull;
use std::sync::LazyLock;
use zx_types::zx_rseq_t;

/// The maximum number of threads supported by the RSEQ allocator.
///
/// We map the VMO linearly up front to keep the mappings contiguous in virtual memory.
/// This limit guarantees we don't exceed a reasonable amount of virtual memory.
const MAX_THREADS: u64 = 100_000;

/// Represents a unique memory location within the global RSEQ VMO assigned to a thread.
#[derive(Debug, Default, Clone, Copy)]
struct ThreadSlot {
    /// The offset in bytes from the start of the global RSEQ VMO mapping.
    offset: u64,
}

/// A synchronized allocator that manages unique thread slots in a single shared VMO.
struct Allocator {
    /// The backing virtual memory object mapped for RSEQ slots.
    vmo: zx::Vmo,
    /// The memory address where the `vmo` starts in the current process's virtual address space.
    mapped_addr: usize,
    /// A list of returned slots that are available for reuse by registering threads.
    free_list: Vec<ThreadSlot>,
    /// The next monotonically increasing slot index to be allocated.
    next_slot_index: u64,
    /// The distance in bytes between thread RSEQ slots.
    stride: u64,
}

/// Rounds up a given size to the next multiple of the operating system page size.
fn round_up_to_page_size(size: usize) -> usize {
    let page_size = zx::system_get_page_size() as usize;
    ((size + page_size - 1) / page_size) * page_size
}

impl Allocator {
    /// Creates a new `Allocator` mapping space for up to `MAX_THREADS` structures.
    fn new() -> Self {
        let stride = std::cmp::max(
            zx::system_get_dcache_line_size() as u64,
            mem::size_of::<zx_rseq_t>() as u64,
        );
        let needed_size = (MAX_THREADS as usize) * (stride as usize);
        let map_size = round_up_to_page_size(needed_size);
        let vmo = zx::Vmo::create(map_size as u64).expect("failed to create RSEQ VMO");
        let flags = zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE;
        let mapped_addr = fuchsia_runtime::vmar_root_self()
            .map(0, &vmo, 0, map_size, flags)
            .expect("failed to map RSEQ VMO");

        Self { vmo, mapped_addr, free_list: Vec::new(), next_slot_index: 0, stride }
    }

    /// Allocates a unique `ThreadSlot` from the free list or the unmapped pool.
    ///
    /// # Panics
    ///
    /// Panics if the globally managed pool exceeds `MAX_THREADS` (100,000 threads).
    fn allocate(&mut self) -> ThreadSlot {
        if let Some(slot) = self.free_list.pop() {
            return slot;
        }

        assert!(self.next_slot_index < MAX_THREADS, "RSEQ max thread count exceeded");
        let index = self.next_slot_index;
        self.next_slot_index += 1;
        let offset = index * self.stride;

        ThreadSlot { offset }
    }

    /// Returns an active thread slot's memory address back to the free list.
    fn free(&mut self, abi: *mut zx_rseq_t) {
        self.free_list.push(self.slot(abi));
    }

    /// Retrieves the raw VMO handle needed by the kernel to register threads.
    fn vmo_handle(&self) -> zx::sys::zx_handle_t {
        self.vmo.raw_handle()
    }

    /// Returns the `ThreadSlot` for the given pointer to `zx_rseq_t`.
    fn slot(&self, abi: *mut zx_rseq_t) -> ThreadSlot {
        let offset = (abi as usize - self.mapped_addr) as u64;
        ThreadSlot { offset }
    }

    /// Returns a pointer to the `zx_rseq_t` for the given slot.
    fn abi(&self, slot: &ThreadSlot) -> *mut zx_rseq_t {
        (self.mapped_addr + slot.offset as usize) as *mut zx_rseq_t
    }
}

static ALLOCATOR: LazyLock<Mutex<Allocator>> = LazyLock::new(|| Mutex::new(Allocator::new()));

thread_local! {
    /// The `zx_rseq_t` structure for the current thread, if any.
    ///
    /// This structure is used to store the restartable sequence information for the current thread.
    /// It is registered with the kernel when the thread is created and unregistered when the thread
    /// is destroyed.
    static RSEQ: std::cell::Cell<*mut zx_rseq_t> = Default::default();
}

/// The restartable sequence for the current thread.
///
/// This structure is used to store the restartable sequence information for the current thread.
/// It is registered with the kernel when the thread is created and unregistered when the thread
/// is destroyed.
#[derive(Debug, Clone, Copy)]
pub struct Rseq {
    abi: NonNull<zx_rseq_t>,
}

impl Rseq {
    /// Returns the restartable sequence for the current thread.
    ///
    /// # Panics
    ///
    /// Panics if the thread has not been registered for restartable sequences.
    /// See `rseq_register_thread()`.
    ///
    /// # Safety
    ///
    /// The returned object must not be used after the current thread calls
    /// `rseq_unregister_thread()`.
    pub unsafe fn get() -> Self {
        let abi = NonNull::new(RSEQ.with(|rseq| rseq.get())).expect("thread not registered");
        Self { abi }
    }

    /// Returns a pointer to the `zx_rseq_t` structure.
    ///
    /// The returned pointer is only valid until the current thread calls
    /// `rseq_unregister_thread()`.
    ///
    /// Useful for accessing the `zx_rseq_t` structure from inline assembly.
    pub fn as_ptr(&self) -> *mut zx_rseq_t {
        self.abi.as_ptr()
    }

    /// Reads the current CPU ID.
    ///
    /// The current CPU can change at any time. Useful for preparing for a restartable sequence.
    ///
    /// # Safety
    ///
    /// This method cannot be used after the current thread calls `rseq_unregister_thread()`. That
    /// invariant is required by the safety contract of `get()` as well.
    pub unsafe fn current_cpu(&self) -> u32 {
        let abi = self.as_ptr();
        unsafe {
            let cpu_id_ptr: *mut u32 = std::ptr::addr_of_mut!((*abi).cpu_id);
            // The CPU ID is only written by the kernel on this thread, which means a volatile
            // read is necessary and sufficient.
            std::ptr::read_volatile(cpu_id_ptr)
        }
    }

    /// Activate a critical section.
    ///
    /// The critical section is defined by the `critical_section` parameter. The critical section is
    /// active until the returned `RseqScope` is dropped.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned `RseqScope` is dropped before calling
    /// `rseq_unregister_thread()`.
    #[must_use = "The critical section is active only until the returned `RseqScope` is dropped."]
    pub unsafe fn activate(&self, critical_section: RseqCriticalSection) -> RseqScope {
        self.set_critical_section(critical_section);
        RseqScope { rseq: *self }
    }

    fn set_critical_section(&self, critical_section: RseqCriticalSection) {
        let abi = self.as_ptr();
        unsafe {
            let start_ip_ptr: *mut u64 = std::ptr::addr_of_mut!((*abi).start_ip);
            let post_commit_offset_ptr: *mut u64 =
                std::ptr::addr_of_mut!((*abi).post_commit_offset);
            let abort_ip_ptr: *mut u64 = std::ptr::addr_of_mut!((*abi).abort_ip);

            // The kernel reads these values on the current thread, which means a volatile
            // write is necessary and sufficient.

            // We first set the post-commit offset to 0, which prevents the kernel from entering
            // the critical section. Then we set the start and abort IPs, and finally the
            // post-commit offset, which enables the critical section.
            std::ptr::write_volatile(post_commit_offset_ptr, 0);
            std::ptr::write_volatile(start_ip_ptr, critical_section.start_ip);
            std::ptr::write_volatile(abort_ip_ptr, critical_section.abort_ip);
            std::ptr::write_volatile(post_commit_offset_ptr, critical_section.post_commit_offset);
        }
    }
}

/// The bounds of a critical section.
#[derive(Debug, Default, Clone, Copy)]
pub struct RseqCriticalSection {
    /// The address of the start of the critical section.
    start_ip: u64,

    /// The offset from the start of the critical section to the post-commit label.
    post_commit_offset: u64,

    /// The offset from the start of the critical section to the abort label.
    abort_ip: u64,
}

impl RseqCriticalSection {
    /// Creates a new critical section.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the `start_ip`, `post_commit_offset`, and `abort_ip`
    /// parameters correctly define the critical section.
    pub unsafe fn new(start_ip: u64, post_commit_offset: u64, abort_ip: u64) -> Self {
        Self { start_ip, post_commit_offset, abort_ip }
    }
}

/// A RAII guard that clears the critical section bounds when dropped.
pub struct RseqScope {
    rseq: Rseq,
}

impl Drop for RseqScope {
    fn drop(&mut self) {
        self.rseq.set_critical_section(RseqCriticalSection::default());
    }
}

/// Register the current thread for restartable sequences.
///
/// # Panics
///
/// Panics if the thread is already registered, if the maximum number of supported
/// threads (`MAX_THREADS`) has been exhausted, or if setting the thread RSEQ via syscall fails.
pub fn rseq_register_thread() {
    RSEQ.with(|rseq| {
        assert!(rseq.get().is_null(), "thread already registered");
    });

    let (vmo_handle, slot, abi) = {
        let mut allocator = ALLOCATOR.lock();

        let vmo_handle = allocator.vmo_handle();
        let slot = allocator.allocate();
        let abi = allocator.abi(&slot);

        (vmo_handle, slot, abi)
    };

    let status = unsafe {
        zx::sys::zx_thread_set_rseq(vmo_handle, slot.offset, mem::size_of::<zx_rseq_t>() as u64)
    };

    zx::Status::ok(status).expect("failed to register thread for RSEQ");

    RSEQ.with(|rseq| {
        rseq.set(abi);
    });
}

/// Unregister the current thread from the restartable sequence.
///
/// # Panics
///
/// Panics if the thread is not registered, or if unsetting the thread RSEQ via syscall fails.
pub fn rseq_unregister_thread() {
    let abi = RSEQ.with(|rseq| rseq.take());
    assert!(!abi.is_null(), "thread not registered");

    let status = unsafe { zx::sys::zx_thread_set_rseq(0, 0, 0) };
    zx::Status::ok(status).expect("failed to unregister thread from RSEQ");

    // Zero out the struct to let Zircon's zero-page scanner reclaim the page.
    unsafe {
        std::ptr::write_volatile(abi, mem::zeroed());
    }

    ALLOCATOR.lock().free(abi);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::arch::global_asm;

    unsafe extern "C" {
        fn rseq_test_critical_section(counters: *mut u64, rseq_ptr: *mut zx_rseq_t);
        static rseq_test_start: u8;
        static rseq_test_end: u8;
    }

    #[cfg(target_arch = "x86_64")]
    global_asm!(
        ".globl rseq_test_critical_section",
        ".globl rseq_test_start",
        ".globl rseq_test_end",
        "rseq_test_critical_section:",
        "rseq_test_start:",
        // Arguments: rdi = counters, rsi = rseq_ptr
        "mov ecx, [rsi]",      // READ cpu_id from rseq_ptr into ecx
        "shl rcx, 3",          // multiply by 8
        "add rcx, rdi",        // address of counter
        "inc qword ptr [rcx]", // increment counter
        "rseq_test_end:",
        "ret",
    );

    #[cfg(target_arch = "aarch64")]
    global_asm!(
        ".globl rseq_test_critical_section",
        ".globl rseq_test_start",
        ".globl rseq_test_end",
        "rseq_test_critical_section:",
        "rseq_test_start:",
        // x0 = counters, x1 = rseq_ptr
        "ldr w2, [x1]",   // read cpu_id
        "lsl x2, x2, #3", // multiply by 8
        "add x2, x0, x2", // address of counter
        "ldr x3, [x2]",
        "add x3, x3, #1",
        "str x3, [x2]",
        "rseq_test_end:",
        "ret",
    );

    #[cfg(target_arch = "riscv64")]
    global_asm!(
        ".globl rseq_test_critical_section",
        ".globl rseq_test_start",
        ".globl rseq_test_end",
        "rseq_test_critical_section:",
        "rseq_test_start:",
        // a0 = counters, a1 = rseq_ptr
        "lw t0, 0(a1)",   // read cpu_id
        "slli t0, t0, 3", // multiply by 8
        "add t0, a0, t0", // address of counter
        "ld t1, 0(t0)",
        "addi t1, t1, 1",
        "sd t1, 0(t0)",
        "rseq_test_end:",
        "ret",
    );

    struct TestRegistrationGuard;

    impl TestRegistrationGuard {
        fn new() -> Self {
            rseq_register_thread();
            Self
        }
    }

    impl Drop for TestRegistrationGuard {
        fn drop(&mut self) {
            RSEQ.with(|rseq| {
                if !rseq.get().is_null() {
                    rseq_unregister_thread();
                }
            });
        }
    }

    #[test]
    #[should_panic = "thread already registered"]
    fn test_double_registration_panics() {
        let _guard = TestRegistrationGuard::new();
        rseq_register_thread();
    }

    #[test]
    #[should_panic = "thread not registered"]
    fn test_unregistered_unregister_panics() {
        rseq_unregister_thread();
    }

    #[test]
    fn test_current_cpu() {
        let _guard = TestRegistrationGuard::new();
        unsafe {
            let rseq = Rseq::get();
            let cpu_id = rseq.current_cpu();
            assert_ne!(cpu_id, zx_types::ZX_INFO_INVALID_CPU);
        }
    }

    #[test]
    fn test_activate_field_propagation() {
        let _guard = TestRegistrationGuard::new();
        let rseq = unsafe { Rseq::get() };
        let cs = unsafe { RseqCriticalSection::new(10, 20, 30) };

        let scope = unsafe { rseq.activate(cs) };
        let abi = rseq.as_ptr();
        unsafe {
            assert_eq!((*abi).start_ip, 10);
            assert_eq!((*abi).abort_ip, 30);
            assert_eq!((*abi).post_commit_offset, 20);
        }

        drop(scope);
        unsafe {
            assert_eq!((*abi).start_ip, 0);
            assert_eq!((*abi).abort_ip, 0);
            assert_eq!((*abi).post_commit_offset, 0);
        }
    }

    #[test]
    fn test_rseq_per_cpu_counter() {
        let _guard = TestRegistrationGuard::new();

        let rseq = unsafe { Rseq::get() };
        let cpu_count = zx::system_get_num_cpus();
        let mut counters = vec![0u64; cpu_count as usize];

        unsafe {
            let start = &rseq_test_start as *const u8 as u64;
            let end = &rseq_test_end as *const u8 as u64;

            let cs = RseqCriticalSection::new(start, end - start, start);
            let _scope = rseq.activate(cs);

            // Execute the critical section
            rseq_test_critical_section(counters.as_mut_ptr(), rseq.as_ptr());
        }

        let sum = counters.iter().sum::<u64>();
        assert_eq!(sum, 1, "Sum of counters should be 1");
    }

    #[test]
    fn test_rseq_packing_and_reuse() {
        for _ in 0..10 {
            let _guard = TestRegistrationGuard::new();
        }

        let threads: Vec<_> = (0..200)
            .map(|_| {
                std::thread::spawn(|| {
                    let _guard = TestRegistrationGuard::new();
                    let cpu_id = unsafe { Rseq::get().current_cpu() };
                    assert_ne!(cpu_id, zx_types::ZX_INFO_INVALID_CPU);
                })
            })
            .collect();

        for t in threads {
            t.join().unwrap();
        }
    }
}
