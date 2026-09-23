// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Types and structures for kernel scheduler state (`include/kernel/scheduler_state.h`).

use core::mem::MaybeUninit;
use object_constants_rs as object_constants;

/// The scheduling state of a thread matching C++ `enum thread_state : uint8_t`.
// LINT.IfChange(ThreadStateKind)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadStateKind {
    /// Thread has been created but not yet made runnable.
    Initial = 0,
    /// Thread is ready to run and in a run queue.
    Ready = 1,
    /// Thread is currently running on a CPU.
    Running = 2,
    /// Thread is blocked waiting on an event, mutex, or condition.
    Blocked = 3,
    /// Thread is blocked on a read lock.
    BlockedReadLock = 4,
    /// Thread is sleeping.
    Sleeping = 5,
    /// Thread is suspended.
    Suspended = 6,
    /// Thread is dead and awaiting cleanup.
    Death = 7,
}

// LINT.ThenChange(//zircon/kernel/kernel/thread_ffi.cc:thread_state)

zr::static_assert!(core::mem::size_of::<ThreadStateKind>() == 1);
zr::static_assert!(core::mem::align_of::<ThreadStateKind>() == 1);

/// Opaque wrapper around C++ `SchedulerState::BaseProfile`.
#[repr(C, align(8))]
pub struct SchedulerStateBaseProfile(
    zr::OpaqueBytes<{ object_constants::kSchedulerStateBaseProfileSize }>,
);

zr::static_assert_size_and_align!(
    SchedulerStateBaseProfile,
    object_constants::kSchedulerStateBaseProfileSize,
    object_constants::kSchedulerStateBaseProfileAlign,
);

unsafe extern "C" {
    fn cpp_scheduler_state_base_profile_init_fair(
        out_profile: *mut MaybeUninit<SchedulerStateBaseProfile>,
        priority: i32,
        inheritable: bool,
    );
}

// thread priority
/// Number of priority levels.
pub const NUM_PRIORITIES: i32 = 32;
/// Lowest priority level.
pub const LOWEST_PRIORITY: i32 = 0;
/// Highest priority level.
pub const HIGHEST_PRIORITY: i32 = NUM_PRIORITIES - 1;
/// Priority level for DPC threads.
pub const DPC_PRIORITY: i32 = NUM_PRIORITIES - 2;
/// Priority level for idle threads.
pub const IDLE_PRIORITY: i32 = LOWEST_PRIORITY;
/// Low priority level.
pub const LOW_PRIORITY: i32 = NUM_PRIORITIES / 4;
/// Default thread priority level.
pub const DEFAULT_PRIORITY: i32 = NUM_PRIORITIES / 2;
/// High priority level.
pub const HIGH_PRIORITY: i32 = (NUM_PRIORITIES / 4) * 3;

impl SchedulerStateBaseProfile {
    /// Creates a fair `SchedulerStateBaseProfile` with the given priority and inheritable flag.
    pub fn fair(priority: i32, inheritable: bool) -> Self {
        let mut profile = MaybeUninit::uninit();
        // SAFETY: `cpp_scheduler_state_base_profile_init_fair` unconditionally initializes
        // a scheduler state into the provided storage.
        unsafe {
            cpp_scheduler_state_base_profile_init_fair(&mut profile, priority, inheritable);
            profile.assume_init()
        }
    }

    /// Returns a raw pointer to the underlying byte storage.
    pub fn get(&self) -> *mut [u8; object_constants::kSchedulerStateBaseProfileSize] {
        self.0.get()
    }
}
