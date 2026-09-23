// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtin_devices::BuiltinDevVnode;
use crate::class_names::{
    CLASS_NAME_TO_SERVICE, CLASSES_THAT_ALLOW_TOPOLOGICAL_PATH, CLASSES_THAT_ASSUME_ORDERING, State,
};
use driver_manager_types::StartRequestReceiver;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_device as fdevice;
use fidl_fuchsia_device_fs as fdevfs;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::channel::mpsc;
use log::{debug, error, warn};
use rand::{RngExt as _, SeedableRng as _};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::Directory;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::simple::Simple;
use vfs::execution_scope::ExecutionScope;
use vfs::path::Path;
use vfs::remote::RemoteLike;
use vfs::service::endpoint;
use vfs::{ObjectRequestRef, pseudo_directory};

mod builtin_devices;
mod class_names;

pub enum ConnectorMsg {
    Controller(ServerEnd<fdevice::ControllerMarker>),
    Protocol(zx::Channel),
}

pub type Connector = mpsc::UnboundedSender<ConnectorMsg>;

pub enum OutgoingDirectoryMsg {
    Connect(ServerEnd<fio::DirectoryMarker>),
    AddServiceInstance(String, String, fio::DirectoryProxy),
}

pub type OutgoingDirectory = mpsc::UnboundedSender<OutgoingDirectoryMsg>;

struct PathServer {
    path: String,
    scope: fasync::ScopeHandle,
}

impl PathServer {
    fn new(path: String, scope: fasync::ScopeHandle) -> Self {
        Self { path, scope }
    }

    fn serve(&self, channel: zx::Channel, class_name: &str) {
        if !CLASSES_THAT_ALLOW_TOPOLOGICAL_PATH.contains(class_name) {
            error!(
                "Access to the topological path channel is not permitted for class {}",
                class_name
            );
            return;
        }
        let mut stream = ServerEnd::<fdevfs::TopologicalPathMarker>::new(channel).into_stream();
        let path = self.path.clone();
        self.scope.spawn_local(async move {
            while let Some(Ok(msg)) = stream.next().await {
                match msg {
                    fdevfs::TopologicalPathRequest::GetTopologicalPath { responder } => {
                        let _ = responder.send(Ok(&path));
                    }
                }
            }
        });
    }
}

struct ServiceInfo {
    parent_dir: Arc<Simple>,
    member_name: String,
}

pub struct Devnode {
    devfs: Weak<Devfs>,
    parent: Weak<Simple>,
    vnode: Arc<DevnodeVnode>,
    name: String,
    path_server: PathServer,
    children: Arc<Simple>,
    service_info: Mutex<Option<ServiceInfo>>,
    #[allow(unused)]
    scope: fasync::Scope,
}

impl Drop for Devnode {
    fn drop(&mut self) {
        let mut service_info = self.service_info.lock();
        if let Some(info) = service_info.take() {
            // The second argument to remove_entry is whether the entry is a directory.
            // We are removing a service, which is not a directory.
            let _ = info.parent_dir.remove_entry(&info.member_name, false);
            let _ = info.parent_dir.remove_entry(fdevfs::DEVICE_TOPOLOGY_NAME, false);
        }
        if let Some(parent_dir) = self.parent.upgrade() {
            let _ = parent_dir.remove_entry(self.name.clone(), false);
        }
    }
}

impl Devnode {
    fn new(
        devfs: Weak<Devfs>,
        parent: Weak<Simple>,
        connector: Connector,
        name: String,
        path: &str,
        class_name: &str,
    ) -> Arc<Self> {
        let scope = fasync::Scope::new_with_name("path_server");

        let this = Arc::new_cyclic(|weak_self: &Weak<Devnode>| {
            let children = Simple::new();
            let vnode =
                Arc::new(DevnodeVnode { connector: Some(connector), children: children.clone() });

            let this = Self {
                devfs,
                parent,
                vnode,
                name,
                path_server: PathServer::new(path.to_string(), scope.as_handle().clone()),
                children,
                service_info: Mutex::new(None),
                scope,
            };

            let weak_self2 = weak_self.clone();
            this.children
                .add_entry(
                    fdevfs::DEVICE_CONTROLLER_NAME,
                    endpoint(move |_, channel| {
                        if let Some(this) = weak_self2.upgrade() {
                            let _ = this.vnode.connector.as_ref().unwrap().unbounded_send(
                                ConnectorMsg::Controller(channel.into_zx_channel().into()),
                            );
                        }
                    }),
                )
                .unwrap();

            let weak_self2 = weak_self.clone();
            this.children
                .add_entry(
                    fdevfs::DEVICE_PROTOCOL_NAME,
                    endpoint(move |_, channel| {
                        if let Some(this) = weak_self2.upgrade() {
                            let _ = this
                                .vnode
                                .connector
                                .as_ref()
                                .unwrap()
                                .unbounded_send(ConnectorMsg::Protocol(channel.into()));
                        }
                    }),
                )
                .unwrap();

            let class_name_clone = class_name.to_string();
            let weak_self = weak_self.clone();
            this.children
                .add_entry(
                    fdevfs::DEVICE_TOPOLOGY_NAME,
                    endpoint(move |_, channel| {
                        if let Some(this) = weak_self.upgrade() {
                            this.path_server.serve(channel.into(), &class_name_clone);
                        }
                    }),
                )
                .unwrap();

            this
        });

        if let Some(parent) = this.parent.upgrade() {
            parent.add_entry(this.name.clone(), this.vnode.clone()).unwrap();
        }
        this
    }

