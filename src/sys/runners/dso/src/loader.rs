// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{ControlHandle, ServerEnd};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_ldsvc as fldsvc;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use log::{error, warn};
use std::ffi::{CStr, c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::LazyLock;
use std::{mem, thread};

unsafe extern "C" {
    /// SAFETY: documented at sdk/lib/c/include/zircon/dlfcn.h
    fn dl_set_loader_service(new_svc: zx::sys::zx_handle_t) -> zx::sys::zx_handle_t;

    /// SAFETY: documented at sdk/lib/c/include/zircon/dlfcn.h
    fn dlopen_vmo_vmar(
        vmo: zx::sys::zx_handle_t,
        vmar: zx::sys::zx_handle_t,
        flag: c_int,
    ) -> *mut c_void;
}

#[derive(Debug)]
pub(super) struct Library {
    pub(super) ptr: NonNull<c_void>,
}

/// SAFETY: This pointer is an internal detail of the libc and libc guarantees it's safe to send
/// across threads. Eg, it's not necessary to close it on the same thread it was dlopen'd.
unsafe impl Send for Library {}

impl Library {
    pub(super) fn try_load(dso_name: &str, vmo: zx::Vmo) -> Result<Self, zx::Status> {
        let root_vmar = fuchsia_runtime::vmar_root_self();
        assert!(!vmo.as_handle_ref().is_invalid(), "try_load: vmo invalid");
        assert!(!root_vmar.as_handle_ref().is_invalid(), "try_load: root vmar invalid");
        // SAFETY: Asserted that inputs are valid. `dlopen_vmo_vmar` does not take ownership of
        // `vmo` or `root_vmar` so we don't surrender ownership.
        let library =
            unsafe { dlopen_vmo_vmar(vmo.raw_handle(), root_vmar.raw_handle(), libc::RTLD_NOW) };
        if library.is_null() {
            // SAFETY: All `dlerror` is safe to call and we don't do anything unsafe with the
            // return value.
            let err_p = unsafe { libc::dlerror() };
            // SAFETY: `dlerror` guarantees `err_p` is a valid C-string
            let err = unsafe { std::ffi::CStr::from_ptr(err_p) };
            if !err_p.is_null() {
                error!(dso_name:%; "Failed to dlopen DSO: {err:?}");
            } else {
                error!(dso_name:%; "Failed to dlopen DSO: UNKNOWN");
            }
        }

        Ok(Self { ptr: NonNull::new(library).ok_or(zx::Status::INTERNAL)? })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: This pointer is always obtained by `dlopen_vmo_vmar`. `dlclose` is only called
        // once on the pointer because [`Drop::drop`] is called only once.
        unsafe {
            _ = libc::dlclose(self.ptr.as_ptr());
        };
    }
}

#[derive(Debug)]
pub(super) struct Loader {
    lib_dir: fio::DirectoryProxy,
    scope: fasync::Scope,
}

static LOADER_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

impl Loader {
    pub(super) fn install(
        dso_name: &str,
        dso_vmo: zx::Vmo,
        lib_dir: fio::DirectoryProxy,
    ) -> Result<Library, zx::Status> {
        // This lock isn't technically necessary _yet_. Since this function is not async and the
        // executor is single threaded. However, either of these could change in the future, and
        // we'd like to avoid a sneaky and difficult to diagnose bug down the road.
        let _guard = LOADER_LOCK.lock();

        let (ld_client, ld_server) = fidl::endpoints::create_endpoints::<fldsvc::LoaderMarker>();
        // We are the only thing that is able to mess with the loader service in
        // dso_runner. This is not an async function and the executor is single-threaded so no
        // concurrency is possible within this function. We always re-install the original one, so
        // there is always a valid one currently installed.
        //
        // SAFETY: `ld_client` is the peer of a valid loader serve hosted in `loader_thread` below.
        let old_loader: zx::Channel = unsafe {
            zx::NullableHandle::from_raw(dl_set_loader_service(ld_client.into_channel().into_raw()))
        }
        .into();
        // The loader service needs to run on a separate thread because the `dlopen` operation will
        // make a sync call back into the loader service we just installed.
        let loader_thread = thread::spawn(move || {
            let mut executor = fasync::LocalExecutor::default();
            executor.run_singlethreaded(async move {
                let loader = Rc::new(Self { lib_dir, scope: fasync::Scope::new() });
                _ = loader.run_loader(ld_server).await;
            });
        });

        let dso_name = dso_name.to_string();
        let library = Library::try_load(&dso_name, dso_vmo);

        // SAFETY: `old_loader` is a valid loader service handle because it is the original channel
        // returned by `dl_set_loader_service` above.
        //
        // The returned handle is safe to pass to [`zx::NullableHandle::from_raw`] because it
        // was obtained from `ld_client.into_raw` above.
        _ = unsafe { zx::NullableHandle::from_raw(dl_set_loader_service(old_loader.into_raw())) };
        // The loader thread is no longer needed, ensure it exits.
        loader_thread.join().unwrap();

        Ok(library?)
    }

    /// This is _mostly_ identical to the standard implementation in crate `library_loader`,
    /// with a couple small differences:
    ///
    /// - Tasks are spawned in `scope` rather than with [`Task::detach`].
    /// - This future is run directly in the executor rather than spawning a task.
    ///
    /// In the future there may be more differences, such as supporting transparent remappings
    /// of libraries, but these existing differences were enough to fork the default
    /// implementation. Overall it's not much code.
    fn run_loader(
        self: &Rc<Self>,
        request: ServerEnd<fldsvc::LoaderMarker>,
    ) -> impl Future<Output = Result<(), anyhow::Error>> + '_ + use<'_> {
        let this = Rc::downgrade(self);
        let lib_dirs = vec![Clone::clone(&self.lib_dir)];
        async move {
            let mut search_dirs = lib_dirs.clone();
            let mut stream = request.into_stream();
            while let Some(req) = stream.try_next().await? {
                let this = this.upgrade().unwrap();
                match req {
                    fldsvc::LoaderRequest::Done { control_handle } => {
                        control_handle.shutdown();
                    }
                    fldsvc::LoaderRequest::LoadObject { object_name, responder } => {
                        match Self::load_object(&search_dirs, &object_name).await {
                            Ok(vmo) => {
                                responder.send(zx::sys::ZX_OK, Some(vmo))?;
                            }
                            Err(err) => {
                                warn!(err:?; "loader failed to load object");
                                responder.send(zx::sys::ZX_ERR_NOT_FOUND, None)?;
                            }
                        }
                    }
                    fldsvc::LoaderRequest::Config { config, responder } => {
                        match library_loader::parse_config_string(&lib_dirs, &config) {
                            Ok(new_search_path) => {
                                search_dirs = new_search_path;
                                responder.send(zx::sys::ZX_OK)?;
                            }
                            Err(err) => {
                                warn!(err:%; "loader failed to parse config");
                                responder.send(zx::sys::ZX_ERR_INVALID_ARGS)?;
                            }
                        }
                    }
                    fldsvc::LoaderRequest::Clone { loader, responder } => {
                        self.scope.spawn_local(async move {
                            _ = this.run_loader(loader);
                        });
                        responder.send(zx::sys::ZX_OK)?;
                    }
                };
            }
            Ok(())
        }
        .inspect_err(|err| {
            warn!(err:%; "failed to serve loader service");
        })
    }

    async fn load_object(
        search_dirs: &Vec<fio::DirectoryProxy>,
        object_name: &str,
    ) -> Result<zx::Vmo, Vec<anyhow::Error>> {
        let mut errors = vec![];
        for dir_proxy in search_dirs {
            match library_loader::load_vmo(dir_proxy, &object_name).await {
                Ok(b) => {
                    return Ok(b);
                }
                Err(e) => errors.push(e),
            }
        }
        Err(errors.into())
    }
}

#[repr(C)]
pub(super) struct dso_sync_input {
    handle_count: u32,
    handle: *mut zx::sys::zx_handle_t,
    handle_info: *mut u32,
    name_count: u32,
    names: *mut *const ::libc::c_char,
    argc: ::libc::c_int,
    argv: *mut *const ::libc::c_char,
    envp: *mut *const ::libc::c_char,
}

#[repr(C)]
pub(super) struct dso_async_input {
    handle_count: u32,
    handle: *mut zx::sys::zx_handle_t,
    handle_info: *mut u32,
    name_count: u32,
    names: *mut *const ::libc::c_char,
    argc: ::libc::c_int,
    argv: *mut *const ::libc::c_char,
    envp: *mut *const ::libc::c_char,
    dispatcher: *mut fdf_sys::fdf_dispatcher_t,
}

pub(super) type SyncEntryPoint = unsafe extern "C" fn(input: dso_sync_input) -> ::libc::c_int;

pub(super) type AsyncEntryPoint = unsafe extern "C" fn(input: dso_async_input) -> ::libc::c_int;

#[derive(Debug)]
pub(super) enum Hooks {
    Sync(SyncEntryPoint),
    Async(AsyncEntryPoint),
}

/// SAFETY: These hooks are just static pointers to code and therefore have no thread local state.
unsafe impl Send for Hooks {}
/// SAFETY: These hooks are valid on every thread after they are loaded.
unsafe impl Sync for Hooks {}

#[derive(Debug, Clone, Copy)]
pub(super) struct CArray<T>(*mut T, usize);

impl<T: Sized> CArray<T> {
    pub(super) fn new(arr: &mut [T]) -> Self {
        Self(arr.as_mut_ptr(), arr.len())
    }
}

// SAFETY: [`CArray`]s are never referenced past the point [`ProgramResources`] is released, after
// the async component's dispatcher is shutdown.
unsafe impl<T> Send for CArray<T> {}

impl Hooks {
    /// Returns error if the expected symbol was not found, with the name of the symbol.
    pub(super) fn new_from_library(
        library: &Library,
        is_async: bool,
    ) -> Result<Hooks, &'static CStr> {
        const SYM_SYNC: &CStr = c"_dso_start";
        const SYM_ASYNC: &CStr = c"_dso_start_async";

        // SAFETY: The symbol is valid as long as the shared library is not closed. So its
        // lifetime must track that of `library` from above. We also do a null check to ensure
        // it's a valid pointer.
        if is_async {
            // SAFETY: The symbol is valid as long as the shared library is not closed, which we
            // know to be true because the `library` reference is keeping the library alive.
            let ptr = unsafe { libc::dlsym(library.ptr.as_ptr(), SYM_ASYNC.as_ptr()) };
            if ptr.is_null() {
                return Err(SYM_ASYNC);
            }

            // SAFETY: There is no way for us to verify from here that `ptr` has the type
            // signature of [`AsyncEntryPoint`], but it should be as long as the binary was
            // built with the standard DSO library.
            let entry = unsafe { mem::transmute(ptr) };
            Ok(Self::Async(entry))
        } else {
            // SAFETY: The symbol is valid as long as the shared library is not closed, which we
            // know to be true because the `library` reference is keeping the library alive.
            let ptr = unsafe { libc::dlsym(library.ptr.as_ptr(), SYM_SYNC.as_ptr()) };
            if ptr.is_null() {
                return Err(SYM_SYNC);
            }

            // SAFETY: There is no way for us to verify from here that `ptr` has the type
            // signature of [`AsyncEntryPoint`], but it should be as long as the binary was
            // built with the standard DSO library.
            let entry = unsafe { mem::transmute(ptr) };
            Ok(Self::Sync(entry))
        }
    }

    /// Invokes the _dso_start_async entrypoint in an async DSO.
    ///
    /// # Safety
    ///
    /// `self` must be [`Hooks::Async`]. The contents of `handle`, `handle_info`, `names`, `argv`,
    /// and `envp` must be kept alive until `dispatcher` is shutdown.
    pub(super) unsafe fn dso_start_async(
        &self,
        handle: CArray<zx::sys::zx_handle_t>,
        handle_info: CArray<u32>,
        names: CArray<*const ::libc::c_char>,
        argv: CArray<*const ::libc::c_char>,
        envp: CArray<*const ::libc::c_char>,
        dispatcher: fdf::AutoReleaseDispatcher,
    ) -> ::core::ffi::c_int {
        match self {
            Self::Sync(_) => unreachable!(),
            Self::Async(e) => unsafe {
                (e)(dso_async_input {
                    handle: handle.0,
                    handle_count: handle.1 as u32,
                    handle_info: handle_info.0,
                    name_count: names.1 as u32,
                    names: names.0,
                    argv: argv.0,
                    argc: argv.1 as i32,
                    envp: envp.0,
                    dispatcher: dispatcher.as_dispatcher_ref().as_raw(),
                })
            },
        }
    }

    /// Invokes the _dso_start_async entrypoint in a sync DSO.
    ///
    /// # Safety
    ///
    /// `self` must be [`Hooks::Sync`]. The contents of `handle`, `handle_info`, `names`, `argv`,
    /// and `envp` must be kept alive until the function returns.
    pub(super) unsafe fn dso_start(
        &self,
        handle: &mut [zx::sys::zx_handle_t],
        handle_info: &mut [u32],
        names: &mut [*const ::libc::c_char],
        argv: &mut [*const ::libc::c_char],
        envp: &mut [*const ::libc::c_char],
    ) -> ::core::ffi::c_int {
        let handle_count = handle.len() as u32;
        let name_count = names.len() as u32;
        let argc = argv.len() as ::libc::c_int;
        match self {
            Self::Async(_) => unreachable!(),
            Self::Sync(e) => unsafe {
                (e)(dso_sync_input {
                    handle_count,
                    handle: handle.as_mut_ptr(),
                    handle_info: handle_info.as_mut_ptr(),
                    name_count,
                    names: names.as_mut_ptr(),
                    argc,
                    argv: argv.as_mut_ptr(),
                    envp: envp.as_mut_ptr(),
                })
            },
        }
    }
}
