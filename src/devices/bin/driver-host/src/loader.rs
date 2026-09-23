// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{ControlHandle, ServerEnd, SynchronousProxy};
use fidl_fuchsia_ldsvc as fldsvc;
use futures::{TryFutureExt, TryStreamExt};
use std::collections::BTreeMap;
use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::LazyLock;
use zx::Status;

unsafe extern "C" {
    fn dl_set_loader_service(new_svc: zx::sys::zx_handle_t) -> zx::sys::zx_handle_t;
    fn dlopen_vmo_vmar(
        vmo: zx::sys::zx_handle_t,
        vmar: zx::sys::zx_handle_t,
        flag: c_int,
    ) -> *mut c_void;
}

#[derive(Debug)]
pub(crate) struct Library {
    pub(crate) ptr: NonNull<c_void>,
}
/// SAFETY: This pointer is an internal detail of the libc and libc guarantees it's safe to send
/// across threads. Eg, it's not necessary to close it on the same thread it was dlopen'd.
unsafe impl Send for Library {}

impl Library {
    fn try_load(vmo: zx::Vmo, vmar: &Option<zx::Vmar>) -> Result<Self, Status> {
        // SAFETY: By checking it's not null and placing in the Library wrapper, we ensure it's a
        // valid pointer and we will close it after it's no longer used.
        let library = unsafe {
            dlopen_vmo_vmar(
                vmo.into_raw(),
                vmar.as_ref()
                    .map(|h| h.raw_handle())
                    .unwrap_or_else(|| fuchsia_runtime::vmar_root_self().raw_handle()),
                libc::RTLD_NOW,
            )
        };
        if library.is_null() {
            log::error!("Failed to dlopen driver");
        }
        Ok(Library { ptr: NonNull::new(library).ok_or(Status::INTERNAL)? })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: It's necessary to close a library after creating it.
        unsafe { libc::dlclose(self.ptr.as_ptr()) };
    }
}

pub(crate) struct LoaderService {
    /// Protects us to ensure only a single LoaderService is ever installed at a time.
    lock: futures::lock::Mutex<()>,
}

static LOADER_SERVICE: LazyLock<LoaderService> =
    LazyLock::new(|| LoaderService { lock: futures::lock::Mutex::new(()) });

impl LoaderService {
    pub(crate) async fn install(overrides: BTreeMap<String, zx::Vmo>) -> Rc<Loader> {
        Loader::install(LOADER_SERVICE.lock.lock().await, overrides)
    }

    pub(crate) async fn try_load(vmo: zx::Vmo, vmar: &Option<zx::Vmar>) -> Result<Library, Status> {
        LOADER_SERVICE.lock.lock().await;
        Library::try_load(vmo, vmar)
    }
}

pub(crate) struct Loader {
    #[allow(unused)]
    lock_guard: futures::lock::MutexGuard<'static, ()>,
    scope: fuchsia_async::Scope,
    overrides: BTreeMap<String, zx::Vmo>,
    old_loader: Option<fldsvc::LoaderSynchronousProxy>,
}

impl Loader {
    fn install(
        lock_guard: futures::lock::MutexGuard<'static, ()>,
        overrides: BTreeMap<String, zx::Vmo>,
    ) -> Rc<Self> {
        // Install new loader service
        let (client_end, server_end) = fidl::endpoints::create_endpoints::<fldsvc::LoaderMarker>();
        // SAFETY: We are the only thing that is able to mess with the loader service in the
        // driver_host. We always re-install the original one, so there is always a valid one
        // currently installed.
        let old_loader = unsafe {
            zx::NullableHandle::from_raw(dl_set_loader_service(
                client_end.into_channel().into_raw(),
            ))
        };

        let loader = Rc::new(Loader {
            lock_guard,
            scope: fuchsia_async::Scope::new(),
            overrides,
            old_loader: Some(fldsvc::LoaderSynchronousProxy::from_channel(old_loader.into())),
        });
        loader.run_loader(server_end);
        loader
    }

    pub(crate) async fn try_load(
        &self,
        vmo: zx::Vmo,
        vmar: &Option<zx::Vmar>,
    ) -> Result<Library, Status> {
        // This needs to be done on another thread because it makes sync calls back into the
        // loader service we just installed.
        let vmar = vmar.as_ref().map(|v| v.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
        fuchsia_async::unblock(move || Library::try_load(vmo, &vmar)).await
    }

    fn load_object(&self, object_name: &str) -> Result<zx::Vmo, Status> {
        if let Some(vmo) = self.overrides.get(object_name) {
            Ok(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?)
        } else if let Some(old_loader) = &self.old_loader {
            match old_loader.load_object(object_name, zx::MonotonicInstant::INFINITE) {
                Ok((status, vmo)) => {
                    Status::ok(status)?;
                    Ok(vmo.ok_or(Status::INTERNAL)?)
                }
                Err(fidl::Error::ClientCall(status)) => Err(status),
                Err(_) => Err(Status::INTERNAL),
            }
        } else {
            Err(Status::INTERNAL)
        }
    }

    fn run_loader(self: &Rc<Self>, request: ServerEnd<fldsvc::LoaderMarker>) {
        let this = Rc::downgrade(self);
        self.scope.spawn_local(
            async move {
                let mut stream = request.into_stream();
                while let Some(req) = stream.try_next().await? {
                    let this = this.upgrade().unwrap();
                    match req {
                        fldsvc::LoaderRequest::Done { control_handle } => {
                            control_handle.shutdown();
                        }
                        fldsvc::LoaderRequest::LoadObject { object_name, responder } => {
                            let _ = match this.load_object(&object_name) {
                                Ok(vmo) => responder.send(zx::sys::ZX_OK, Some(vmo)),
                                Err(e) => responder.send(e.into_raw(), None),
                            };
                        }
                        fldsvc::LoaderRequest::Config { config, responder } => {
                            if let Some(old_loader) = &this.old_loader {
                                match old_loader.config(&config, zx::MonotonicInstant::INFINITE) {
                                    Ok(response) => responder.send(response)?,
                                    Err(fidl::Error::ClientCall(status)) => {
                                        responder.send(status.into_raw())?;
                                    }
                                    Err(_) => responder.send(zx::sys::ZX_ERR_INTERNAL)?,
                                };
                            } else {
                                responder.send(zx::sys::ZX_ERR_INTERNAL)?;
                            }
                        }
                        fldsvc::LoaderRequest::Clone { loader, responder } => {
                            this.run_loader(loader);
                            responder.send(zx::sys::ZX_OK)?;
                        }
                    };
                }
                Ok(())
            }
            .unwrap_or_else(|e: anyhow::Error| {
                log::warn!("Failed to serve loader service with error {e}")
            }),
        );
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // SAFETY: We need to re-install the old loader service in order to ensure there are no
        // safety problems in the future.
        let _ = unsafe {
            zx::NullableHandle::from_raw(dl_set_loader_service(
                self.old_loader.take().unwrap().into_channel().into_raw(),
            ))
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn loader_service_install() {
        let loader = LoaderService::install(BTreeMap::new()).await;
        assert_eq!(Rc::strong_count(&loader), 1);
    }
}