    fn try_add_service(
        self: &Arc<Self>,
        class_name: &str,
        connector: Connector,
        instance_name: &str,
    ) -> Result<(), zx::Status> {
        let service_entry = if let Some(entry) = CLASS_NAME_TO_SERVICE.get(class_name) {
            entry
        } else {
            return Ok(());
        };

        let devfs = self.devfs.upgrade().ok_or(zx::Status::BAD_STATE)?;

        let instance_dir = Simple::new();
        let handler = endpoint(move |_, channel| {
            let _ = connector.unbounded_send(ConnectorMsg::Protocol(channel.into()));
        });

        let full_path = format!(
            "svc/{}/{}/{}",
            service_entry.service_name, instance_name, service_entry.member_name
        );
        if let Err(e) = instance_dir.add_entry(service_entry.member_name, handler) {
            warn!("Failed to add service entry '{}' for class '{}': {}", full_path, class_name, e);
            return Err(e);
        }
        debug!("Added service entry '{}' for class '{}'", full_path, class_name);

        *self.service_info.lock() = Some(ServiceInfo {
            parent_dir: instance_dir.clone(),
            member_name: service_entry.member_name.to_string(),
        });

        // Add topological path service
        let weak_self = Arc::downgrade(self);
        let class_name_clone = class_name.to_string();
        let topo_handler = endpoint(move |_, channel| {
            if let Some(this) = weak_self.upgrade() {
                this.path_server.serve(channel.into(), &class_name_clone);
            }
        });
        let topo_full_path = format!("{}/{}", instance_name, fdevfs::DEVICE_TOPOLOGY_NAME);
        if let Err(e) = instance_dir.add_entry(fdevfs::DEVICE_TOPOLOGY_NAME, topo_handler) {
            warn!(
                "Failed to add topological path service entry '{}' for class '{}': {}",
                topo_full_path, class_name, e
            );
            let _ = instance_dir.remove_entry(service_entry.member_name, false);
            return Err(e);
        }

        let instance_dir = vfs::directory::serve(
            instance_dir,
            ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        if let Err(e) = devfs.outgoing.unbounded_send(OutgoingDirectoryMsg::AddServiceInstance(
            service_entry.service_name.to_string(),
            instance_name.to_string(),
            instance_dir,
        )) {
            warn!(
                "Failed to add instance to outgoing directory '{}' for class '{}': {}",
                topo_full_path, class_name, e
            );
            return Err(zx::Status::BAD_STATE);
        }

        Ok(())
    }

    fn add_child(
        self: &Arc<Self>,
        name: &str,
        class_name: Option<&str>,
        connector: Connector,
    ) -> Result<DevfsDevice, zx::Status> {
        if self.children.get_entry(name).is_ok() {
            warn!("rejecting duplicate device name '{}'", name);
            return Err(zx::Status::ALREADY_EXISTS);
        }

        let mut child = DevfsDevice::new();
        let child_path = format!("{}/{}", self.path_server.path, name);
        if let Some(class_name) = class_name
            && let Some(service_entry) = CLASS_NAME_TO_SERVICE.get(class_name)
        {
            let instance_name = self.devfs.upgrade().unwrap().make_instance_name(class_name)?;
            if matches!(service_entry.state, State::Devfs | State::DevfsAndService) {
                let devfs = self.devfs.upgrade().unwrap();
                let class_dir = devfs.class_dirs.get(class_name).unwrap();
                child.protocol = Some(Devnode::new(
                    self.devfs.clone(),
                    Arc::downgrade(class_dir),
                    connector.clone(),
                    instance_name.clone(),
                    &child_path,
                    class_name,
                ));
            }

            if matches!(service_entry.state, State::DevfsAndService) {
                let protocol_node = child.protocol.as_ref().unwrap().clone();
                protocol_node.try_add_service(class_name, connector.clone(), &instance_name)?;
            }
        }

        child.topological = Some(TopologicalDevnode::Devnode(Devnode::new(
            self.devfs.clone(),
            Arc::downgrade(&self.children),
            connector,
            name.to_string(),
            &child_path,
            class_name.unwrap_or(""),
        )));

        Ok(child)
    }
}

struct DevnodeVnode {
    connector: Option<Connector>,
    children: Arc<Simple>,
}

impl RemoteLike for DevnodeVnode {
    fn open(
        self: Arc<DevnodeVnode>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        if flags.contains(fio::Flags::PROTOCOL_DIRECTORY)
            || flags.contains(fio::Flags::PROTOCOL_NODE)
            || !path.is_empty()
        {
            self.children.clone().open(scope, path, flags, object_request)
        } else if let Some(ref connector) = self.connector
            && flags.contains(fio::Flags::PROTOCOL_SERVICE)
        {
            // This is a connection to the device itself.
            let _ = connector
                .unbounded_send(ConnectorMsg::Protocol(object_request.take().into_channel()));
            Ok(())
        } else {
            Err(zx::Status::NOT_FOUND)
        }
    }
}

impl DirectoryEntry for DevnodeVnode {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_remote(self)
    }
}

