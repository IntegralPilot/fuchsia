// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bootup_tracker::BootupTracker;
use crate::driver_host_runner::DriverHostRunner;
use crate::memory_attribution::MemoryAttributor;
use crate::offer_injection::OfferInjector;
use crate::runner::Runner;
use crate::{DriverRunnerBridge, LoaderServiceFactory, perform_bfs, to_collection};
use driver_manager_bind::BindManagerHandle;
use driver_manager_composite::{CompositeNodeSpec, CompositeNodeSpecManager};
use driver_manager_devfs::Devfs;
use driver_manager_driver_host::{DriverHost, DriverHostComponent};
use driver_manager_node::Node;
use driver_manager_shutdown::NodeRemovalTracker;
use driver_manager_types::{Collection, to_bind_rule2, to_property2};
use driver_manager_utils::DictionaryUtil;
use fidl::endpoints::{ServerEnd, create_endpoints};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_driver_crash as fcrash;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_host as fdh;
use fidl_fuchsia_driver_index as fdi;
use fidl_fuchsia_driver_token as fdt;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use fuchsia_inspect as inspect;
use fuchsia_inspect::ArrayProperty;
use futures::StreamExt;
use futures::channel::oneshot;
use log::{debug, error, info, warn};
use rand::rngs::StdRng;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

pub struct DriverRunner {
    pub(crate) driver_index: fdi::DriverIndexProxy,
    pub(crate) dictionary_util: Rc<DictionaryUtil>,
    loader_service_factory: LoaderServiceFactory,
    pub(crate) root_node: Rc<Node>,
    pub bind_manager: BindManagerHandle,
    pub(crate) composite_node_spec_manager: CompositeNodeSpecManager,
    runner: Runner,
    driver_host_runner: Rc<DriverHostRunner>,
    pub(crate) removal_tracker: Rc<RefCell<NodeRemovalTracker>>,
    pub bootup_tracker: Rc<BootupTracker>,
    driver_hosts: RefCell<Vec<Weak<dyn DriverHost>>>,
    pub devfs: Arc<Devfs>,
    pub(crate) memory_attributor: Rc<MemoryAttributor>,
    launcher: Option<fidl_fuchsia_driver_loader::DriverHostLauncherProxy>,
    pub(crate) enable_test_shutdown_delays: bool,
    pub(crate) shutdown_test_rng: Rc<RefCell<StdRng>>,
    pub(crate) scope: fasync::Scope,
}

