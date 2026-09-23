// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf::AutoReleaseDispatcher;
pub use fuchsia_dso_macro::main;

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_process_lifecycle::LifecycleMarker;

/// In a DSO component, perform setup necessary before calling `dso_main`.
#[doc(hidden)]
pub fn dso_init(
    _handle_count: u32,
    _handle: *mut ::zx::sys::zx_handle_t,
    _handle_info: *mut u32,
    _name_count: u32,
    _names: *mut *const ::std::ffi::c_char,
    _argc: ::std::ffi::c_int,
    _argv: *mut *const ::std::ffi::c_char,
    _envp: *mut *const ::std::ffi::c_char,
) {
    // TODO(https://fxbug.dev/403545512): implement this for sync support
    //unsafe {
    //fdio::__libc_extensions_init(handle_count, handle, handle_info, name_count, names);
    //fdio::fdio_startup_handles_init_tls(handle_count, handle, handle_info);
    //}
    //fuchsia_dso::store_args(argc, argv, envp);
}

/// Arguments to an async DSO main()
#[derive(Debug)]
pub struct DsoAsyncArgs {
    /// The component's incoming namespace.
    pub incoming: Vec<cm_types::NamespaceEntry>,
    /// The component's outgoing directory. This is an [`Option`] but should typically be present.
    pub outgoing_dir: Option<fidl::endpoints::ServerEnd<::fidl_fuchsia_io::DirectoryMarker>>,
    /// A reference to the dispatcher used to dispatch the component's tasks.
    pub dispatcher: AutoReleaseDispatcher,
    /// The component's lifecycle server handle. It can close this channel to signal component
    /// exit.
    pub lifecycle: ServerEnd<LifecycleMarker>,
    /// The component's structured config VMO, if it has one.
    pub config: Option<zx::Vmo>,
}

/// Input type for [`dso_init_async`] that is `Send`.
///
/// # Safety
///
/// This type implements `Send`. It is the caller's responsibility not to cause race conditions
/// with the pointers if they use `Send`.
#[doc(hidden)]
pub struct DsoStartAsyncPayload {
    pub handle_count: u32,
    pub handle: *mut ::zx::sys::zx_handle_t,
    pub handle_info: *mut u32,
    pub name_count: u32,
    pub names: *mut *const ::std::ffi::c_char,
    pub argc: ::std::ffi::c_int,
    pub argv: *mut *const ::std::ffi::c_char,
    pub envp: *mut *const ::std::ffi::c_char,
    pub dispatcher: *mut ::std::ffi::c_void,
}

unsafe impl Send for DsoStartAsyncPayload {}

/// In a DSO async component, perform setup necessary before calling `dso_main_async`.
#[doc(hidden)]
pub fn dso_init_async(payload: DsoStartAsyncPayload) -> DsoAsyncArgs {
    use std::{ffi, ptr, slice};
    let DsoStartAsyncPayload {
        handle_count,
        handle,
        handle_info,
        name_count,
        names,
        argc: _, // ignore for now
        argv: _, // ignore for now
        envp: _, // ignore for now
        dispatcher,
    } = payload;

    // SAFETY: dso_runner which provides `handle` guarantees `handle_count` is in bounds.
    let handle = unsafe { slice::from_raw_parts(handle, handle_count as usize) };
    // SAFETY: dso_runner which provides `handle_info` guarantees `handle_count` is in bounds.
    let handle_info = unsafe { slice::from_raw_parts(handle_info, handle_count as usize) };
    // SAFETY: dso_runner which provides `names` guarantees `name_count` is in bounds.
    let names = unsafe { slice::from_raw_parts(names, name_count as usize) };
    let dispatcher = dispatcher as *mut fdf_sys::fdf_dispatcher_t;
    // TODO(https://fxbug.dev/488394483): This is a test API but it's currently the simplest way to
    // set the driver dispatcher on the fuchsia-async executor thread. This should be replaced
    // when there's a better way to override the dispatcher.
    // SAFETY: dso_runner guarantees `dispatcher` is a valid dispatcher.
    assert_eq!(
        unsafe { fdf_sys::fdf_testing_set_default_dispatcher(dispatcher) },
        zx::sys::ZX_OK,
        "fdf_testing_set_default_dispatcher"
    );
    // SAFETY: dso_runner guarantees `dispatcher` is an `fdf_dispatcher_t` so this is a valid cast.
    let dispatcher = unsafe {
        fdf::AutoReleaseDispatcher::from_raw(
            ptr::NonNull::new(dispatcher).expect("null dispatcher"),
        )
    };
    struct HandleInfo {
        handle: zx::NullableHandle,
        id: fuchsia_runtime::HandleInfo,
    }
    let handle_infos = handle.iter().zip(handle_info.iter()).filter_map(|(handle, info)| {
        Some(HandleInfo {
            id: fuchsia_runtime::HandleInfo::try_from(info.clone()).ok()?,
            // SAFETY: dso_runner guarantees all handles in `handle_info` were valid.
            handle: unsafe { zx::NullableHandle::from_raw(*handle) },
        })
    });

    let mut incoming = vec![];
    let mut outgoing_dir = None;
    let mut lifecycle = None;
    let mut config = None;
    for handle_info in handle_infos {
        let HandleInfo { id, handle } = handle_info;
        match id.handle_type() {
            fuchsia_runtime::HandleType::FileDescriptor => {
                // TODO(https://fxbug.dev/403545512): what should we do with these?
            }
            fuchsia_runtime::HandleType::NamespaceDirectory => {
                let arg = id.arg() as usize;
                if arg >= names.len() {
                    continue;
                }
                // SAFETY: dso_runner guarantees all names were valid C strings. We just checked
                // that `arg` is in bounds.
                let Ok(path) = unsafe { ffi::CStr::from_ptr(names[arg]) }.to_str() else {
                    continue;
                };
                let Ok(path) = cm_types::NamespacePath::new(path) else {
                    continue;
                };
                let directory = fidl::Channel::from(handle);
                incoming.push(cm_types::NamespaceEntry { path, directory: directory.into() });
            }
            fuchsia_runtime::HandleType::DirectoryRequest => {
                let directory = fidl::Channel::from(handle);
                outgoing_dir = Some(directory.into());
            }
            fuchsia_runtime::HandleType::Lifecycle => {
                let ch = fidl::Channel::from(handle);
                lifecycle = Some(ServerEnd::<LifecycleMarker>::from(ch));
            }
            fuchsia_runtime::HandleType::ComponentConfigVmo => {
                let vmo = zx::Vmo::from(handle);
                config = Some(vmo);
            }
            _ => {}
        }
    }

    let lifecycle = lifecycle.expect("libfuchsia: expected lifecycle handle missing");
    DsoAsyncArgs { incoming, outgoing_dir, dispatcher, lifecycle, config }
}

/// In a DSO component, perform cleanup necessary after calling `dso_main`.
#[doc(hidden)]
pub fn dso_fini() {
    // TODO(https://fxbug.dev/403545512): implement this for sync support
    //unsafe {
    //fdio::__libc_extensions_fini();
    //}
}

#[doc(hidden)]
pub fn adapt_to_pass_arguments<A, R>(f: impl FnOnce(A) -> R, args: A) -> impl FnOnce() -> R {
    move || f(args)
}
