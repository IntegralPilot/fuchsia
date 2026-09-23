// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::timer::Timer;
use crate::kernel::types::cpu_num_t;
use crate::vm::page_state::VmPageCounts;
use core::mem::offset_of;
use core::sync::atomic::AtomicU64;
use percpu_bindings as bindings;

/// An opaque type representing the C++ `TimerQueue` class.
#[repr(transparent)]
pub struct TimerQueue(pub bindings::TimerQueue);

/// An opaque type representing the C++ `CpuSearchSet` class.
#[repr(transparent)]
pub struct CpuSearchSet(pub bindings::CpuSearchSet);

/// An opaque type representing the C++ `Scheduler` class.
#[repr(transparent)]
pub struct Scheduler(pub bindings::Scheduler);

#[cfg(with_lock_dep)]
/// An opaque type representing the C++ `lockdep::ThreadLockState` class.
#[repr(transparent)]
pub struct ThreadLockState(pub bindings::lockdep_ThreadLockState);

/// An opaque type representing the C++ `ChainLockTransaction` class.
#[repr(C)]
pub struct ChainLockTransaction {
    _private: [u8; 0],
}

/// Type alias for `guest_stats`.
pub type GuestStats = bindings::guest_stats;
#[allow(non_camel_case_types)]
pub type guest_stats = bindings::guest_stats;

/// Type alias for `cpu_stats`.
pub type CpuStats = bindings::cpu_stats;
#[allow(non_camel_case_types)]
pub type cpu_stats = bindings::cpu_stats;

/// An opaque type representing the C++ `IdlePowerThread` class.
#[repr(transparent)]
pub struct IdlePowerThread(pub bindings::IdlePowerThread);

/// An opaque type representing the C++ `DpcRunner` class.
#[repr(transparent)]
pub struct DpcRunner(pub bindings::DpcRunner);

/// An opaque type representing the C++ `StallAccumulator` class.
#[repr(transparent)]
pub struct StallAccumulator(pub bindings::StallAccumulator);

/// An opaque type representing the C++ `PlatformCpuResumeState` struct.
#[repr(transparent)]
pub struct PlatformCpuResumeState(pub bindings::PlatformCpuResumeState);

/// Rust representation of the C++ `struct percpu`.
#[repr(C)]
pub struct PerCpu {
    /// ZST to force alignment to target arch cache lines.
    _align: crate::arch_rs::CpuAlignMarker,
    /// Each CPU maintains a per-cpu queue of timers.
    pub timer_queue: TimerQueue,
    /// per cpu search set
    pub search_set: CpuSearchSet,
    /// per cpu scheduler
    pub scheduler: Scheduler,
    #[cfg(with_lock_dep)]
    /// state for runtime lock validation when in irq context
    pub lock_state: ThreadLockState,
    /// The state of the currently active chain-lock transaction if any, nullptr
    /// otherwise.
    pub active_cl_transaction: *mut ChainLockTransaction,
    /// The chainlock conflict ID generator for this CPU.  This field is used by
    /// threads to know when to retry a ChainLockTransaction when a conflict is
    /// encountered.  See lib/kconcurrent for more details.
    pub chain_lock_conflict_id: AtomicU64,
    /// guest entry/exit statistics
    pub gstats: GuestStats,
    /// thread/cpu level statistics
    pub stats: CpuStats,
    /// per cpu idle/power thread
    pub idle_power_thread: IdlePowerThread,
    /// kernel counters arena
    pub counters: *mut i64,
    /// Each cpu maintains a DpcRunner.
    pub dpc_runner: DpcRunner,
    /// Page state counts are percpu because they change frequently and we don't want to pay for either
    /// heavy synchronization, or transferring the counters between CPUs a lot. Using percpu relaxed
    /// atomics we minimize the need for synchronization (no locks, interrupt or preemption disabling
    /// needed), and avoid cache line bouncing the counters around different CPUs.
    ///
    /// While it's OK for an observer to temporarily see incorrect values, the counts need to
    /// eventually quiesce. It's important that we don't "drop" changes and that the values don't
    /// drift over time.
    ///
    /// When modifying it is not necessary to disable preemption, and can just use
    /// percpu::GetCurrent()| and then modify the counts. In the unlikely event a CPU migration happens
    /// this does not effect correctness, and paying for the rare cache line transfer is preferable to
    /// consistently paying to avoid migration.
    ///
    /// When reading, use |ForEachPreemptDisable|. Although it is not possible to guarantee a
    /// consistent snapshot of these counters, it should be good enough for diagnostic uses.
    pub vm_page_counts: VmPageCounts,
    /// lockup_detector state.
    ///
    /// Every active CPU wakes up periodically (even during periods of suspend-to-idle) to record a
    /// heartbeat, as well as to check to see if any of its peers are showing signs of problems.  The
    /// lockup detector timer is the timer used for this.
    ///
    /// This field is not a member of LockupDetectorState because Timer depends on
    /// SpinLock, which depends on lockup_detector.  By pulling it out of
    /// LockupDetectorState we can inline performance critical lockup_detector
    /// functions.  See also gLockupDetectorPerCpuState.
    pub lockup_detector_timer: Timer,
    /// When thread sampling is enabled, we use this timer to periodically mark that the active thread
    /// should be sampled when safe to do so.
    pub sampling_timer: Timer,
    /// The accumulated memory stall timers for this CPU.
    pub memory_stall_accumulator: StallAccumulator,
    /// See |PlatformResumeState| type declaration.
    pub resume_state: PlatformCpuResumeState,
}

