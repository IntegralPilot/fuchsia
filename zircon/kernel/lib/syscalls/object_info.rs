// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    BusTransactionInitiatorDispatcher, Dispatcher, HandleValue, JobDispatcher, MsiDispatcher,
    SocketDispatcher, ThreadDispatcher, TimerDispatcher, VmAddressRegionDispatcher,
    VmObjectDispatcher,
};
use crate::user_copy::UserOutPtr;
use debug::ltracef;
use syscalls_macro::syscall;
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx_status::Status;
use zx_types::*;

const LOCAL_TRACE: u32 = 0;

// Specialize the conversion to work for any version that is a subset of zx_info_vmo_t.
// Being a subset the full zx_info_vmo_t can just be copied over.
fn convert_vmo_info_v1(vmo: &zx_info_vmo_t) -> zx_info_vmo_v1_t {
    zx_info_vmo_v1_t {
        koid: vmo.koid,
        name: vmo.name,
        size_bytes: vmo.size_bytes,
        parent_koid: vmo.parent_koid,
        num_children: vmo.num_children,
        num_mappings: vmo.num_mappings,
        share_count: vmo.share_count,
        flags: vmo.flags,
        committed_bytes: vmo.committed_bytes,
        handle_rights: vmo.handle_rights,
        cache_policy: vmo.cache_policy,
        ..Default::default()
    }
}

fn convert_vmo_info_v2(vmo: &zx_info_vmo_t) -> zx_info_vmo_v2_t {
    zx_info_vmo_v2_t {
        koid: vmo.koid,
        name: vmo.name,
        size_bytes: vmo.size_bytes,
        parent_koid: vmo.parent_koid,
        num_children: vmo.num_children,
        num_mappings: vmo.num_mappings,
        share_count: vmo.share_count,
        flags: vmo.flags,
        committed_bytes: vmo.committed_bytes,
        handle_rights: vmo.handle_rights,
        cache_policy: vmo.cache_policy,
        metadata_bytes: vmo.metadata_bytes,
        committed_change_events: vmo.committed_change_events,
        ..Default::default()
    }
}

fn convert_vmo_info_v3(vmo: &zx_info_vmo_t) -> zx_info_vmo_v3_t {
    zx_info_vmo_v3_t {
        koid: vmo.koid,
        name: vmo.name,
        size_bytes: vmo.size_bytes,
        parent_koid: vmo.parent_koid,
        num_children: vmo.num_children,
        num_mappings: vmo.num_mappings,
        share_count: vmo.share_count,
        flags: vmo.flags,
        committed_bytes: vmo.committed_bytes,
        handle_rights: vmo.handle_rights,
        cache_policy: vmo.cache_policy,
        metadata_bytes: vmo.metadata_bytes,
        committed_change_events: vmo.committed_change_events,
        populated_bytes: vmo.populated_bytes,
        ..Default::default()
    }
}

// Current second version is an extension of v1; simply copy over the
// earlier header and context.arch fields.
fn convert_thread_exception_report_v1(info: &zx_exception_report_t) -> zx_exception_report_v1_t {
    // SAFETY: `info` is a valid `zx_exception_report_t` (40 bytes). The 32-byte prefix
    // corresponds to `zx_exception_report_v1_t` (8-byte header + 24-byte arch exception data).
    let src_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(
            (info as *const zx_exception_report_t).cast::<u8>(),
            core::mem::size_of::<zx_exception_report_v1_t>(),
        )
    };
    let (v1, _) = zx_exception_report_v1_t::read_from_prefix(src_bytes)
        .expect("slice length matches size_of::<zx_exception_report_v1_t>()");
    v1
}

// Copies to usermode the actual (number of records written) and the avail (number of records
// available).
fn actual_avail_result(
    actual: usize,
    avail: usize,
    user_actual: UserOutPtr<usize>,
    user_avail: UserOutPtr<usize>,
) -> Result<(), Status> {
    if !user_actual.is_null() {
        user_actual.write(actual)?;
    }
    if !user_avail.is_null() {
        user_avail.write(avail)?;
    }
    Ok(())
}

fn single_record_bytes(
    dst_buffer: UserOutPtr<u8>,
    dst_buffer_size: usize,
    user_actual: UserOutPtr<usize>,
    user_avail: UserOutPtr<usize>,
    src_bytes: &[u8],
) -> Result<(), Status> {
    let actual = if dst_buffer_size >= src_bytes.len() {
        dst_buffer.copy_slice_to_user(src_bytes)?;
        1
    } else {
        0
    };
    actual_avail_result(actual, 1, user_actual, user_avail)?;
    if actual == 0 {
        return Err(Status::BUFFER_TOO_SMALL);
    }
    Ok(())
}

