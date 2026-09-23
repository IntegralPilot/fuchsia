// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use driver_manager_types::StartedComponent;
use fidl::endpoints::{ClientEnd, ServerEnd, create_endpoints};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_driver_loader as floader;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use fuchsia_runtime::{HandleType, take_startup_handle};
use futures::TryStreamExt;
use futures::channel::oneshot;
use log::{error, warn};
use rand::RngExt as _;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const TOKEN_ID: u32 =
    fuchsia_runtime::HandleInfo::new(fuchsia_runtime::HandleType::User0, 0).as_raw();

// TODO(https://fxbug.dev/341358132): support retrieving different vdsos. For now we will
// just use the driver manager's vdso.
fn get_vdso_vmo() -> Result<zx::Vmo, zx::Status> {
    let vdso = zx::Vmo::from(take_startup_handle(HandleType::VdsoVmo.into()).unwrap());
    vdso.duplicate_handle(zx::Rights::SAME_RIGHTS)
}

fn program_value(program: &fdata::Dictionary, key: &str) -> Result<String, zx::Status> {
    if let Some(entries) = &program.entries {
        for entry in entries {
            if entry.key == key
                && let Some(fdata::DictionaryValue::Str(s)) =
                    entry.value.as_ref().map(|v| v.as_ref())
            {
                return Ok(s.clone());
            }
        }
    }
    Err(zx::Status::NOT_FOUND)
}

fn ns_value(
    ns: &mut Vec<frunner::ComponentNamespaceEntry>,
    path: &str,
) -> Result<fio::DirectoryProxy, zx::Status> {
    for entry in ns {
        if entry.path.as_deref() == Some(path)
            && let Some(dir) = entry.directory.take()
        {
            return Ok(dir.into_proxy());
        }
    }
    Err(zx::Status::NOT_FOUND)
}

pub struct DriverHost {
    process: zx::Process,
    root_vmar: zx::Vmar,
}

impl DriverHost {
    pub fn new(process: zx::Process, root_vmar: zx::Vmar) -> Self {
        Self { process, root_vmar }
    }

    pub fn get_duplicate_handles(&self) -> Result<(zx::Process, zx::Vmar), zx::Status> {
        let process = self.process.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        let root_vmar = self.root_vmar.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
        Ok((process, root_vmar))
    }
}

pub type StartComponentCallback = oneshot::Sender<Result<StartedComponent, zx::Status>>;

pub struct DriverHostRunner {
    realm: fcomponent::RealmProxy,
    next_driver_host_id: Cell<u64>,
    driver_hosts: RefCell<Vec<Rc<DriverHost>>>,
    start_requests: Rc<RefCell<HashMap<zx::Koid, StartComponentCallback>>>,
}

impl DriverHostRunner {
    pub fn new(realm: fcomponent::RealmProxy) -> Rc<Self> {
        Rc::new(Self {
            realm,
            next_driver_host_id: Cell::new(rand::rng().random_range(0..1000)),
            driver_hosts: RefCell::new(Vec::new()),
            start_requests: Rc::new(RefCell::new(HashMap::new())),
        })
    }

    pub fn publish(self: &Rc<Self>, fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>) {
        let runner = self.clone();
        fs.dir("svc").add_fidl_service_at(
            "fuchsia.component.runner.DriverHostRunner",
            move |stream: frunner::ComponentRunnerRequestStream| {
                let runner = runner.clone();
                fasync::Task::local(async move {
                    runner.serve(stream).await.unwrap_or_else(|e| {
                        warn!("Failed to serve DriverHostRunner: {}", e);
                    });
                })
                .detach();
            },
        );
    }

