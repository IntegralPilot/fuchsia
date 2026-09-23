// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;

use crate::object::{
    Dispatcher, EventDispatcher, HandleValue, JobDispatcher, ProcessDispatcher,
    validate_system_resource,
};
use crate::user_copy::{UserInPtr, UserOutPtr};
use boot_options::BootOptions;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::*;

const ZX_DEFAULT_SYSTEM_EVENT_LOW_MEMORY_RIGHTS: zx_rights_t =
    ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_WAIT;

const ZX_SYSTEM_SUSPEND_OPTION_DISCARD: u64 = 1 << 0;
const ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY: u64 = 1 << 1;
const VALID_SUSPEND_FLAGS: u64 =
    ZX_SYSTEM_SUSPEND_OPTION_DISCARD | ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY;

const HALT_ACTION_REBOOT: u32 = 1;
const HALT_ACTION_REBOOT_BOOTLOADER: u32 = 2;
const HALT_ACTION_REBOOT_RECOVERY: u32 = 3;
const HALT_ACTION_SHUTDOWN: u32 = 4;

zr::static_assert!(core::mem::size_of::<zx_system_powerctl_arg_t>() == 12);
zr::static_assert!(core::mem::align_of::<zx_system_powerctl_arg_t>() == 4);
zr::static_assert!(core::mem::size_of::<zx_wake_source_report_header_t>() == 24);
zr::static_assert!(core::mem::align_of::<zx_wake_source_report_header_t>() == 8);
zr::static_assert!(core::mem::size_of::<zx_wake_source_report_entry_t>() == 72);
zr::static_assert!(core::mem::align_of::<zx_wake_source_report_entry_t>() == 8);

// Allocate this many extra bytes at the end of the bootdata for the platform
// to fill in with platform specific boot structures.
pub const BOOTDATA_PLATFORM_EXTRA_BYTES: usize = page::SIZE * 4;

pub const MEMORY_STALL_MAX_WINDOW: zx_duration_mono_t = 10 * 1_000_000_000;

unsafe extern "C" {
    fn cpp_system_mexec_payload_get_helper(
        buffer: *mut MaybeUninit<u8>,
        buffer_size: usize,
        out_zbi_size: *mut usize,
    ) -> zx_status_t;
    fn cpp_system_mexec_core(
        resource: zx_handle_t,
        kernel_vmo: zx_handle_t,
        data_zbi_vmo: zx_handle_t,
    ) -> zx_status_t;
    fn cpp_percpu_processor_count() -> usize;
    fn cpp_scheduler_update_processing_rates(info: *mut zx_cpu_performance_info_t, count: usize);
    fn cpp_scheduler_update_processing_limits(info: *mut zx_cpu_perf_limit_t, count: usize);
    fn cpp_scheduler_get_performance_scales(info: *mut zx_cpu_performance_info_t, count: usize);
    fn cpp_scheduler_get_default_performance_scales(
        info: *mut zx_cpu_performance_info_t,
        count: usize,
    );
    fn cpp_scheduler_get_processing_limits(info: *mut zx_cpu_perf_limit_t, count: usize);

    fn cpp_mp_hotplug_cpu_mask_all() -> zx_status_t;
    fn cpp_mp_unplug_cpu_mask_all_but_primary() -> zx_status_t;
    fn cpp_platform_graceful_halt_helper(action: u32);
    fn cpp_halt_token_ack_pending_halt() -> zx_status_t;
    #[cfg(target_arch = "x86_64")]
    fn cpp_system_powerctl_x86_set_pkg_pl1(arg: &zx_system_powerctl_arg_t) -> zx_status_t;
    fn cpp_wake_vector_discard_wake_event_report();
    fn cpp_idle_power_thread_transition_all_active_to_suspend(
        resume_deadline: zx_instant_boot_t,
    ) -> zx_instant_boot_t;
    fn cpp_wake_vector_generate_wake_event_report(
        start_time: zx_instant_boot_t,
        out_header: *mut zx_wake_source_report_header_t,
        out_entries: *mut zx_wake_source_report_entry_t,
        num_entries: u32,
        actual_entries: *mut u32,
    ) -> zx_status_t;
}