impl DriverRunner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        realm: fcomponent::RealmProxy,
        introspector: fcomponent::IntrospectorProxy,
        dictionary_util: DictionaryUtil,
        driver_index: fdi::DriverIndexProxy,
        loader_service_factory: LoaderServiceFactory,
        enable_test_shutdown_delays: bool,
        offer_injector: OfferInjector,
        devfs: Arc<Devfs>,
    ) -> Rc<Self> {
        Rc::new_cyclic(|weak_driver_runner| {
            let bind_manager_bridge = Box::new(DriverRunnerBridge(weak_driver_runner.clone()));
            let bind_manager = BindManagerHandle::new(bind_manager_bridge);

            let composite_manager_bridge = Box::new(DriverRunnerBridge(weak_driver_runner.clone()));
            let composite_node_spec_manager =
                CompositeNodeSpecManager::new(composite_manager_bridge);

            let bootup_tracker = BootupTracker::new(bind_manager.clone());

            let root_node = Node::new(
                "dev",
                Weak::new(),
                Box::new(DriverRunnerBridge(weak_driver_runner.clone())),
            );
            root_node.setup_devfs_for_root_node(devfs.root_node());

            let runner = Runner::new(realm.clone(), introspector, offer_injector);
            let driver_host_runner = DriverHostRunner::new(realm);
            let removal_tracker = NodeRemovalTracker::new();
            let memory_attributor = Rc::new(MemoryAttributor::new());

            Self {
                driver_index,
                dictionary_util: Rc::new(dictionary_util),
                loader_service_factory,
                root_node,
                bind_manager,
                composite_node_spec_manager,
                runner,
                driver_host_runner,
                removal_tracker,
                bootup_tracker,
                driver_hosts: RefCell::new(Vec::new()),
                devfs,
                memory_attributor,
                launcher: None,
                enable_test_shutdown_delays,
                shutdown_test_rng: Rc::new(RefCell::new(rand::make_rng())),
                scope: fasync::Scope::new_with_name("driver_runner"),
            }
        })
    }

    pub fn register_notifier(self: &Rc<Self>) -> Result<(), anyhow::Error> {
        let (client, server) = create_endpoints();
        self.driver_index.set_notifier(client)?;
        let weak_self = Rc::downgrade(self);
        self.scope.spawn_local(async move {
            let mut stream = server.into_stream();
            while let Some(Ok(msg)) = stream.next().await {
                match msg {
                    fdi::DriverNotifierRequest::NewDriverAvailable { .. } => {
                        let Some(this) = weak_self.upgrade() else {
                            return;
                        };
                        let _ = this.bind_manager.try_bind_all_available().await;
                    }
                }
            }
        });
        Ok(())
    }

    pub fn root_node(&self) -> Rc<Node> {
        self.root_node.clone()
    }

    pub fn get_composite_list_info(&self) -> Vec<fdd::CompositeNodeInfo> {
        self.composite_node_spec_manager.get_composite_info()
    }

    pub fn driver_hosts(&self) -> Vec<Rc<dyn DriverHost>> {
        self.driver_hosts.borrow().iter().filter_map(|w| w.upgrade()).collect()
    }

    pub fn get_driver_host(
        &self,
        driver_host_name_for_colocation: &str,
    ) -> Option<Rc<dyn DriverHost>> {
        if driver_host_name_for_colocation.is_empty() {
            return None;
        }
        for host_weak in self.driver_hosts.borrow().iter() {
            if let Some(host) = host_weak.upgrade()
                && host.name_for_colocation() == driver_host_name_for_colocation
            {
                return Some(host);
            }
        }
        None
    }

    pub async fn start_root_driver(self: &Rc<Self>, url: String) -> Result<(), zx::Status> {
        self.bootup_tracker.start();
        let package_type = if url.starts_with("fuchsia-boot://") {
            fdf::DriverPackageType::Boot
        } else {
            fdf::DriverPackageType::Base
        };

        let (sender, receiver) = oneshot::channel();
        self.root_node.set_driver_host_name_for_colocation("root");
        let self_clone = self.clone();
        self.scope.spawn_local(async move {
            if let Err(e) = self_clone.start_driver(&self_clone.root_node, &url, package_type).await
            {
                error!("Failed to start root driver: {}", e);
                sender.send(Err(e)).unwrap();
            } else {
                sender.send(Ok(())).unwrap();
            }
        });
        receiver.await.map_err(|_| zx::Status::CANCELED)?
    }

    pub fn start_devfs_driver(self: &Rc<Self>) {
        let self_clone = self.clone();
        self.scope.spawn_local(async move {
            let (client, server) = fidl::endpoints::create_proxy::<fcomponent::ControllerMarker>();
            let result = self_clone
                .runner
                .create_driver_component(
                    "devfs_driver",
                    "fuchsia-boot:///devfs-driver#meta/devfs-driver.cm",
                    &Collection::Boot.to_string(),
                    &[],
                    None,
                    true,
                    server,
                )
                .await;
            if let Ok((handle, receiver)) = result {
                self_clone.devfs.set_component_controller_proxy(client);
                if let Err(e) = self_clone.devfs.attach_component(handle, receiver).await {
                    error!("Failed to attach devfs component: {}", e);
                }
            } else {
                error!("Starting the devfs component failed");
            }
        });
    }

    pub async fn start_driver(
        self: &Rc<Self>,
        node: &Rc<Node>,
        url: &str,
        package_type: fdf::DriverPackageType,
    ) -> Result<(), zx::Status> {
        // Ensure `node`'s collection is equal to or higher ranked than its ancestor
        // nodes' collections. This is to avoid node components having a dependency
        // cycle with each other. For example, node components in the boot driver
        // collection depend on the devfs component which ultimately depends on all
        // components within the package driver collection. If a package driver
        // component depended on a component in the boot driver collection (a lower
        // ranked collection than the package driver collection) then a cyclic
        // dependency would occur.
        node.set_collection(to_collection(node, package_type));
        node.set_driver_package_type(package_type);

        let moniker = node.make_component_moniker();
        self.bootup_tracker.notify_new_start_request(
            moniker.clone(),
            url.to_string(),
            node.weak_from_this(),
        );

        match self.start_driver_internal(node, url, &moniker).await {
            Ok(_) => {
                node.complete_bind(Ok(())).await;
                self.bootup_tracker.notify_start_complete(&moniker);
                Ok(())
            }
            Err(err) => {
                node.on_start_error(err);
                node.complete_bind(Err(err)).await;
                self.bootup_tracker.notify_start_complete(&moniker);
                Err(err)
            }
        }
    }

    async fn start_driver_internal(
        self: &Rc<Self>,
        node: &Rc<Node>,
        url: &str,
        moniker: &str,
    ) -> Result<(), zx::Status> {
        let dictionary = node.prepare_dictionary().await;

        if !node.has_component_controller_proxy() {
            let (client, server) = fidl::endpoints::create_proxy::<fcomponent::ControllerMarker>();

            let offers = node.offers().clone();
            let create_result = self
                .runner
                .create_driver_component(
                    moniker,
                    url,
                    &node.collection().to_string(),
                    &offers,
                    dictionary,
                    node.skip_injected_offers(),
                    server,
                )
                .await;

            match create_result {
                Ok((handle_info, receiver)) => {
                    node.set_created_info(client, handle_info, receiver).await;
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }

        node.send_start_request().await?;
        let (start_info, controller) = node.get_next_start_request().await?;
        node.start_driver(start_info, controller).await
    }

    pub async fn create_driver_host(
        &self,
        use_next_vdso: bool,
        driver_host_name_for_colocation: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status> {
        let (exposed_dir_client, exposed_dir_server) = create_endpoints::<fio::DirectoryMarker>();
        let name = if !driver_host_name_for_colocation.is_empty() {
            format!("driver-host-{}", driver_host_name_for_colocation.trim_start_matches('#'))
        } else {
            format!("driver-host-{}", self.driver_hosts.borrow().len())
        };

        self.create_driver_host_component(&name, exposed_dir_server, use_next_vdso)?;

        let driver_host_proxy =
            connect_to_protocol_at_dir_root::<fdh::DriverHostMarker>(&exposed_dir_client)
                .map_err(|_| zx::Status::INTERNAL)?;

        let (tx, rx) = oneshot::channel();
        let _ = self.loader_service_factory.unbounded_send(tx);
        let loader_service_client = rx.await.map_err(|e| {
            error!("Failed to connect to loader service: {}", e);
            zx::Status::INTERNAL
        })??;

        let driver_host: Rc<dyn DriverHost> = Rc::new(DriverHostComponent::new(
            driver_host_proxy,
            None,
            ExecutionScope::new(),
            driver_host_name_for_colocation,
        ));
        driver_host.install_loader(loader_service_client)?;

        self.driver_hosts.borrow_mut().push(Rc::downgrade(&driver_host));

        Ok(driver_host)
    }

    pub async fn create_driver_host_dynamic_linker(
        self: &Rc<Self>,
        driver_host_name_for_colocation: String,
    ) -> Result<Rc<dyn DriverHost>, zx::Status> {
        let driver_host_runner = self.driver_host_runner.clone();
        let launcher = self.launcher.clone().unwrap();
        let (exposed_dir_client, exposed_dir_server) = create_endpoints::<fio::DirectoryMarker>();
        let loader_client = driver_host_runner
            .start_driver_host(launcher, exposed_dir_server)
            .await
            .map_err(|e| {
                error!("Failed to start driver host: {e:?}");
                zx::Status::INTERNAL
            })?;

        let driver_host_client =
            connect_to_protocol_at_dir_root::<fdh::DriverHostMarker>(&exposed_dir_client)
                .map_err(|_| zx::Status::INTERNAL)?;
        let driver_host: Rc<dyn DriverHost> = Rc::new(DriverHostComponent::new(
            driver_host_client,
            Some(loader_client.into_proxy()),
            ExecutionScope::new(),
            driver_host_name_for_colocation,
        ));
        self.driver_hosts.borrow_mut().push(Rc::downgrade(&driver_host));
        Ok(driver_host)
    }

    fn create_driver_host_component(
        &self,
        moniker: &str,
        exposed_dir: ServerEnd<fio::DirectoryMarker>,
        use_next_vdso: bool,
    ) -> Result<(), zx::Status> {
        let url = if use_next_vdso {
            "fuchsia-boot:///driver_host#meta/driver_host_next.cm"
        } else {
            "fuchsia-boot:///driver_host#meta/driver_host.cm"
        };

        let child_decl = fdecl::Child {
            name: Some(moniker.to_string()),
            url: Some(url.to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };

        let create_child_args = fcomponent::CreateChildArgs::default();

        let realm = self.runner.realm.clone();
        let child_moniker = moniker.to_string();
        self.scope.spawn_local(async move {
            let result = realm
                .create_child(
                    &fdecl::CollectionRef { name: "driver-hosts".to_string() },
                    &child_decl,
                    create_child_args,
                )
                .await;

            if let Err(e) = result {
                error!("Failed to create driver host '{}': {}", child_moniker, e);
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
            }
        });

        Ok(())
    }

    pub async fn destroy_driver_host(
        &self,
        driver_host_name_for_colocation: String,
    ) -> Result<(), zx::Status> {
        let name = if !driver_host_name_for_colocation.is_empty() {
            let suffix = driver_host_name_for_colocation.trim_start_matches('#');
            format!("driver-host-{}", suffix)
        } else {
            return Err(zx::Status::INVALID_ARGS);
        };

        let child_ref =
            fdecl::ChildRef { name: name.clone(), collection: Some("driver-hosts".to_string()) };

        let result = self.runner.realm.destroy_child(&child_ref).await;
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(fcomponent::Error::InstanceNotFound)) => Ok(()),
            Ok(Err(fcomponent::Error::InstanceDied)) => Ok(()),
            Ok(Err(e)) => {
                error!("Failed to destroy driver host '{}': {:?}", name, e);
                Err(zx::Status::INTERNAL)
            }
            Err(e) => {
                error!("Failed to destroy driver host '{}': {}", name, e);
                Err(zx::Status::INTERNAL)
            }
        }
    }

    pub fn publish(self: &Rc<Self>, fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>) {
        self.runner.publish(fs);
        self.driver_host_runner.publish(fs);
        self.memory_attributor.publish(fs);

        let this = self.clone();
        fs.dir("svc").add_fidl_service(move |stream: fdf::CompositeNodeManagerRequestStream| {
            this.serve_composite_node_manager(stream);
        });

        let this = self.clone();
        fs.dir("svc").add_fidl_service(move |stream: fdt::NodeBusTopologyRequestStream| {
            this.serve_node_bus_topology(stream);
        });

        let this = self.clone();
        fs.dir("svc").add_fidl_service(move |stream: fcrash::CrashIntrospectRequestStream| {
            this.serve_crash_introspect(stream);
        });

        let this = self.clone();
        fs.dir("svc").add_fidl_service(move |stream: fdt::DebugRequestStream| {
            this.serve_debug(stream);
        });
    }

    pub fn serve_debug(self: &Rc<Self>, mut stream: fdt::DebugRequestStream) {
        let this = self.clone();
        self.scope.spawn_local(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fdt::DebugRequest::LogStackTrace { node_token, responder } => {
                        let result = this.log_stack_trace(node_token).await;
                        let _ = match result {
                            Ok(()) => responder.send(Ok(())),
                            Err(status) => responder.send(Err(status.into_raw())),
                        };
                    }
                    fdt::DebugRequest::GetHostKoid { node_token, responder } => {
                        let result = this.get_host_koid(node_token).await;
                        let _ = match result {
                            Ok(host_koid) => responder.send(Ok(host_koid.raw_koid())),
                            Err(status) => responder.send(Err(status.into_raw())),
                        };
                    }
                    fdt::DebugRequest::_UnknownMethod { ordinal, .. } => {
                        warn!("Unknown Debug request: {}", ordinal);
                    }
                }
            }
        });
    }

    async fn log_stack_trace(&self, node_token: zx::Event) -> Result<(), zx::Status> {
        let token_koid = node_token.basic_info()?.koid;
        let node = self.find_node_by_token_koid(token_koid).await;
        if let Some(node) = node {
            if let Some(host) = node.driver_host() {
                host.trigger_stack_trace();
                Ok(())
            } else {
                Err(zx::Status::NOT_FOUND)
            }
        } else {
            Err(zx::Status::NOT_FOUND)
        }
    }

    async fn get_host_koid(&self, node_token: zx::Event) -> Result<zx::Koid, zx::Status> {
        let token_koid = node_token.basic_info()?.koid;
        let node = self.find_node_by_token_koid(token_koid).await;
        if let Some(node) = node {
            if let Some(host) = node.driver_host() {
                host.get_process_koid().await
            } else {
                Err(zx::Status::NOT_FOUND)
            }
        } else {
            Err(zx::Status::NOT_FOUND)
        }
    }

    pub fn serve_composite_node_manager(
        self: &Rc<Self>,
        mut stream: fdf::CompositeNodeManagerRequestStream,
    ) {
        let this = self.clone();
        self.scope.spawn_local(async move {
            while let Some(Ok(request)) = stream.next().await {
                this.handle_composite_node_manager_request(request).await;
            }
        });
    }

    async fn handle_composite_node_manager_request(
        self: &Rc<Self>,
        request: fdf::CompositeNodeManagerRequest,
    ) {
        match request {
            fdf::CompositeNodeManagerRequest::AddSpec { payload, responder } => {
                let result = self.add_spec(payload).await;
                let _ = responder.send(result);
            }
            fdf::CompositeNodeManagerRequest::_UnknownMethod { .. } => (),
        }
    }

    async fn add_spec(
        self: &Rc<Self>,
        spec: fdf::CompositeNodeSpec,
    ) -> Result<(), fdf::CompositeNodeSpecError> {
        let name = spec.name.clone().ok_or(fdf::CompositeNodeSpecError::MissingArgs)?;

        let parents_present = spec.parents.is_some();
        let parents2_present = spec.parents2.is_some();

        if !parents_present && !parents2_present {
            return Err(fdf::CompositeNodeSpecError::MissingArgs);
        }

        if parents_present && parents2_present {
            return Err(fdf::CompositeNodeSpecError::DuplicateParents);
        }

        let parents2 = if let Some(ref parents) = spec.parents {
            if parents.is_empty() {
                return Err(fdf::CompositeNodeSpecError::EmptyNodes);
            }
            parents
                .iter()
                .map(|parent| {
                    let bind_rules = parent.bind_rules.iter().map(to_bind_rule2).collect();
                    let properties = parent.properties.iter().map(to_property2).collect();
                    fdf::ParentSpec2 { bind_rules, properties }
                })
                .collect()
        } else if let Some(ref parents2) = spec.parents2 {
            if parents2.is_empty() {
                return Err(fdf::CompositeNodeSpecError::EmptyNodes);
            }
            parents2.clone()
        } else {
            unreachable!();
        };

        let driver_host_name_for_colocation = spec.driver_host.clone().unwrap_or_default();

        let spec_for_manager = CompositeNodeSpec::new(
            name,
            parents2,
            Box::new(DriverRunnerBridge(Rc::downgrade(self))),
            driver_host_name_for_colocation,
        );

        self.composite_node_spec_manager.add_spec(spec, spec_for_manager).await
    }

    pub fn serve_node_bus_topology(self: &Rc<Self>, mut stream: fdt::NodeBusTopologyRequestStream) {
        let this = self.clone();
        self.scope.spawn_local(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fdt::NodeBusTopologyRequest::Get { token, responder } => {
                        let result = this.get_bus_topology(token).await;
                        let _ = match result {
                            Ok(topology) => responder.send(Ok(&topology)),
                            Err(status) => responder.send(Err(status.into_raw())),
                        };
                    }
                    fdt::NodeBusTopologyRequest::_UnknownMethod { .. } => (),
                }
            }
        });
    }

    async fn get_bus_topology(&self, token: zx::Event) -> Result<Vec<fdf::BusInfo>, zx::Status> {
        let token_koid = token.basic_info()?.koid;

        let node = self.find_node_by_token_koid(token_koid).await;

        if let Some(node) = node { Ok(node.get_bus_topology()) } else { Err(zx::Status::NOT_FOUND) }
    }

    pub fn serve_crash_introspect(
        self: &Rc<Self>,
        mut stream: fcrash::CrashIntrospectRequestStream,
    ) {
        let this = self.clone();
        self.scope.spawn_local(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fcrash::CrashIntrospectRequest::FindDriverCrash {
                        process_koid,
                        thread_koid,
                        responder,
                    } => {
                        let result = this
                            .find_driver_crash(
                                zx::Koid::from_raw(process_koid),
                                zx::Koid::from_raw(thread_koid),
                            )
                            .await;
                        let _ = match result {
                            Ok(info) => responder.send(Ok(&info)),
                            Err(status) => responder.send(Err(status.into_raw())),
                        };
                    }
                }
            }
        });
    }

    async fn find_driver_crash(
        &self,
        process_koid: zx::Koid,
        thread_koid: zx::Koid,
    ) -> Result<fcrash::DriverCrashInfo, zx::Status> {
        let hosts = self.driver_hosts.borrow().clone();
        for host in hosts {
            if let Some(host) = host.upgrade()
                && let Ok(koid) = host.get_process_koid().await
                && koid == process_koid
            {
                let crash_info = host.get_crash_info(thread_koid).await?;
                let token = crash_info.node_token.ok_or(zx::Status::INTERNAL)?;
                let token_koid = token.into_handle().basic_info()?.koid;

                let node = self.find_node_by_token_koid(token_koid).await;

                if let Some(node) = node {
                    return Ok(fcrash::DriverCrashInfo {
                        node_moniker: Some(node.make_component_moniker()),
                        url: crash_info.url,
                        ..Default::default()
                    });
                } else {
                    return Err(zx::Status::NOT_FOUND);
                }
            }
        }
        Err(zx::Status::NOT_FOUND)
    }

    async fn find_node_by_token_koid(&self, token_koid: zx::Koid) -> Option<Rc<Node>> {
        let mut result: Option<Rc<Node>> = None;
        perform_bfs(self.root_node(), async |current| {
            if result.is_some() {
                return false; // Already found.
            }
            if let Some(current_koid) = current.token_koid()
                && current_koid == token_koid
            {
                result = Some(current.clone());
                return false;
            }
            true
        })
        .await;
        result
    }

    pub async fn rebind_composites_with_driver(&self, driver_url: &str) -> u32 {
        let mut names = HashSet::new();
        perform_bfs(self.root_node(), async |node| {
            if node.is_composite() && node.driver_url() == driver_url {
                names.insert(node.name().to_string());
                return false; // Do not visit children
            }
            true // Continue to children
        })
        .await;

        let count = names.len() as u32;
        for name in names {
            let _ = self.composite_node_spec_manager.rebind(name, None).await;
        }

        count
    }

    pub async fn restart_nodes_colocated_with_driver_url(
        self: &Rc<Self>,
        url: &str,
        rematch_flags: fdd::RestartRematchFlags,
    ) -> Result<u32, zx::Status> {
        // Step 1: Find all driver hosts with the given driver URL.
        let mut driver_hosts = HashSet::new();
        perform_bfs(self.root_node(), async |node| {
            if node.driver_url() == url
                && let Some(host) = node.driver_host()
            {
                // We need a way to uniquely identify the host.
                // Using the raw pointer should work.
                driver_hosts.insert(Rc::as_ptr(&host) as *const ());
            }
            true // Continue BFS
        })
        .await;

        if driver_hosts.is_empty() {
            warn!(
                "restart_nodes_colocated_with_driver_url: no driver hosts found with url {}",
                url
            );
            return Ok(0);
        }

        let driver_host_count = driver_hosts.len() as u32;

        // Step 2: Perform another BFS to restart the nodes.
        let this = self.clone();
        perform_bfs(self.root_node(), async |node| {
            let host_ptr = node.driver_host().map(|host| Rc::as_ptr(&host) as *const ());
            if host_ptr.is_none() || !driver_hosts.contains(&host_ptr.unwrap()) {
                // Not in one of the restarting hosts. Continue to children.
                return true;
            }

            // This node is in a driver host that needs to be restarted.
            if node.evaluate_rematch_flags(rematch_flags, url) {
                if node.is_composite() {
                    debug!(
                        "RestartNodesColocatedWithDriverUrl rebinding composite {}",
                        node.make_component_moniker()
                    );
                    let _ = this
                        .composite_node_spec_manager
                        .rebind(node.name().to_string(), None)
                        .await;
                } else {
                    debug!(
                        "RestartNodesColocatedWithDriverUrl restarting node with rematch {}",
                        node.make_component_moniker()
                    );
                    let (tx, _rx) = oneshot::channel();
                    node.restart_node_with_rematch(Some("".to_string()), tx);
                }
            } else {
                info!(
                    "RestartNodesColocatedWithDriverUrl restarting node {}",
                    node.make_component_moniker()
                );
                node.restart_node();
            }

            false // Do not visit children.
        })
        .await;

        Ok(driver_host_count)
    }

    pub async fn restart_with_dictionary(
        self: &Rc<Self>,
        moniker: String,
        dictionary: fsandbox::DictionaryRef,
        reset_eventpair: zx::EventPair,
    ) {
        let imported = self.dictionary_util.import_dictionary(dictionary).await;
        let imported = match imported {
            Ok(imported) => imported,
            Err(e) => {
                error!("Failed to import dictionary: {}", e);
                return;
            }
        };

        let mut restarted_node: Option<Rc<Node>> = None;
        perform_bfs(self.root_node(), async |node| {
            if restarted_node.is_some() {
                return false; // Already found, stop searching.
            }

            if node.make_component_moniker() == moniker {
                if node.has_subtree_dictionary() {
                    error!(concat!(
                        "RestartWithDictionary requested node id already contains a ",
                        "dictionary_ref from another RestartWithDictionary operation."
                    ));
                    return false; // Stop searching
                }
                assert!(restarted_node.is_none(), "Multiple nodes with same moniker not possible.");
                restarted_node = Some(node.clone());
                node.set_subtree_dictionary(imported);
                node.restart_node();
                return false; // Found it, stop searching.
            }

            true // Continue searching
        })
        .await;

        if let Some(restarted_node) = restarted_node {
            self.scope.spawn_local(async move {
                let signals = zx::Signals::EVENTPAIR_PEER_CLOSED | zx::Signals::EVENTPAIR_SIGNALED;
                let on_signals = fasync::OnSignals::new(&reset_eventpair, signals);
                on_signals.await.expect("failed to wait on eventpair");

                info!("RestartWithDictionary operation released.");
                restarted_node.remove_subtree_dictionary();
                restarted_node.restart_node();
            });
        }
    }

    pub fn inspect(&self) -> inspect::Inspector {
        let inspector =
            inspect::Inspector::new(inspect::InspectorConfig::default().size(2 * 256 * 1024));

        let mut roots = Vec::new();
        let mut unique_nodes = HashSet::new();

        let device_tree = inspector.root().create_child("node_topology");
        let mut root_node_inspect = device_tree.create_child(self.root_node.name());

        self.inspect_node_recursive(
            &self.root_node,
            &mut root_node_inspect,
            &mut roots,
            &mut unique_nodes,
        );

        device_tree.record(root_node_inspect);
        inspector.root().record(device_tree);

        for root in roots {
            inspector.root().record(root);
        }

        self.bind_manager.record_inspect(inspector.root());

        inspector
    }

    fn inspect_node_recursive(
        &self,
        node: &Rc<Node>,
        inspect_node: &mut inspect::Node,
        roots: &mut Vec<inspect::Node>,
        unique_nodes: &mut HashSet<*const Node>,
    ) {
        let node_ptr = Rc::as_ptr(node);
        if !unique_nodes.insert(node_ptr) {
            return;
        }

        let offers = node.offers();
        if !offers.is_empty() {
            let array = inspect_node.create_string_array("offers", offers.len());
            for (i, offer) in offers.iter().enumerate() {
                array.set(i, &offer.service_name);
            }
            inspect_node.record(array);
        }

        let symbols = node.symbols();
        if !symbols.is_empty() {
            let array = inspect_node.create_string_array("symbols", symbols.len());
            for (i, symbol) in symbols.iter().enumerate() {
                if let Some(name) = &symbol.name {
                    array.set(i, name);
                }
            }
            inspect_node.record(array);
        }

        if let Some(properties) = node.get_node_properties(None)
            && !properties.is_empty()
        {
            inspect_node.record_child("properties", |properties_node| {
                for (i, property) in properties.iter().enumerate() {
                    properties_node.record_child(i.to_string(), |inspect_property| {
                        inspect_property.record_string("key", &property.key);
                        match &property.value {
                            fdf::NodePropertyValue::StringValue(s) => {
                                inspect_property.record_string("value", s)
                            }
                            fdf::NodePropertyValue::IntValue(i) => {
                                inspect_property.record_uint("value", *i as u64)
                            }
                            fdf::NodePropertyValue::EnumValue(e) => {
                                inspect_property.record_string("value", e)
                            }
                            fdf::NodePropertyValue::BoolValue(b) => {
                                inspect_property.record_bool("value", *b)
                            }
                            _ => inspect_property.record_string("value", "UNKNOWN VALUE TYPE"),
                        }
                    });
                }
            });
        }

        inspect_node
            .record_string("type", if node.is_composite() { "Composite Device" } else { "Device" });
        inspect_node.record_string("topological_path", node.make_topological_path(false));
        inspect_node.record_string("driver", node.driver_url());

        for child in node.children() {
            let mut child_inspect_node = inspect_node.create_child(child.name());
            self.inspect_node_recursive(&child, &mut child_inspect_node, roots, unique_nodes);
            roots.push(child_inspect_node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MockDriverHost, MockNodeManager};
    use futures::channel::mpsc;
    use std::sync::atomic::Ordering;

    use fidl_fuchsia_driver_index as fdi;

    #[fuchsia::test]
    async fn test_log_stack_trace() {
        let (realm, _) = fidl::endpoints::create_proxy::<fcomponent::RealmMarker>();
        let (introspector, _) = fidl::endpoints::create_proxy::<fcomponent::IntrospectorMarker>();
        let (driver_index, _) = fidl::endpoints::create_proxy::<fdi::DriverIndexMarker>();
        let (capability_store, _) =
            fidl::endpoints::create_proxy::<fsandbox::CapabilityStoreMarker>();
        let (loader_service_factory, _) = mpsc::unbounded();

        let driver_runner = DriverRunner::new(
            realm,
            introspector,
            DictionaryUtil::new(capability_store),
            driver_index,
            loader_service_factory,
            false,
            OfferInjector::new(crate::offer_injection::PowerOffersConfig {
                power_inject_offer: false,
                power_suspend_enabled: false,
            }),
            Devfs::new(mpsc::unbounded().0),
        );

        let node_manager = Box::new(MockNodeManager);
        let node = Node::new("test_node", Rc::downgrade(&driver_runner.root_node), node_manager);
        let host = Rc::new(MockDriverHost::new());
        node.set_host(host.clone());

        // We need the node to have a token and be found by find_node_by_token_koid.
        // Node::token_koid() returns Some(koid) if it's in DriverComponent state.
        let token = zx::Event::create();
        let token_remote = token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let koid = token.koid().unwrap();

        node.set_state_for_testing(driver_manager_node::types::NodeState::DriverComponent(
            driver_manager_node::types::DriverComponent::new(
                "url".to_string(),
                token_remote,
                koid,
                None,
                None,
                None,
                driver_manager_node::types::DriverState::Running,
            ),
        ));

        // Add the node to the driver runner's root node as a child.
        driver_runner.root_node.add_child_to_children_for_testing(node);

        // Call LogStackTrace with the token.
        let result = driver_runner.log_stack_trace(token).await;
        assert!(result.is_ok());
        assert_eq!(host.stack_trace_count.load(Ordering::SeqCst), 1);

        // Call with a random token should return NOT_FOUND.
        let random_token = zx::Event::create();
        let result = driver_runner.log_stack_trace(random_token).await;
        assert_eq!(result, Err(zx::Status::NOT_FOUND));
    }

    #[fuchsia::test]
    async fn test_get_host_koid() {
        let (realm, _) = fidl::endpoints::create_proxy::<fcomponent::RealmMarker>();
        let (introspector, _) = fidl::endpoints::create_proxy::<fcomponent::IntrospectorMarker>();
        let (driver_index, _) = fidl::endpoints::create_proxy::<fdi::DriverIndexMarker>();
        let (capability_store, _) =
            fidl::endpoints::create_proxy::<fsandbox::CapabilityStoreMarker>();
        let (loader_service_factory, _) = mpsc::unbounded();

        let driver_runner = DriverRunner::new(
            realm,
            introspector,
            DictionaryUtil::new(capability_store),
            driver_index,
            loader_service_factory,
            false,
            OfferInjector::new(crate::offer_injection::PowerOffersConfig {
                power_inject_offer: false,
                power_suspend_enabled: false,
            }),
            Devfs::new(mpsc::unbounded().0),
        );

        let node_manager = Box::new(MockNodeManager);
        let node = Node::new("test_node", Rc::downgrade(&driver_runner.root_node), node_manager);
        let host = Rc::new(MockDriverHost::new());
        node.set_host(host.clone());

        let token = zx::Event::create();
        let token_remote = token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let koid = token.koid().unwrap();

        node.set_state_for_testing(driver_manager_node::types::NodeState::DriverComponent(
            driver_manager_node::types::DriverComponent::new(
                "url".to_string(),
                token_remote,
                koid,
                None,
                None,
                None,
                driver_manager_node::types::DriverState::Running,
            ),
        ));

        driver_runner.root_node.add_child_to_children_for_testing(node);

        let result = driver_runner.get_host_koid(token).await;
        assert_eq!(result, Ok(zx::Koid::from_raw(0)));

        let random_token = zx::Event::create();
        let result = driver_runner.get_host_koid(random_token).await;
        assert_eq!(result, Err(zx::Status::NOT_FOUND));
    }
}