// Copies a single record, |src_record|, into the user buffer |dst_buffer| of size
// |dst_buffer_size|. If the copy succeeds, the value 1 is copied into |user_avail|.
fn single_record_result<T: IntoBytes + Immutable>(
    dst_buffer: UserOutPtr<u8>,
    dst_buffer_size: usize,
    user_actual: UserOutPtr<usize>,
    user_avail: UserOutPtr<usize>,
    src_record: &T,
) -> Result<(), Status> {
    let record_size = core::mem::size_of::<T>();
    let actual = if dst_buffer_size >= record_size {
        dst_buffer.reinterpret::<T>().copy_to_user(src_record)?;
        1
    } else {
        0
    };
    actual_avail_result(actual, 1, user_actual, user_avail)?;
    if actual == 0 {
        return Err(Status::BUFFER_TOO_SMALL);
    }
    Ok(())
}

unsafe extern "C" {
    fn cpp_object_get_info_cpp_types(
        handle: zx_handle_t,
        topic: u32,
        buffer: *mut core::ffi::c_void,
        buffer_size: usize,
        actual: *mut usize,
        avail: *mut usize,
    ) -> zx_status_t;
}

// actual is an optional return parameter for the number of records returned
// avail is an optional return parameter for the number of records available
//
// Topics which return a fixed number of records will return ZX_ERR_BUFFER_TOO_SMALL
// if there is not enough buffer space provided.
// This allows for zx_object_get_info(handle, topic, &info, sizeof(info), NULL, NULL)
#[syscall]
pub fn sys_object_get_info(
    handle: HandleValue,
    topic: u32,
    buffer: UserOutPtr<u8>,
    buffer_size: usize,
    actual: UserOutPtr<usize>,
    avail: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {:?} topic {}\n", handle, topic);

    /// Helper macro for single-record topic queries that obtain a typed dispatcher with
    /// `ZX_RIGHT_INSPECT` (or rights) and copy the single record into user buffers using
    /// `single_record_result`.
    macro_rules! single_record_info {
        // Infallible method call on a dispatcher with ZX_RIGHT_INSPECT.
        ($dispatcher_type:ty, $method:ident) => {{
            let dispatcher =
                Dispatcher::get_with_rights::<$dispatcher_type>(handle, ZX_RIGHT_INSPECT)?;
            let record = dispatcher.$method();
            single_record_result(buffer, buffer_size, actual, avail, &record)?;
            Ok(())
        }};
        // Fallible method call on a dispatcher with ZX_RIGHT_INSPECT.
        ($dispatcher_type:ty, $method:ident?) => {{
            let dispatcher =
                Dispatcher::get_with_rights::<$dispatcher_type>(handle, ZX_RIGHT_INSPECT)?;
            let record = dispatcher.$method()?;
            single_record_result(buffer, buffer_size, actual, avail, &record)?;
            Ok(())
        }};
        // Custom closure/expression receiving the dispatcher with ZX_RIGHT_INSPECT.
        ($dispatcher_type:ty, |$disp:ident| $body:expr) => {{
            let $disp = Dispatcher::get_with_rights::<$dispatcher_type>(handle, ZX_RIGHT_INSPECT)?;
            let record = $body;
            single_record_result(buffer, buffer_size, actual, avail, &record)?;
            Ok(())
        }};
        // Closure/expression with handle rights lookup.
        (with_rights: |$disp:ident, $rights:ident| $body:expr) => {{
            let ($disp, $rights) = Dispatcher::get_dispatcher_and_rights(handle)?;
            let record = $body;
            single_record_result(buffer, buffer_size, actual, avail, &record)?;
            Ok(())
        }};
    }

    match topic {
        ZX_INFO_HANDLE_BASIC => {
            single_record_info!(with_rights: |dispatcher, rights| {
                dispatcher.get_handle_info(rights)
            })
        }
        ZX_INFO_JOB_CHILDREN | ZX_INFO_JOB_PROCESSES => {
            let job = Dispatcher::get_with_rights::<JobDispatcher>(handle, ZX_RIGHT_ENUMERATE)?;
            // Don't recurse; we only want the job's direct children.
            let max =
                if buffer_size == 0 { 0 } else { buffer_size / core::mem::size_of::<zx_koid_t>() };
            let is_jobs = topic == ZX_INFO_JOB_CHILDREN;
            let (count, avail_count) =
                job.enumerate_children(buffer.reinterpret(), max, is_jobs)?;
            actual_avail_result(count, avail_count, actual, avail)?;
            Ok(())
        }
        ZX_INFO_THREAD => single_record_info!(ThreadDispatcher, get_info_for_userspace),
        ZX_INFO_THREAD_EXCEPTION_REPORT_V1 => {
            single_record_info!(ThreadDispatcher, |thread| {
                let report = thread.get_exception_report()?;
                convert_thread_exception_report_v1(&report)
            })
        }
        ZX_INFO_THREAD_EXCEPTION_REPORT => {
            let thread = Dispatcher::get_with_rights::<ThreadDispatcher>(handle, ZX_RIGHT_INSPECT)?;
            let report = thread.get_exception_report()?;
            // SAFETY: `report` is a valid, fully initialized 40-byte `zx_exception_report_t` struct.
            let src_bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(
                    (&report as *const zx_exception_report_t).cast::<u8>(),
                    core::mem::size_of::<zx_exception_report_t>(),
                )
            };
            single_record_bytes(buffer, buffer_size, actual, avail, src_bytes)?;
            Ok(())
        }
        ZX_INFO_THREAD_STATS => single_record_info!(ThreadDispatcher, get_stats_for_userspace?),
        ZX_INFO_TASK_RUNTIME | ZX_INFO_TASK_RUNTIME_V1 => {
            let dispatcher = Dispatcher::get_with_rights::<Dispatcher>(handle, ZX_RIGHT_INSPECT)?;
            let runtime: zx_info_task_runtime_t =
                if let Some(job) = dispatcher.downcast::<JobDispatcher>() {
                    job.get_runtime_stats()
                } else if let Some(thread) = dispatcher.downcast::<ThreadDispatcher>() {
                    thread.get_runtime_stats()
                } else {
                    // SAFETY: Call C++ FFI helper for topics handled by C++ dispatcher instances (e.g. ProcessDispatcher).
                    let status = unsafe {
                        cpp_object_get_info_cpp_types(
                            handle.raw_value(),
                            topic,
                            buffer.as_ptr() as *mut core::ffi::c_void,
                            buffer_size,
                            actual.as_ptr(),
                            avail.as_ptr(),
                        )
                    };
                    return Status::ok(status);
                };
            if topic == ZX_INFO_TASK_RUNTIME_V1 {
                let v1 = zx_info_task_runtime_v1_t {
                    cpu_time: runtime.cpu_time,
                    queue_time: runtime.queue_time,
                };
                single_record_result(buffer, buffer_size, actual, avail, &v1)?;
            } else {
                single_record_result(buffer, buffer_size, actual, avail, &runtime)?;
            }
            Ok(())
        }
        ZX_INFO_VMO | ZX_INFO_VMO_V1 | ZX_INFO_VMO_V2 | ZX_INFO_VMO_V3 => {
            let (dispatcher, rights) = Dispatcher::get_dispatcher_and_rights(handle)?;
            let vmo = dispatcher.downcast::<VmObjectDispatcher>().ok_or(Status::WRONG_TYPE)?;
            let info = vmo.get_vmo_info(rights);
            match topic {
                ZX_INFO_VMO_V1 => {
                    let v1 = convert_vmo_info_v1(&info);
                    single_record_result(buffer, buffer_size, actual, avail, &v1)?;
                }
                ZX_INFO_VMO_V2 => {
                    let v2 = convert_vmo_info_v2(&info);
                    single_record_result(buffer, buffer_size, actual, avail, &v2)?;
                }
                ZX_INFO_VMO_V3 => {
                    let v3 = convert_vmo_info_v3(&info);
                    single_record_result(buffer, buffer_size, actual, avail, &v3)?;
                }
                _ => {
                    single_record_result(buffer, buffer_size, actual, avail, &info)?;
                }
            }
            Ok(())
        }
        ZX_INFO_VMAR => single_record_info!(VmAddressRegionDispatcher, get_vmar_info),
        ZX_INFO_HANDLE_COUNT => {
            single_record_info!(Dispatcher, |dispatcher| {
                zx_info_handle_count_t { handle_count: dispatcher.current_handle_count() }
            })
        }
        ZX_INFO_SOCKET => single_record_info!(SocketDispatcher, get_info),
        ZX_INFO_JOB => single_record_info!(JobDispatcher, get_info),
        ZX_INFO_TIMER => single_record_info!(TimerDispatcher, get_info),
        ZX_INFO_MSI => single_record_info!(MsiDispatcher, get_info),
        ZX_INFO_BTI => single_record_info!(BusTransactionInitiatorDispatcher, get_info),
        // C++ only topics (and VM map/vmo enumeration, system resources, process info, etc.)
        _ => {
            // SAFETY: Call C++ FFI helper for topics handled by C++ dispatcher instances or subsystems.
            let status = unsafe {
                cpp_object_get_info_cpp_types(
                    handle.raw_value(),
                    topic,
                    buffer.as_ptr() as *mut core::ffi::c_void,
                    buffer_size,
                    actual.as_ptr(),
                    avail.as_ptr(),
                )
            };
            Status::ok(status)
        }
    }
}