#[syscall]
pub fn sys_system_mexec_payload_get(
    resource: HandleValue,
    user_buffer: UserOutPtr<u8>,
    buffer_size: usize,
) -> Result<(), Status> {
    if !BootOptions::get().enable_debugging_syscalls {
        return Err(Status::NOT_SUPPORTED);
    }
    // Highly privileged, only mexec resource should have access.
    validate_system_resource(resource, ZX_RSRC_SYSTEM_MEXEC_BASE)?;

    // Limit the size of the result that we can return to userspace.
    if buffer_size > BOOTDATA_PLATFORM_EXTRA_BYTES {
        return Err(Status::INVALID_ARGS);
    }

    let mut buffer =
        kalloc::Box::<[u8]>::try_new_uninit_slice(buffer_size).map_err(|_| Status::NO_MEMORY)?;
    let mut zbi_size = 0usize;
    // SAFETY: We pass a valid allocated buffer of size `buffer_size` and a valid `zbi_size` out pointer.
    let status = unsafe {
        cpp_system_mexec_payload_get_helper(buffer.as_mut_ptr(), buffer_size, &mut zbi_size)
    };
    Status::ok(status)?;
    debug_assert!(zbi_size <= buffer_size);
    // SAFETY: cpp_system_mexec_payload_get_helper initializes buffer[..zbi_size] on success.
    let zbi_buffer = unsafe { core::slice::from_raw_parts(buffer.as_ptr() as *const u8, zbi_size) };
    user_buffer.copy_slice_to_user(zbi_buffer)?;
    Ok(())
}

#[syscall]
pub fn sys_system_mexec(
    resource: HandleValue,
    kernel_vmo: HandleValue,
    data_zbi_vmo: HandleValue,
) -> Result<(), Status> {
    if !BootOptions::get().enable_debugging_syscalls {
        return Err(Status::NOT_SUPPORTED);
    }
    validate_system_resource(resource, ZX_RSRC_SYSTEM_MEXEC_BASE)?;

    // SAFETY: Forwarding handles to inner C++ mexec coalescing & execution logic.
    let status = unsafe {
        cpp_system_mexec_core(
            resource.raw_value(),
            kernel_vmo.raw_value(),
            data_zbi_vmo.raw_value(),
        )
    };
    Status::ok(status)?;
    Ok(())
}

#[syscall]
pub fn sys_system_get_event(
    root_job: HandleValue,
    kind: u32,
    out: &mut HandleValue,
) -> Result<(), Status> {
    let rights = if kind == ZX_SYSTEM_EVENT_OUT_OF_MEMORY {
        ZX_RIGHT_MANAGE_PROCESS
    } else {
        // We check for the root job below. We should not need to enforce rights beyond that.
        ZX_RIGHT_NONE
    };
    let job = Dispatcher::get_with_rights::<JobDispatcher>(root_job, rights)?;

    // Validate that the job is in fact the first usermode job (aka root job).
    if !job.is_root() {
        return Err(Status::ACCESS_DENIED);
    }

    match kind {
        ZX_SYSTEM_EVENT_OUT_OF_MEMORY
        | ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY
        | ZX_SYSTEM_EVENT_MEMORY_PRESSURE_CRITICAL
        | ZX_SYSTEM_EVENT_MEMORY_PRESSURE_WARNING
        | ZX_SYSTEM_EVENT_MEMORY_PRESSURE_NORMAL => {
            let event = EventDispatcher::get_mem_pressure_event(kind);
            // Do not grant default event rights, as we don't want userspace to, for
            // example, be able to signal this event.
            *out = ProcessDispatcher::with_current(|up| {
                up.make_and_add_handle_from_ref(event, ZX_DEFAULT_SYSTEM_EVENT_LOW_MEMORY_RIGHTS)
            })?;
            Ok(())
        }
        _ => Err(Status::INVALID_ARGS),
    }
}