impl GetEntryInfo for DevnodeVnode {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

#[derive(Clone)]
pub struct RootDevnode {
    children: Arc<Simple>,
    devfs: Weak<Devfs>,
}

impl RootDevnode {
    fn add_child(
        &self,
        name: &str,
        class_name: Option<&str>,
        connector: Connector,
    ) -> Result<DevfsDevice, zx::Status> {
        let devfs = self.devfs.upgrade().unwrap();

        let mut child = DevfsDevice::new();
        let child_path = format!("/{}", name);
        if let Some(class_name) = class_name
            && let Some(service_entry) = CLASS_NAME_TO_SERVICE.get(class_name)
        {
            let instance_name = devfs.make_instance_name(class_name)?;
            if matches!(service_entry.state, State::Devfs | State::DevfsAndService) {
                let class_dir = devfs.class_dirs.get(class_name).unwrap();
                child.protocol = Some(Devnode::new(
                    self.devfs.clone(),
                    Arc::downgrade(class_dir),
                    connector.clone(),
                    instance_name.clone(),
                    &child_path,
                    class_name,
                ));
            }

            if matches!(service_entry.state, State::DevfsAndService) {
                let protocol_node = child.protocol.as_ref().unwrap().clone();
                protocol_node.try_add_service(class_name, connector.clone(), &instance_name)?;
            }
        }

        child.topological = Some(TopologicalDevnode::Devnode(Devnode::new(
            self.devfs.clone(),
            Arc::downgrade(&self.children),
            connector,
            name.to_string(),
            &child_path,
            class_name.unwrap_or(""),
        )));

        Ok(child)
    }
}

#[derive(Clone)]
pub enum TopologicalDevnode {
    Root(RootDevnode),
    Devnode(Arc<Devnode>),
}

impl TopologicalDevnode {
    pub fn add_child(
        &self,
        name: &str,
        class_name: Option<&str>,
        connector: Connector,
    ) -> Result<DevfsDevice, zx::Status> {
        match self {
            Self::Root(devnode) => devnode.add_child(name, class_name, connector),
            Self::Devnode(devnode) => devnode.add_child(name, class_name, connector),
        }
    }
}

#[derive(Clone)]
pub struct DevfsDevice {
    pub topological: Option<TopologicalDevnode>,
    pub protocol: Option<Arc<Devnode>>,
}

impl DevfsDevice {
    pub fn new() -> Self {
        Self { topological: None, protocol: None }
    }
}

impl Default for DevfsDevice {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Devfs {
    root: Arc<Simple>,
    class_dirs: HashMap<String, Arc<Simple>>,
    device_number_generator: Mutex<rand::rngs::StdRng>,
    classes_that_assume_ordering: Mutex<HashMap<String, u32>>,
    outgoing: OutgoingDirectory,
    component_controller_proxy: Mutex<Option<fidl_fuchsia_component::ControllerProxy>>,
}

impl Devfs {
    pub fn new(outgoing: OutgoingDirectory) -> Arc<Self> {
        let class = Simple::new();
        let root = pseudo_directory!(
            "class" => class.clone(),
            "null" => BuiltinDevVnode::new(true),
            "zero" => BuiltinDevVnode::new(false),
            "builtin" => pseudo_directory!(
                "null" => BuiltinDevVnode::new(true),
                "zero" => BuiltinDevVnode::new(false),
            ),
        );

        let mut class_dirs = HashMap::new();
        for (class_name, _) in &CLASS_NAME_TO_SERVICE {
            let dir = Simple::new();
            class.add_entry(*class_name, dir.clone()).unwrap();
            class_dirs.insert(class_name.to_string(), dir);
        }

        let mut ordering = HashMap::new();
        for class_name in CLASSES_THAT_ASSUME_ORDERING.iter() {
            ordering.insert(class_name.to_string(), 0);
        }

        Arc::new(Self {
            root,
            class_dirs,
            device_number_generator: Mutex::new(rand::rngs::StdRng::from_seed(Default::default())),
            classes_that_assume_ordering: Mutex::new(ordering),
            outgoing,
            component_controller_proxy: Mutex::new(None),
        })
    }

