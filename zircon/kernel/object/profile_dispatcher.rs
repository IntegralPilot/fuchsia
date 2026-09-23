// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;
use fbl::Canary;
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_PROFILE, ZX_PRIORITY_DEFAULT, ZX_PRIORITY_HIGH, ZX_PROFILE_INFO_FLAG_CPU_MASK,
    ZX_PROFILE_INFO_FLAG_DEADLINE, ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY,
    ZX_PROFILE_INFO_FLAG_PRIORITY, ZX_RIGHT_APPLY_PROFILE, ZX_RIGHT_DUPLICATE, ZX_RIGHT_TRANSFER,
    zx_cpu_set_t, zx_profile_info_t, zx_rights_t,
};

use super::KernelHandle;
use super::profile_dispatcher_ffi::{
    cpp_profile_dispatcher_create, cpp_profile_dispatcher_validate_and_create_profile,
};
use super::thread_dispatcher::ThreadDispatcher;
use super::vm_address_region_dispatcher::{MemoryPriority, VmAddressRegionDispatcher};
use crate::counters;
use crate::kernel::scheduler_state::SchedulerStateBaseProfile;
use crate::kernel::types::cpu_mask_t;

use object_constants_rs as object_constants;

/// Default rights for a ProfileDispatcher handle.
pub const DEFAULT_RIGHTS: zx_rights_t =
    ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_APPLY_PROFILE;

zr::static_assert_size_and_align!(
    ProfileDispatcherState,
    object_constants::kProfileDispatcherStateSize,
    object_constants::kProfileDispatcherStateAlign,
);

counters::define_kcounter!(DISPATCHER_PROFILE_CREATE_COUNT, "dispatcher.profile.create", Sum);
counters::define_kcounter!(DISPATCHER_PROFILE_DESTROY_COUNT, "dispatcher.profile.destroy", Sum);

fn parse_cpu_mask(set: &zx_cpu_set_t) -> cpu_mask_t {
    // The code below only supports reading up to 1 word in the mask.
    zr::static_assert!(counters::SMP_MAX_CPUS <= core::mem::size_of::<u64>() * 8);
    zr::static_assert!(counters::SMP_MAX_CPUS <= core::mem::size_of::<cpu_mask_t>() * 8);
    zr::static_assert!(counters::SMP_MAX_CPUS <= zx_types::ZX_CPU_SET_MAX_CPUS);

    // We throw away any bits beyond SMP_MAX_CPUs.
    (set.mask[0] as cpu_mask_t) & crate::kernel::bits::bit_mask_u32(counters::SMP_MAX_CPUS)
}

fn validate_and_create_profile(
    info: &zx_profile_info_t,
) -> Result<SchedulerStateBaseProfile, Status> {
    let mut profile = MaybeUninit::uninit();
    // SAFETY: info and profile pointers are valid.
    let status = unsafe {
        cpp_profile_dispatcher_validate_and_create_profile(info as *const _, &mut profile)
    };
    Status::ok(status)?;
    // SAFETY: On success, cpp_profile_dispatcher_validate_and_create_profile() initializes
    // profile
    Ok(unsafe { profile.assume_init() })
}

fn parse_memory_priority(info: &zx_profile_info_t) -> Result<MemoryPriority, Status> {
    // SAFETY: info.flags has ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY set.
    let priority = unsafe { info.zx_profile_info_union.priority_params.priority };
    if priority == ZX_PRIORITY_HIGH {
        Ok(MemoryPriority::High)
    } else if priority == ZX_PRIORITY_DEFAULT {
        Ok(MemoryPriority::Default)
    } else {
        Err(Status::INVALID_ARGS)
    }
}

const SCHED_FLAGS: u32 = ZX_PROFILE_INFO_FLAG_PRIORITY | ZX_PROFILE_INFO_FLAG_DEADLINE;
const AFFINITY_FLAGS: u32 = ZX_PROFILE_INFO_FLAG_CPU_MASK;
const THREAD_FLAGS: u32 = SCHED_FLAGS | AFFINITY_FLAGS;
const MEMORY_FLAGS: u32 = ZX_PROFILE_INFO_FLAG_MEMORY_PRIORITY;
const REQUIRED_FLAGS: u32 = THREAD_FLAGS | MEMORY_FLAGS;

pub fn validate_profile_info(info: &zx_profile_info_t) -> Result<(), Status> {
    if (info.flags & REQUIRED_FLAGS) == 0 {
        return Err(Status::INVALID_ARGS);
    }

    if (info.flags & MEMORY_FLAGS) != 0 && (info.flags & THREAD_FLAGS) != 0 {
        return Err(Status::INVALID_ARGS);
    }

    if (info.flags & SCHED_FLAGS) != 0 {
        let _ = validate_and_create_profile(info)?;
    }

    if (info.flags & MEMORY_FLAGS) != 0 {
        let _ = parse_memory_priority(info)?;
    }

    Ok(())
}

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct ProfileDispatcherState {
    canary: Canary<{ fbl::magic(b"PROF") }>,

    profile: Option<SchedulerStateBaseProfile>,
    cpu_mask: Option<cpu_mask_t>,
    memory_priority: Option<MemoryPriority>,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl ProfileDispatcherState {
    pub fn init(
        _dispatcher: *const ProfileDispatcher,
        info: &zx_profile_info_t,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        let profile = if (info.flags & SCHED_FLAGS) != 0 {
            Some(validate_and_create_profile(info).expect("pre-validated profile"))
        } else {
            None
        };

        let cpu_mask = if (info.flags & AFFINITY_FLAGS) != 0 {
            Some(parse_cpu_mask(&info.cpu_affinity_mask))
        } else {
            None
        };

        let memory_priority = if (info.flags & MEMORY_FLAGS) != 0 {
            Some(parse_memory_priority(info).expect("pre-validated memory priority"))
        } else {
            None
        };

        DISPATCHER_PROFILE_CREATE_COUNT.add(1);

        pin_init!(Self {
            canary: Canary::new(),
            profile,
            cpu_mask,
            memory_priority,
            lock <- KMutex::init(),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for ProfileDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_PROFILE_DESTROY_COUNT.add(1);
    }
}

super::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct ProfileDispatcher,
    ProfileDispatcherState,
    ZX_OBJ_TYPE_PROFILE,
    object_constants::kProfileDispatcherStateOffset
);

impl ProfileDispatcher {
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    pub fn create(info: &zx_profile_info_t) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        validate_profile_info(info)?;
        // SAFETY: `cpp_profile_dispatcher_create` initializes `handle` on success.
        let handle = unsafe {
            KernelHandle::create(|out| cpp_profile_dispatcher_create(info as *const _, out))
        }?;
        Ok((handle, DEFAULT_RIGHTS))
    }

    pub fn apply_profile_to_thread(&self, thread: &ThreadDispatcher) -> Result<(), Status> {
        let state = self.state();
        if let Some(ref profile) = state.profile {
            thread.set_base_profile(profile)?;
        }

        if let Some(mask) = state.cpu_mask {
            thread.set_soft_affinity(mask)?;
        }

        Ok(())
    }

    pub fn apply_profile_to_vmar(&self, vmar: &VmAddressRegionDispatcher) -> Result<(), Status> {
        let state = self.state();
        if let Some(memory_priority) = state.memory_priority {
            vmar.set_memory_priority(memory_priority)?;
        }
        Ok(())
    }
}