#[syscall]
pub fn sys_system_watch_memory_stall(
    resource: HandleValue,
    kind: u32,
    threshold: zx_duration_mono_t,
    window: zx_duration_mono_t,
    out: &mut HandleValue,
) -> Result<(), Status> {
    *out = ProcessDispatcher::with_current(|up| {
        up.enforce_basic_policy(ZX_POL_NEW_EVENT)?;

        if window > MEMORY_STALL_MAX_WINDOW || window <= 0 || threshold <= 0 || threshold > window {
            return Err(Status::INVALID_ARGS);
        }
        validate_system_resource(resource, ZX_RSRC_SYSTEM_STALL_BASE)?;

        let (handle, rights) = EventDispatcher::create_memory_stall(kind, threshold, window)?;
        up.make_and_add_handle(handle, rights)
    })?;
    Ok(())
}

#[syscall]
pub fn sys_system_set_performance_info(
    resource: HandleValue,
    topic: u32,
    info_void: UserInPtr<u8>,
    count: usize,
) -> Result<(), Status> {
    validate_system_resource(resource, ZX_RSRC_SYSTEM_CPU_BASE)?;

    // SAFETY: Getting processor count from percpu has no preconditions.
    let num_cpus = unsafe { cpp_percpu_processor_count() };
    if count == 0 || count > num_cpus {
        return Err(Status::OUT_OF_RANGE);
    }

    match topic {
        ZX_CPU_PERF_SCALE => {
            let mut uninit_info =
                kalloc::Box::<[zx_cpu_performance_info_t]>::try_new_uninit_slice(count)
                    .map_err(|_| Status::NO_MEMORY)?;
            let performance_info = info_void
                .reinterpret::<zx_cpu_performance_info_t>()
                .copy_slice_from_user(&mut uninit_info)?;

            let mut last_cpu = u32::MAX;
            for info in performance_info.iter() {
                let cpu = info.logical_cpu_number;
                if last_cpu != u32::MAX && cpu <= last_cpu {
                    return Err(Status::INVALID_ARGS);
                }
                last_cpu = cpu;
                if cpu as usize >= num_cpus
                    || (info.performance_scale.integer_part == 0
                        && info.performance_scale.fractional_part == 0)
                {
                    return Err(Status::OUT_OF_RANGE);
                }
            }
            // SAFETY: Passing valid slice pointer and count to C++ scheduler.
            unsafe {
                cpp_scheduler_update_processing_rates(performance_info.as_mut_ptr(), count);
            }
            Ok(())
        }
        ZX_CPU_PERF_LIMIT => {
            let mut uninit_info = kalloc::Box::<[zx_cpu_perf_limit_t]>::try_new_uninit_slice(count)
                .map_err(|_| Status::NO_MEMORY)?;
            let limit_info = info_void
                .reinterpret::<zx_cpu_perf_limit_t>()
                .copy_slice_from_user(&mut uninit_info)?;

            let mut last_cpu = u32::MAX;
            for entry in limit_info.iter() {
                let cpu = entry.logical_cpu_number;
                if last_cpu != u32::MAX && cpu <= last_cpu {
                    return Err(Status::INVALID_ARGS);
                }
                last_cpu = cpu;
                if cpu as usize >= num_cpus {
                    return Err(Status::OUT_OF_RANGE);
                }
                // TODO(eieio): Add support for the other limit types.
                if entry.limit_type != ZX_CPU_PERF_LIMIT_TYPE_RATE {
                    return Err(Status::NOT_SUPPORTED);
                }
            }
            // SAFETY: Passing valid slice pointer and count to C++ scheduler.
            unsafe {
                cpp_scheduler_update_processing_limits(limit_info.as_mut_ptr(), count);
            }
            Ok(())
        }
        _ => Err(Status::INVALID_ARGS),
    }
}