    pub fn root_node(self: &Arc<Self>) -> TopologicalDevnode {
        TopologicalDevnode::Root(RootDevnode {
            children: self.root.clone(),
            devfs: Arc::downgrade(self),
        })
    }

    pub fn serve(&self) -> fio::DirectoryProxy {
        vfs::directory::serve(
            self.root.clone(),
            ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
    }

    pub fn set_component_controller_proxy(
        &self,
        controller: fidl_fuchsia_component::ControllerProxy,
    ) {
        let mut proxy = self.component_controller_proxy.lock();
        *proxy = Some(controller);
    }

    pub async fn send_start_request(
        &self,
        handle: fidl_fuchsia_process::HandleInfo,
    ) -> Result<(), zx::Status> {
        let start_child_args = fidl_fuchsia_component::StartChildArgs {
            numbered_handles: Some(vec![handle]),
            ..Default::default()
        };
        let (_, server_end) = fidl::endpoints::create_endpoints();

        let proxy = self.component_controller_proxy.lock().clone().ok_or_else(|| {
            error!("no component controller proxy");
            zx::Status::INTERNAL
        })?;

        proxy
            .start(start_child_args, server_end)
            .await
            .map_err(|e| {
                error!("Failed to start driver for devfs: {}", e);
                zx::Status::INTERNAL
            })?
            .map_err(|e| {
                error!("Failed to start driver for devfs: {:?}", e);
                zx::Status::INTERNAL
            })?;
        Ok(())
    }

    pub async fn attach_component(
        &self,
        handle: fidl_fuchsia_process::HandleInfo,
        mut receiver: StartRequestReceiver,
    ) -> Result<(), zx::Status> {
        self.send_start_request(handle).await?;
        let start_request = receiver.next().await.ok_or(zx::Status::TIMED_OUT)??;
        let start_info = start_request.info;
        let controller = start_request.controller;

        if let Some(outgoing_dir) = start_info.outgoing_dir {
            let _ = self.outgoing.unbounded_send(OutgoingDirectoryMsg::Connect(outgoing_dir));
        } else {
            warn!("No outgoing dir available for devfs component.");
        }
        fasync::Task::local(async move {
            let (mut stream, handle) = controller.into_stream_and_control_handle();
            if let Some(Ok(_)) = stream.next().await {
                let _ = handle
                    .send_on_stop(fidl_fuchsia_component_runner::ComponentStopInfo {
                        ..Default::default()
                    })
                    .inspect_err(|e| {
                        error!("Failed to stop driver for devfs: {}", e);
                    });
            }
        })
        .detach();
        Ok(())
    }

    pub fn make_instance_name(&self, class_name: &str) -> Result<String, zx::Status> {
        if !CLASS_NAME_TO_SERVICE.contains_key(class_name) {
            return Err(zx::Status::NOT_FOUND);
        }
        if let Some(next_id) = self.classes_that_assume_ordering.lock().get_mut(class_name) {
            let id = *next_id;
            *next_id += 1;
            Ok(format!("{:03}", id))
        } else {
            let mut rng = self.device_number_generator.lock();
            Ok(format!("{}", rng.random::<u32>()))
        }
    }
}