// One of the members of the PerCpu has a zero sized type, which is not FFI compliant, however we
// are separately validating the equivalence of the final layout of the C++ and Rust objects so
// this is okay.
#[allow(improper_ctypes)]
unsafe extern "C" {
    // C++ name mangled form of `size_t percpu::processor_count_`
    #[link_name = "_ZN6percpu16processor_count_E"]
    /// Number of percpu entries.
    static PROCESSOR_COUNT: usize;
    // C++ name mangled form of `size_t percpu::processor_index_`
    #[link_name = "_ZN6percpu16processor_index_E"]
    /// Translates from CPU number to percpu instance. Some or all instances
    /// of percpu may be discontiguous.
    static PROCESSOR_INDEX: *const *const PerCpu;
}

// Compile-time layout assertions against C++ `struct percpu` via bindgen.
zr::static_assert!(size_of::<PerCpu>() == size_of::<bindings::percpu>());
zr::static_assert!(align_of::<PerCpu>() == align_of::<bindings::percpu>());

zr::static_assert!(offset_of!(PerCpu, timer_queue) == offset_of!(bindings::percpu, timer_queue));
zr::static_assert!(offset_of!(PerCpu, search_set) == offset_of!(bindings::percpu, search_set));
zr::static_assert!(offset_of!(PerCpu, scheduler) == offset_of!(bindings::percpu, scheduler));
#[cfg(with_lock_dep)]
zr::static_assert!(offset_of!(PerCpu, lock_state) == offset_of!(bindings::percpu, lock_state));
zr::static_assert!(
    offset_of!(PerCpu, active_cl_transaction)
        == offset_of!(bindings::percpu, active_cl_transaction)
);
zr::static_assert!(
    offset_of!(PerCpu, chain_lock_conflict_id)
        == offset_of!(bindings::percpu, chain_lock_conflict_id)
);
zr::static_assert!(offset_of!(PerCpu, gstats) == offset_of!(bindings::percpu, gstats));
zr::static_assert!(offset_of!(PerCpu, stats) == offset_of!(bindings::percpu, stats));
zr::static_assert!(
    offset_of!(PerCpu, idle_power_thread) == offset_of!(bindings::percpu, idle_power_thread)
);
zr::static_assert!(offset_of!(PerCpu, counters) == offset_of!(bindings::percpu, counters));
zr::static_assert!(offset_of!(PerCpu, dpc_runner) == offset_of!(bindings::percpu, dpc_runner));
zr::static_assert!(
    offset_of!(PerCpu, vm_page_counts) == offset_of!(bindings::percpu, vm_page_counts)
);
zr::static_assert!(
    offset_of!(PerCpu, lockup_detector_timer)
        == offset_of!(bindings::percpu, lockup_detector_timer)
);
zr::static_assert!(
    offset_of!(PerCpu, sampling_timer) == offset_of!(bindings::percpu, sampling_timer)
);
zr::static_assert!(
    offset_of!(PerCpu, memory_stall_accumulator)
        == offset_of!(bindings::percpu, memory_stall_accumulator)
);
zr::static_assert!(offset_of!(PerCpu, resume_state) == offset_of!(bindings::percpu, resume_state));