#[syscall]
pub fn sys_system_get_performance_info(
    resource: HandleValue,
    topic: u32,
    info_count: usize,
    info_void: UserOutPtr<u8>,
    output_count: UserOutPtr<usize>,
) -> Result<(), Status> {
    validate_system_resource(resource, ZX_RSRC_SYSTEM_CPU_BASE)?;

    // SAFETY: Getting processor count from percpu has no preconditions.
    let num_cpus = unsafe { cpp_percpu_processor_count() };
    if info_count != num_cpus {
        return Err(Status::OUT_OF_RANGE);
    }
    if output_count.is_null() {
        return Err(Status::INVALID_ARGS);
    }

    match topic {
        ZX_CPU_PERF_SCALE => {
            let mut info =
                kalloc::Box::<[zx_cpu_performance_info_t]>::try_new_zeroed_slice(info_count)
                    .map_err(|_| Status::NO_MEMORY)?;
            // SAFETY: Passing valid slice pointer and count.
            unsafe { cpp_scheduler_get_performance_scales(info.as_mut_ptr(), info_count) };
            info_void.reinterpret::<zx_cpu_performance_info_t>().copy_slice_to_user(&info)?;
            output_count.write(info_count)?;
            Ok(())
        }
        ZX_CPU_DEFAULT_PERF_SCALE => {
            let mut info =
                kalloc::Box::<[zx_cpu_performance_info_t]>::try_new_zeroed_slice(info_count)
                    .map_err(|_| Status::NO_MEMORY)?;
            // SAFETY: Passing valid slice pointer and count.
            unsafe { cpp_scheduler_get_default_performance_scales(info.as_mut_ptr(), info_count) };
            info_void.reinterpret::<zx_cpu_performance_info_t>().copy_slice_to_user(&info)?;
            output_count.write(info_count)?;
            Ok(())
        }
        ZX_CPU_PERF_LIMIT => {
            let mut info = kalloc::Box::<[zx_cpu_perf_limit_t]>::try_new_zeroed_slice(info_count)
                .map_err(|_| Status::NO_MEMORY)?;
            // SAFETY: Passing valid slice pointer and count.
            unsafe { cpp_scheduler_get_processing_limits(info.as_mut_ptr(), info_count) };
            info_void.reinterpret::<zx_cpu_perf_limit_t>().copy_slice_to_user(&info)?;
            output_count.write(info_count)?;
            Ok(())
        }
        _ => Err(Status::INVALID_ARGS),
    }
}

#[syscall]
pub fn sys_system_powerctl(
    power_rsrc: HandleValue,
    cmd: u32,
    raw_arg: UserInPtr<zx_system_powerctl_arg_t>,
) -> Result<(), Status> {
    validate_system_resource(power_rsrc, ZX_RSRC_SYSTEM_POWER_BASE)?;

    #[cfg(not(target_arch = "x86_64"))]
    let _ = raw_arg;

    match cmd {
        ZX_SYSTEM_POWERCTL_ENABLE_ALL_CPUS => {
            // SAFETY: Call C++ MP hotplug helper.
            let status = unsafe { cpp_mp_hotplug_cpu_mask_all() };
            Status::ok(status)?;
            Ok(())
        }
        ZX_SYSTEM_POWERCTL_DISABLE_ALL_CPUS_BUT_PRIMARY => {
            // SAFETY: Call C++ MP unplug helper.
            let status = unsafe { cpp_mp_unplug_cpu_mask_all_but_primary() };
            Status::ok(status)?;
            Ok(())
        }
        #[cfg(target_arch = "x86_64")]
        ZX_SYSTEM_POWERCTL_ACPI_TRANSITION_S_STATE => Err(Status::NOT_SUPPORTED),
        #[cfg(target_arch = "x86_64")]
        ZX_SYSTEM_POWERCTL_X86_SET_PKG_PL1 => {
            let arg = raw_arg.read()?;
            // SAFETY: Pass reference to valid arg.
            let status = unsafe { cpp_system_powerctl_x86_set_pkg_pl1(&arg) };
            Status::ok(status)?;
            Ok(())
        }
        ZX_SYSTEM_POWERCTL_REBOOT => {
            // SAFETY: Call C++ halt helper for reboot.
            unsafe { cpp_platform_graceful_halt_helper(HALT_ACTION_REBOOT) };
            Ok(())
        }
        ZX_SYSTEM_POWERCTL_ACK_KERNEL_INITIATED_REBOOT => {
            // SAFETY: Call C++ halt token ack helper.
            let status = unsafe { cpp_halt_token_ack_pending_halt() };
            Status::ok(status)?;
            Ok(())
        }
        ZX_SYSTEM_POWERCTL_REBOOT_BOOTLOADER => {
            // SAFETY: Call C++ halt helper for reboot bootloader.
            unsafe { cpp_platform_graceful_halt_helper(HALT_ACTION_REBOOT_BOOTLOADER) };
            Ok(())
        }
        ZX_SYSTEM_POWERCTL_REBOOT_RECOVERY => {
            // SAFETY: Call C++ halt helper for reboot recovery.
            unsafe { cpp_platform_graceful_halt_helper(HALT_ACTION_REBOOT_RECOVERY) };
            Ok(())
        }
        ZX_SYSTEM_POWERCTL_SHUTDOWN => {
            // SAFETY: Call C++ halt helper for shutdown.
            unsafe { cpp_platform_graceful_halt_helper(HALT_ACTION_SHUTDOWN) };
            Ok(())
        }
        _ => Err(Status::INVALID_ARGS),
    }
}