    async fn serve(
        &self,
        mut stream: frunner::ComponentRunnerRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                    self.start(start_info, controller).await;
                }
                frunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                    warn!("Unknown ComponentRunner request: {}", ordinal);
                }
            }
        }
        Ok(())
    }

    async fn start(
        &self,
        start_info: frunner::ComponentStartInfo,
        controller: ServerEnd<frunner::ComponentControllerMarker>,
    ) {
        let url = start_info.resolved_url.as_deref().unwrap_or("");
        let handles = match start_info.numbered_handles.as_ref() {
            Some(h) => h,
            None => {
                error!("Failed to start driver host'{}', invalid request", url);
                controller.close_with_epitaph(zx::Status::INVALID_ARGS).ok();
                return;
            }
        };
        if handles.len() != 1 || handles[0].handle.is_invalid() || handles[0].id != TOKEN_ID {
            error!("Failed to start driver host '{}', invalid request", url);
            controller.close_with_epitaph(zx::Status::INVALID_ARGS).ok();
            return;
        }

        let koid = match handles[0].handle.koid() {
            Ok(koid) => koid,
            Err(_) => {
                controller.close_with_epitaph(zx::Status::INVALID_ARGS).ok();
                return;
            }
        };

        if self.start_requests.borrow().get(&koid).is_none() {
            error!("Failed to start driver host '{}', unknown request", url);
            controller.close_with_epitaph(zx::Status::UNAVAILABLE).ok();
            return;
        }

        if let Some(callback) = self.start_requests.borrow_mut().remove(&koid) {
            let _ = callback.send(Ok(StartedComponent { info: start_info, controller }));
        }
    }

    pub async fn start_driver_host(
        &self,
        launcher: floader::DriverHostLauncherProxy,
        exposed_dir: ServerEnd<fio::DirectoryMarker>,
    ) -> Result<ClientEnd<floader::DriverHostMarker>, Error> {
        let url = "fuchsia-boot:///driver_host2#meta/driver_host2.cm";
        let id = self.next_driver_host_id.get();
        let name = format!("driver-host-new-{}", id);
        self.next_driver_host_id.set(id + 1);

        let component = self.start_driver_host_component(&name, url, exposed_dir).await?;
        self.load_driver_host(launcher, component.info, &name).await
    }

    async fn load_driver_host(
        &self,
        launcher: floader::DriverHostLauncherProxy,
        mut start_info: frunner::ComponentStartInfo,
        name: &str,
    ) -> Result<ClientEnd<floader::DriverHostMarker>, Error> {
        let program = start_info.program.take().context("Missing 'program'")?;

        let binary = program_value(&program, "binary")
            .with_context(|| "Missing 'binary' argument".to_string())?;

        let mut ns = start_info.ns.take().unwrap_or_default();
        let pkg = ns_value(&mut ns, "/pkg").context("Missing '/pkg' directory")?;

        let exec_vmo = driver_manager_utils::open_pkg_file(&pkg, &binary)
            .await
            .with_context(|| format!("Failed to open driver host '{}' file", binary))?;

        let vdso_vmo =
            get_vdso_vmo().with_context(|| format!("Failed to get vdso vmo for {}", name))?;

        let driver_host = self
            .create_driver_host_process(name)
            .with_context(|| format!("Failed to create driver host process for {}", name))?;

        let (process, root_vmar) = driver_host
            .get_duplicate_handles()
            .with_context(|| format!("GetDuplicateHandles failed for {}", name))?;

        let lib_dir = driver_manager_utils::open_lib_dir(&pkg).inspect_err(|e| {
            error!("Failed to open lib directory {}", e);
        })?;

        let (client_end, server_end) = create_endpoints::<floader::DriverHostMarker>();
        let args = floader::DriverHostLauncherLaunchRequest {
            process: Some(process),
            root_vmar: Some(root_vmar),
            driver_host_binary: Some(exec_vmo),
            vdso: Some(vdso_vmo),
            driver_host_libs: Some(lib_dir),
            driver_host: Some(server_end),
            ..Default::default()
        };

        launcher
            .launch(args)
            .await
            .map_err(|e| {
                error!("Failed to start driver host: {}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)?;

        Ok(client_end)
    }

    fn create_driver_host_process(&self, name: &str) -> Result<Rc<DriverHost>, zx::Status> {
        let job = fuchsia_runtime::job_default()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .map_err(|_| zx::Status::INTERNAL)?;
        let (process, root_vmar) =
            job.create_child_process(zx::ProcessOptions::empty(), name.as_bytes())?;
        let driver_host = Rc::new(DriverHost::new(process, root_vmar));
        self.driver_hosts.borrow_mut().push(driver_host.clone());
        Ok(driver_host)
    }

    async fn start_driver_host_component(
        &self,
        moniker: &str,
        url: &str,
        exposed_dir: ServerEnd<fio::DirectoryMarker>,
    ) -> Result<StartedComponent, zx::Status> {
        let token = zx::Event::create();
        let koid = token.koid()?;

        let (tx, rx) = oneshot::channel();
        self.start_requests.borrow_mut().insert(koid, tx);

        let child_decl = fdecl::Child {
            name: Some(moniker.to_string()),
            url: Some(url.to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };

        let handle_info = fprocess::HandleInfo { handle: token.into(), id: TOKEN_ID };

        let create_child_args = fcomponent::CreateChildArgs {
            numbered_handles: Some(vec![handle_info]),
            ..Default::default()
        };

        let collection_ref = fdecl::CollectionRef { name: "driver-hosts".to_string() };

        let realm = self.realm.clone();
        let child_moniker = moniker.to_string();
        let start_requests = self.start_requests.clone();
        fasync::Task::local(async move {
            let result = realm.create_child(&collection_ref, &child_decl, create_child_args).await;
            let is_error = match result {
                Ok(Ok(())) => false,
                Ok(Err(e)) => {
                    error!("Failed to create child '{}': {:?}", child_moniker, e);
                    true
                }
                Err(e) => {
                    error!("Failed to create child '{}': {}", child_moniker, e);
                    true
                }
            };

            if is_error {
                if let Some(callback) = start_requests.borrow_mut().remove(&koid) {
                    let _ = callback.send(Err(zx::Status::INTERNAL));
                }
                return;
            }

            let child_ref = fdecl::ChildRef {
                name: child_moniker.clone(),
                collection: Some("driver-hosts".to_string()),
            };
            let open_result = realm.open_exposed_dir(&child_ref, exposed_dir).await;
            if let Err(e) = open_result {
                error!(
                    "Failed to open exposed directory for driver host: '{}': {}",
                    child_moniker, e
                );
            } else if let Ok(Err(e)) = open_result {
                error!(
                    "Failed to open exposed directory for driver host: '{}': {:?}",
                    child_moniker, e
                );
            }
        })
        .detach();
        rx.await.map_err(|_| zx::Status::INTERNAL)?
    }
}