// Compile-time layout assertions for member types.
zr::static_assert!(size_of::<TimerQueue>() == size_of::<bindings::TimerQueue>());
zr::static_assert!(align_of::<TimerQueue>() == align_of::<bindings::TimerQueue>());

zr::static_assert!(size_of::<CpuSearchSet>() == size_of::<bindings::CpuSearchSet>());
zr::static_assert!(align_of::<CpuSearchSet>() == align_of::<bindings::CpuSearchSet>());

zr::static_assert!(size_of::<Scheduler>() == size_of::<bindings::Scheduler>());
zr::static_assert!(align_of::<Scheduler>() == align_of::<bindings::Scheduler>());

#[cfg(with_lock_dep)]
zr::static_assert!(size_of::<ThreadLockState>() == size_of::<bindings::lockdep_ThreadLockState>());
#[cfg(with_lock_dep)]
zr::static_assert!(
    align_of::<ThreadLockState>() == align_of::<bindings::lockdep_ThreadLockState>()
);

zr::static_assert!(size_of::<IdlePowerThread>() == size_of::<bindings::IdlePowerThread>());
zr::static_assert!(align_of::<IdlePowerThread>() == align_of::<bindings::IdlePowerThread>());

zr::static_assert!(size_of::<DpcRunner>() == size_of::<bindings::DpcRunner>());
zr::static_assert!(align_of::<DpcRunner>() == align_of::<bindings::DpcRunner>());

zr::static_assert!(size_of::<StallAccumulator>() == size_of::<bindings::StallAccumulator>());
zr::static_assert!(align_of::<StallAccumulator>() == align_of::<bindings::StallAccumulator>());

zr::static_assert!(
    size_of::<PlatformCpuResumeState>() == size_of::<bindings::PlatformCpuResumeState>()
);
zr::static_assert!(
    align_of::<PlatformCpuResumeState>() == align_of::<bindings::PlatformCpuResumeState>()
);

impl PerCpu {
    /// Returns a reference to the percpu instance for given CPU number.
    pub fn get(cpu_num: cpu_num_t) -> &'static Self {
        debug_assert!(cpu_num < Self::processor_count() as cpu_num_t);
        unsafe { &**PROCESSOR_INDEX.wrapping_add(cpu_num as usize) }
    }

    /// Returns a reference to the percpu instance for the calling CPU.
    pub fn get_current() -> &'static Self {
        unsafe { &*crate::arch_rs::get_curr_percpu() }
    }

    /// Returns the number of percpu instances.
    pub fn processor_count() -> usize {
        unsafe { PROCESSOR_COUNT }
    }

    /// Call |func| with the current CPU's percpu struct with preemption disabled.
    pub fn with_current_preempt_disable<F: FnOnce(&Self)>(func: F) {
        let _preempt_disable = crate::kernel::thread::AutoPreemptDisabler::new();
        func(Self::get_current());
    }

    /// Call |func| once per CPU with each CPU's percpu struct with preemption disabled.
    pub fn for_each_preempt_disable<F: FnMut(cpu_num_t, &Self)>(func: F) {
        let _preempt_disable = crate::kernel::thread::AutoPreemptDisabler::new();
        Self::for_each(func);
    }

    /// Call |func| once per CPU with each CPU's percpu struct.
    pub fn for_each<F: FnMut(cpu_num_t, &Self)>(mut func: F) {
        for cpu_num in 0..(Self::processor_count() as cpu_num_t) {
            func(cpu_num, Self::get(cpu_num));
        }
    }
}