// TODO(https://fxbug.dev/42182544): Reconcile with HaltToken, zx_system_powerctl, and
// kernel-initiated-oom-reboot.
#[syscall]
pub fn sys_system_suspend_enter(
    resource: HandleValue,
    resume_deadline: zx_instant_boot_t,
    options: u64,
    out_header: UserOutPtr<zx_wake_source_report_header_t>,
    out_entries: UserOutPtr<zx_wake_source_report_entry_t>,
    num_entries: u32,
    actual_entries: UserOutPtr<u32>,
) -> Result<(), Status> {
    validate_system_resource(resource, ZX_RSRC_SYSTEM_CPU_BASE)?;

    // Make sure that any flags passed by the user are defined.
    if options & !VALID_SUSPEND_FLAGS != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // The event parameters need to be "consistent".  IOW - if someone wants
    // entries, they need to pass a buffer with at least enough room for 1 event,
    // and a pointer to a u32 to hold the number of entries returned.  If they
    // don't, then the pointers should be nullptr, and the num_entries should be
    // zero.
    let wants_entries = !out_entries.is_null();
    if (wants_entries != (num_entries > 0)) || (wants_entries != !actual_entries.is_null()) {
        return Err(Status::INVALID_ARGS);
    }

    // Additionally, if the user passes space for entries, they have to have passed
    // room for a report header as well.  It is OK to get a report header without
    // any entries, but not the other way around.
    let wants_report = !out_header.is_null();
    if wants_entries && !wants_report {
        return Err(Status::INVALID_ARGS);
    }

    // If the user has asked us to discard wake entries for wake vectors which are
    // currently ack'ed, do so now.
    if options & ZX_SYSTEM_SUSPEND_OPTION_DISCARD != 0 {
        // SAFETY: Call C++ discard wake event report helper.
        unsafe { cpp_wake_vector_discard_wake_event_report() };
    }

    // If the user has asked us to only generate a report (but not actually try to
    // suspend), make sure they (at least) passed us a buffer to hold a report
    // header, then generate the report and get out.
    if options & ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY != 0 {
        if !wants_report {
            return Err(Status::INVALID_ARGS);
        }
        // SAFETY: Pass user pointers to C++ report generator after validating pointer consistency.
        let status = unsafe {
            cpp_wake_vector_generate_wake_event_report(
                ZX_TIME_INFINITE,
                out_header.as_ptr(),
                out_entries.as_ptr(),
                num_entries,
                actual_entries.as_ptr(),
            )
        };
        Status::ok(status)?;
        return Ok(());
    }

    // Finally, attempt to drop into suspend.  Then, if the user asked for a wake
    // event report, generate one for them.
    // SAFETY: Call C++ transition to suspend helper.
    let suspend_start_time =
        unsafe { cpp_idle_power_thread_transition_all_active_to_suspend(resume_deadline) };

    if wants_report {
        // SAFETY: Pass user pointers to C++ report generator after validating pointer consistency.
        let status = unsafe {
            cpp_wake_vector_generate_wake_event_report(
                suspend_start_time,
                out_header.as_ptr(),
                out_entries.as_ptr(),
                num_entries,
                actual_entries.as_ptr(),
            )
        };
        Status::ok(status)?;
    }

    Ok(())
}
