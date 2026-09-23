// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::host_identifier::{DefaultIdentifier, HostIdentifier, Identifier};
use anyhow::{Context as _, Result};
use component_debug::dirs::*;
use component_debug::lifecycle::*;
use fidl::endpoints::Proxy;
use fidl_fuchsia_developer_remotecontrol as rcs;
use fidl_fuchsia_developer_remotecontrol_connector as connector;
use fidl_fuchsia_diagnostics_types as diagnostics;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_io as io;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_component::client::connect_to_protocol_at_path;
use futures::channel::oneshot;
use futures::prelude::*;
use log::*;
use moniker::Moniker;
use std::borrow::Borrow;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

mod host_identifier;
pub mod http;

pub struct RemoteControlService {
    ids: RefCell<Vec<Weak<RefCell<Vec<u64>>>>>,
    id_allocator: Box<dyn Fn() -> Result<Box<dyn Identifier + 'static>>>,
    connector: Box<dyn Fn(ConnectionRequest, Weak<RemoteControlService>)>,
    toolbox_dir: RefCell<Option<fio::DirectoryProxy>>,
}

struct Client {
    // Maintain reference-counts to this client's ids.
    // The ids may be shared (e.g. when Overnet maintains two
    // connections to the target -- legacy + CSO), so we can't
    // just maintain a list of RCS's ids and remove when one
    // disappars.  Instead, when these are freed due to the client
    // being dropped, the RCS Weak references will become invalid.
    allocated_ids: Rc<RefCell<Vec<u64>>>,
}

/// Indicates a connection request to be handled by the `connector` argument of
/// `RemoteControlService::new`
pub enum ConnectionRequest {
    Overnet(fidl::Socket, oneshot::Sender<u64>),
    FDomain(fidl::Socket),
}

impl RemoteControlService {
    pub async fn new(connector: impl Fn(ConnectionRequest, Weak<Self>) + 'static) -> Self {
        // Generate a random 64-bit boot ID using Zircon's CPRNG.
        // This replaces the previous monotonic timestamp to ensure uniqueness and prevent
        // collisions, as clients of RCS were using this as a unique identifier.
        //
        // With a FIDL API change, this should probably be increased to a true 128-bit UUID
        // to further improve entropy and prevent any chance of collision.
        let mut bytes = [0u8; 8];
        zx::cprng_draw(&mut bytes);
        let boot_id = u64::from_ne_bytes(bytes);
        Self::new_with_allocator(connector, move || Ok(Box::new(HostIdentifier::new(boot_id)?)))
    }

    pub async fn new_with_default_allocator(
        connector: impl Fn(ConnectionRequest, Weak<Self>) + 'static,
    ) -> Self {
        Self::new_with_allocator(connector, || Ok(Box::new(DefaultIdentifier::new())))
    }

    pub(crate) fn new_with_allocator(
        connector: impl Fn(ConnectionRequest, Weak<Self>) + 'static,
        id_allocator: impl Fn() -> Result<Box<dyn Identifier + 'static>> + 'static,
    ) -> Self {
        Self {
            id_allocator: Box::new(id_allocator),
            ids: Default::default(),
            connector: Box::new(connector),
            toolbox_dir: RefCell::new(None),
        }
    }

    // Some of the ID-lists may be gone because old clients have shut down.
    // They will have a strong_count of 0.  Drop 'em.
    fn remove_old_ids(self: &Rc<Self>) {
        self.ids.borrow_mut().retain(|wirc| wirc.strong_count() > 0);
    }

    async fn handle_connector(
        self: &Rc<Self>,
        client: &Client,
        request: connector::ConnectorRequest,
    ) -> Result<()> {
        match request {
            connector::ConnectorRequest::EstablishCircuit { id, socket, responder } => {
                let (nodeid_sender, nodeid_receiver) = oneshot::channel();
                (self.connector)(
                    ConnectionRequest::Overnet(socket, nodeid_sender),
                    Rc::downgrade(self),
                );
                let node_id = nodeid_receiver.await?;
                client.allocated_ids.borrow_mut().push(id);
                responder.send(node_id)?;
                Ok(())
            }
            connector::ConnectorRequest::FdomainToolboxSocket { socket, responder } => {
                (self.connector)(ConnectionRequest::FDomain(socket), Rc::downgrade(self));
                responder.send()?;
                Ok(())
            }
        }
    }

    async fn handle(self: &Rc<Self>, request: rcs::RemoteControlRequest) -> Result<()> {
        match request {
            rcs::RemoteControlRequest::EchoString { value, responder } => {
                debug!("Received echo string {}", value);
                responder.send(&value)?;
                Ok(())
            }
            rcs::RemoteControlRequest::LogMessage { tag, message, severity, responder } => {
                match severity {
                    diagnostics::Severity::Trace => trace!(tag:%; "{}", message),
                    diagnostics::Severity::Debug => debug!(tag:%; "{}", message),
                    diagnostics::Severity::Info => info!(tag:%; "{}", message),
                    diagnostics::Severity::Warn => warn!(tag:%; "{}", message),
                    diagnostics::Severity::Error => error!(tag:%; "{}", message),
                    // Tracing crate doesn't have a Fatal level, just log an error with a FATAL message embedded.
                    diagnostics::Severity::Fatal => error!(tag:%; "<FATAL> {}", message),
                    diagnostics::Severity::__SourceBreaking { .. } => {
                        error!(tag:%; "<UNKNOWN> {message}")
                    }
                }
                responder.send()?;
                Ok(())
            }
            rcs::RemoteControlRequest::IdentifyHost { responder } => {
                self.clone().identify_host(responder).await?;
                Ok(())
            }
            rcs::RemoteControlRequest::ConnectCapability {
                moniker,
                capability_set,
                capability_name,
                server_channel,
                responder,
            } => {
                responder.send(
                    self.clone()
                        .open_capability(moniker, capability_set, capability_name, server_channel)
                        .await,
                )?;
                Ok(())
            }
            rcs::RemoteControlRequest::GetTime { responder } => {
                responder.send(zx::MonotonicInstant::get())?;
                Ok(())
            }
            rcs::RemoteControlRequest::GetBootTime { responder } => {
                responder.send(zx::BootInstant::get())?;
                Ok(())
            }
            rcs::RemoteControlRequest::_UnknownMethod { ordinal, .. } => {
                warn!("Received unknown request with ordinal {ordinal}");
                Ok(())
            }
        }
    }

    pub async fn serve_connector_stream(self: Rc<Self>, stream: connector::ConnectorRequestStream) {
        // When the stream ends, the client (and its ids) will drop
        let allocated_ids = Rc::new(RefCell::new(vec![]));
        self.ids.borrow_mut().push(Rc::downgrade(&allocated_ids));
        let client = Client { allocated_ids };
        stream
            .for_each_concurrent(None, |request| async {
                match request {
                    Ok(request) => {
                        let _ = self
                            .handle_connector(&client, request)
                            .await
                            .map_err(|e| warn!("stream request handling error: {:?}", e));
                    }
                    Err(e) => warn!("stream error: {:?}", e),
                }
            })
            .await;
    }

    pub async fn serve_stream(self: Rc<Self>, stream: rcs::RemoteControlRequestStream) {
        stream
            .for_each_concurrent(None, |request| async {
                match request {
                    Ok(request) => {
                        let _ = self
                            .handle(request)
                            .await
                            .map_err(|e| warn!("stream request handling error: {:?}", e));
                    }
                    Err(e) => warn!("stream error: {:?}", e),
                }
            })
            .await;
    }

    pub async fn get_host_identity(
        self: &Rc<Self>,
    ) -> Result<rcs::IdentifyHostResponse, rcs::IdentifyHostError> {
        let identifier = match (self.id_allocator)() {
            Ok(i) => i,
            Err(e) => {
                error!(e:%; "Allocating host identifier");
                return Err(rcs::IdentifyHostError::ProxyConnectionFailed);
            }
        };

        // We need to clean up the ids at some point. Let's do
        // it when those IDs are asked for.
        self.remove_old_ids();
        // Now the only vecs should be ones which are still held with a strong
        // Rc reference. Extract those.
        let ids: Vec<u64> = self
            .ids
            .borrow()
            .iter()
            .flat_map(|w| -> Vec<u64> {
                // This is all sadmac's fault. Grr. (Because he suggested, correctly, that
                // we use a Rc<Vec<_>> instead of Vec<Rc<_>>)
                <Rc<RefCell<Vec<u64>>> as Borrow<RefCell<Vec<u64>>>>::borrow(
                    &w.upgrade().expect("Didn't we just clear out refs with expired values??"),
                )
                .borrow()
                .clone()
            })
            .collect();
        let target_identity = identifier.identify().await.map(move |mut i| {
            i.ids = Some(ids);
            i
        });
        target_identity
    }

    pub async fn identify_host(
        self: &Rc<Self>,
        responder: rcs::RemoteControlIdentifyHostResponder,
    ) -> Result<()> {
        responder
            .send(self.get_host_identity().await.as_ref().map_err(|e| *e))
            .context("responding to client")?;
        Ok(())
    }

    /// Connects to a capability identified by the given moniker in the specified set of
    /// capabilities at the given capability name.
    async fn open_capability(
        self: &Rc<Self>,
        moniker: String,
        capability_set: fsys::OpenDirType,
        capability_name: String,
        server_end: zx::Channel,
    ) -> Result<(), rcs::ConnectCapabilityError> {
        // Connect to the root LifecycleController protocol
        let lifecycle = connect_to_protocol_at_path::<fsys::LifecycleControllerMarker>(
            "/svc/fuchsia.sys2.LifecycleController.root",
        )
        .map_err(|err| {
            error!(err:%; "could not connect to lifecycle controller");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })?;

        // Connect to the root RealmQuery protocol
        let query = connect_to_protocol_at_path::<fsys::RealmQueryMarker>(
            "/svc/fuchsia.sys2.RealmQuery.root",
        )
        .map_err(|err| {
            error!(err:%; "could not connect to realm query");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })?;

        let moniker = Moniker::try_from(moniker.as_str())
            .map_err(|_| rcs::ConnectCapabilityError::InvalidMoniker)?;
        connect_to_capability_at_moniker(
            moniker,
            capability_set,
            capability_name,
            server_end,
            lifecycle,
            query,
        )
        .await
    }

    /// Returns a directory proxy to `/core/toolbox` by opening it freshly from a cached
    /// namespace handle if still open and valid, or resolves and opens it via Component
    /// Manager if not yet cached or if the previous channel was closed.
    ///
    /// Caching the root namespace handle avoids repeated synchronous `LifecycleController.root`
    /// and `RealmQuery.root` IPC resolution overhead on every short-lived FDomain connection,
    /// while reopening the directory for every call ensures each caller operates on an
    /// independent channel connection.
    async fn get_or_open_toolbox_dir(
        self: &Rc<Self>,
    ) -> Result<fio::DirectoryProxy, rcs::ConnectCapabilityError> {
        {
            // Scope the immutable borrow so that `borrowed` is dropped before any
            // subsequent `borrow_mut()` can occur on cache invalidation.
            let borrowed = self.toolbox_dir.borrow();
            if let Some(dir) = borrowed.as_ref() {
                // Perform a non-blocking check on the kernel channel to immediately detect
                // if PEER_CLOSED is already asserted, without waiting or pumping async reads.
                let is_alive = match dir
                    .as_channel()
                    .as_ref()
                    .wait_one(zx::Signals::CHANNEL_PEER_CLOSED, zx::MonotonicInstant::from_nanos(0))
                    .to_result()
                {
                    Ok(_) => false,
                    Err(zx::Status::TIMED_OUT) => true,
                    Err(_) => false,
                };

                if is_alive {
                    if let Ok(reopened) =
                        fuchsia_fs::directory::open_directory_async(dir, ".", fio::PERM_READABLE)
                    {
                        return Ok(reopened);
                    }
                }
            }
        }
        *self.toolbox_dir.borrow_mut() = None;

        // Connect to the root LifecycleController protocol
        let controller = connect_to_protocol_at_path::<fsys::LifecycleControllerMarker>(
            "/svc/fuchsia.sys2.LifecycleController.root",
        )
        .map_err(|err| {
            error!(err:%; "could not connect to lifecycle controller");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })?;

        // Connect to the root RealmQuery protocol
        let query = connect_to_protocol_at_path::<fsys::RealmQueryMarker>(
            "/svc/fuchsia.sys2.RealmQuery.root",
        )
        .map_err(|err| {
            error!(err:%; "could not connect to realm query");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })?;

        // Attempt to resolve both the modern and legacy locations concurrently and use the one that
        // resolves successfully
        let moniker =
            moniker::Moniker::try_from("toolbox").expect("Moniker 'toolbox' did not parse!");
        let legacy_moniker = moniker::Moniker::try_from("core/toolbox")
            .expect("Moniker 'core/toolbox' did not parse!");
        let (modern, legacy) = futures::join!(
            resolve_instance(&controller, &moniker),
            resolve_instance(&controller, &legacy_moniker)
        );

        let moniker = if modern.is_ok() {
            moniker
        } else if legacy.is_ok() {
            legacy_moniker
        } else {
            error!("Unable to resolve toolbox component in either toolbox or core/toolbox");
            return Err(rcs::ConnectCapabilityError::NoMatchingComponent);
        };

        let dir = component_debug::dirs::open_instance_directory(
            &moniker,
            fsys::OpenDirType::NamespaceDir.into(),
            &query,
        )
        .map_err(|err| {
            error!(err:?; "error opening exposed dir");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })
        .await?;

        let reopened = fuchsia_fs::directory::open_directory_async(&dir, ".", fio::PERM_READABLE)
            .map_err(|err| {
            error!(err:?; "error reopening toolbox dir");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })?;
        *self.toolbox_dir.borrow_mut() = Some(dir);
        Ok(reopened)
    }

    pub async fn open_toolbox(
        self: &Rc<Self>,
        server_end: zx::Channel,
    ) -> Result<(), rcs::ConnectCapabilityError> {
        let dir = self.get_or_open_toolbox_dir().await?;
        dir.open("svc", io::PERM_READABLE, &Default::default(), server_end).map_err(|err| {
            error!(err:?; "error opening svc dir in toolbox");
            *self.toolbox_dir.borrow_mut() = None;
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })?;
        Ok(())
    }
}

/// Connect to the capability at the provided moniker in the specified set of capabilities under
/// the provided capability name.
async fn connect_to_capability_at_moniker(
    moniker: Moniker,
    capability_set: fsys::OpenDirType,
    capability_name: String,
    server_end: zx::Channel,
    lifecycle: fsys::LifecycleControllerProxy,
    query: fsys::RealmQueryProxy,
) -> Result<(), rcs::ConnectCapabilityError> {
    // This is a no-op if already resolved.
    resolve_instance(&lifecycle, &moniker)
        .map_err(|err| match err {
            ResolveError::ActionError(ActionError::InstanceNotFound) => {
                rcs::ConnectCapabilityError::NoMatchingComponent
            }
            err => {
                error!(err:?; "error resolving component");
                rcs::ConnectCapabilityError::CapabilityConnectFailed
            }
        })
        .await?;

    let dir = open_instance_directory(&moniker, capability_set.into(), &query)
        .map_err(|err| {
            error!(err:?; "error opening exposed dir");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        })
        .await?;

    connect_to_capability_in_dir(&dir, &capability_name, server_end).await?;
    Ok(())
}

async fn connect_to_capability_in_dir(
    dir: &io::DirectoryProxy,
    capability_name: &str,
    server_end: zx::Channel,
) -> Result<(), rcs::ConnectCapabilityError> {
    check_entry_exists(dir, capability_name).await?;
    // Connect to the capability
    dir.open(capability_name, io::Flags::PROTOCOL_SERVICE, &Default::default(), server_end).map_err(
        |err| {
            error!(err:%; "error opening capability from exposed dir");
            rcs::ConnectCapabilityError::CapabilityConnectFailed
        },
    )
}

// Checks that the given directory contains an entry with the given name.
async fn check_entry_exists(
    dir: &io::DirectoryProxy,
    capability_name: &str,
) -> Result<(), rcs::ConnectCapabilityError> {
    let dir_idx = capability_name.rfind('/');
    let (capability_name, entries) = match dir_idx {
        Some(dir_idx) => {
            let dirname = &capability_name[0..dir_idx];
            let basename = &capability_name[dir_idx + 1..];
            let nested_dir =
                fuchsia_fs::directory::open_directory(dir, dirname, fio::PERM_READABLE)
                    .await
                    .map_err(|_| rcs::ConnectCapabilityError::NoMatchingCapabilities)?;
            let entries = fuchsia_fs::directory::readdir(&nested_dir)
                .await
                .map_err(|_| rcs::ConnectCapabilityError::CapabilityConnectFailed)?;
            (basename, entries)
        }
        None => {
            let entries = fuchsia_fs::directory::readdir(dir)
                .await
                .map_err(|_| rcs::ConnectCapabilityError::CapabilityConnectFailed)?;
            (capability_name, entries)
        }
    };
    if entries.iter().any(|e| e.name == capability_name) {
        Ok(())
    } else {
        Err(rcs::ConnectCapabilityError::NoMatchingCapabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_buildinfo as buildinfo;
    use fidl_fuchsia_developer_remotecontrol as rcs;
    use fidl_fuchsia_device as fdevice;
    use fidl_fuchsia_hwinfo as hwinfo;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_net as fnet;
    use fidl_fuchsia_net_interfaces as fnet_interfaces;
    use fidl_fuchsia_sysinfo as sysinfo;
    use fuchsia_async as fasync;
    use fuchsia_component::server::ServiceFs;

    const NODENAME: &'static str = "thumb-set-human-shred";
    const BOOT_TIME: u64 = 123456789000000000;
    const SYSINFO_SERIAL: &'static str = "test_sysinfo_serial";
    const SERIAL: &'static str = "test_serial";
    const BOARD_CONFIG: &'static str = "test_board_name";
    const PRODUCT_CONFIG: &'static str = "core";

    const IPV4_ADDR: [u8; 4] = [127, 0, 0, 1];
    const IPV6_ADDR: [u8; 16] = [127, 1, 2, 3, 4, 5, 6, 7, 8, 9, 1, 2, 3, 4, 5, 6];

    fn setup_fake_device_service() -> hwinfo::DeviceProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<hwinfo::DeviceMarker>();
        fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    hwinfo::DeviceRequest::GetInfo { responder } => {
                        let _ = responder.send(&hwinfo::DeviceInfo {
                            serial_number: Some(String::from(SERIAL)),
                            ..Default::default()
                        });
                    }
                }
            }
        })
        .detach();

        proxy
    }

    fn setup_fake_sysinfo_service(status: Result<(), zx::Status>) -> sysinfo::SysInfoProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<sysinfo::SysInfoMarker>();
        fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    sysinfo::SysInfoRequest::GetSerialNumber { responder } => {
                        let _ = responder
                            .send(status.map(|_| SYSINFO_SERIAL).map_err(zx::Status::into_raw));
                    }
                    _ => panic!("unexpected request: {req:?}"),
                }
            }
        })
        .detach();

        proxy
    }

    fn setup_fake_build_info_service() -> buildinfo::ProviderProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<buildinfo::ProviderMarker>();
        fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    buildinfo::ProviderRequest::GetBuildInfo { responder } => {
                        let _ = responder.send(&buildinfo::BuildInfo {
                            board_config: Some(String::from(BOARD_CONFIG)),
                            product_config: Some(String::from(PRODUCT_CONFIG)),
                            ..Default::default()
                        });
                    }
                }
            }
        })
        .detach();

        proxy
    }

    fn setup_fake_name_provider_service() -> fdevice::NameProviderProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fdevice::NameProviderMarker>();

        fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    fdevice::NameProviderRequest::GetDeviceName { responder } => {
                        let _ = responder.send(Ok(NODENAME));
                    }
                }
            }
        })
        .detach();

        proxy
    }

    fn setup_fake_interface_state_service() -> fnet_interfaces::StateProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_interfaces::StateMarker>();

        fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    fnet_interfaces::StateRequest::GetWatcher {
                        options: _,
                        watcher,
                        control_handle: _,
                    } => {
                        let mut stream = watcher.into_stream();
                        let mut first = true;
                        while let Ok(Some(req)) = stream.try_next().await {
                            match req {
                                fnet_interfaces::WatcherRequest::Watch { responder } => {
                                    let event = if first {
                                        first = false;
                                        fnet_interfaces::Event::Existing(
                                            fnet_interfaces::Properties {
                                                id: Some(1),
                                                addresses: Some(
                                                    IntoIterator::into_iter([
                                                        fnet::Subnet {
                                                            addr: fnet::IpAddress::Ipv4(
                                                                fnet::Ipv4Address {
                                                                    addr: IPV4_ADDR,
                                                                },
                                                            ),
                                                            prefix_len: 4,
                                                        },
                                                        fnet::Subnet {
                                                            addr: fnet::IpAddress::Ipv6(
                                                                fnet::Ipv6Address {
                                                                    addr: IPV6_ADDR,
                                                                },
                                                            ),
                                                            prefix_len: 110,
                                                        },
                                                    ])
                                                    .map(Some)
                                                    .map(|addr| fnet_interfaces::Address {
                                                        addr,
                                                        assignment_state: Some(fnet_interfaces::AddressAssignmentState::Assigned),
                                                        ..Default::default()
                                                    })
                                                    .collect(),
                                                ),
                                                online: Some(true),
                                                port_class: Some(
                                                    fnet_interfaces::PortClass::Loopback(
                                                        fnet_interfaces::Empty {},
                                                    ),
                                                ),
                                                has_default_ipv4_route: Some(false),
                                                has_default_ipv6_route: Some(false),
                                                name: Some(String::from("eth0")),
                                                ..Default::default()
                                            },
                                        )
                                    } else {
                                        fnet_interfaces::Event::Idle(fnet_interfaces::Empty {})
                                    };
                                    let () = responder.send(&event).unwrap();
                                }
                            }
                        }
                    }
                }
            }
        })
        .detach();

        proxy
    }

    #[derive(Default)]
    #[non_exhaustive]
    struct RcsEnv {
        system_info_proxy: Option<sysinfo::SysInfoProxy>,
        use_default_identifier: bool,
    }

    fn make_rcs_from_env(env: RcsEnv) -> Rc<RemoteControlService> {
        let RcsEnv { system_info_proxy, use_default_identifier } = env;
        if use_default_identifier {
            Rc::new(RemoteControlService::new_with_allocator(
                |req, _| match req {
                    ConnectionRequest::Overnet(_, sender) => sender.send(0u64).unwrap(),
                    _ => (),
                },
                move || Ok(Box::new(DefaultIdentifier { boot_timestamp_nanos: BOOT_TIME })),
            ))
        } else {
            Rc::new(RemoteControlService::new_with_allocator(
                |req, _| match req {
                    ConnectionRequest::Overnet(_, sender) => sender.send(0u64).unwrap(),
                    _ => (),
                },
                move || {
                    Ok(Box::new(HostIdentifier {
                        interface_state_proxy: setup_fake_interface_state_service(),
                        name_provider_proxy: setup_fake_name_provider_service(),
                        device_info_proxy: setup_fake_device_service(),
                        system_info_proxy: system_info_proxy.clone().unwrap_or_else(|| {
                            setup_fake_sysinfo_service(Err(zx::Status::INTERNAL))
                        }),
                        build_info_proxy: setup_fake_build_info_service(),
                        boot_timestamp_nanos: BOOT_TIME,
                        boot_id: 0,
                    }))
                },
            ))
        }
    }

    fn setup_rcs_proxy_from_env(
        env: RcsEnv,
    ) -> (rcs::RemoteControlProxy, connector::ConnectorProxy) {
        let service = make_rcs_from_env(env);

        let (rcs_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<rcs::RemoteControlMarker>();
        fasync::Task::local({
            let service = Rc::clone(&service);
            async move {
                service.serve_stream(stream).await;
            }
        })
        .detach();
        let (connector_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<connector::ConnectorMarker>();
        fasync::Task::local(async move {
            service.serve_connector_stream(stream).await;
        })
        .detach();

        (rcs_proxy, connector_proxy)
    }

    fn setup_rcs_proxy() -> rcs::RemoteControlProxy {
        setup_rcs_proxy_from_env(Default::default()).0
    }

    fn setup_rcs_proxy_with_connector() -> (rcs::RemoteControlProxy, connector::ConnectorProxy) {
        setup_rcs_proxy_from_env(Default::default())
    }

    fn setup_fake_lifecycle_controller() -> fsys::LifecycleControllerProxy {
        fidl_test_util::spawn_stream_handler(
            move |request: fsys::LifecycleControllerRequest| async move {
                match request {
                    fsys::LifecycleControllerRequest::ResolveInstance { moniker, responder } => {
                        assert_eq!(moniker, "core/my_component");
                        responder.send(Ok(())).unwrap()
                    }
                    _ => panic!("unexpected request: {:?}", request),
                }
            },
        )
    }

    fn setup_exposed_dir(server: ServerEnd<fio::DirectoryMarker>) {
        let mut fs = ServiceFs::new();
        fs.add_fidl_service(move |_: hwinfo::BoardRequestStream| {});
        fs.dir("svc").add_fidl_service(move |_: hwinfo::BoardRequestStream| {});
        fs.serve_connection(server).unwrap();
        fasync::Task::spawn(fs.collect::<()>()).detach();
    }

    /// Set up a fake realm query which asserts a requests coming in have the
    /// right options set, including which of a component's capability sets
    /// (ie. incoming namespace, outgoing directory, etc) the capability is
    /// expected to be requested from.
    fn setup_fake_realm_query(capability_set: fsys::OpenDirType) -> fsys::RealmQueryProxy {
        fidl_test_util::spawn_stream_handler(move |request: fsys::RealmQueryRequest| async move {
            match request {
                fsys::RealmQueryRequest::OpenDirectory { moniker, dir_type, object, responder } => {
                    assert_eq!(moniker, "core/my_component");
                    assert_eq!(dir_type, capability_set);
                    setup_exposed_dir(object);
                    responder.send(Ok(())).unwrap()
                }
                _ => panic!("unexpected request: {:?}", request),
            }
        })
    }

    #[fuchsia::test]
    async fn test_connect_to_component_capability() -> Result<()> {
        for dir_type in vec![
            fsys::OpenDirType::ExposedDir,
            fsys::OpenDirType::NamespaceDir,
            fsys::OpenDirType::OutgoingDir,
        ] {
            let (_client, server) = zx::Channel::create();
            let lifecycle = setup_fake_lifecycle_controller();
            let query = setup_fake_realm_query(dir_type);
            connect_to_capability_at_moniker(
                Moniker::try_from("./core/my_component").unwrap(),
                dir_type,
                "fuchsia.hwinfo.Board".to_string(),
                server,
                lifecycle,
                query,
            )
            .await
            .unwrap();
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_connect_to_component_capability_in_subdirectory() -> Result<()> {
        for dir_type in vec![
            fsys::OpenDirType::ExposedDir,
            fsys::OpenDirType::NamespaceDir,
            fsys::OpenDirType::OutgoingDir,
        ] {
            let (_client, server) = zx::Channel::create();
            let lifecycle = setup_fake_lifecycle_controller();
            let query = setup_fake_realm_query(dir_type);
            connect_to_capability_at_moniker(
                Moniker::try_from("./core/my_component").unwrap(),
                dir_type,
                "svc/fuchsia.hwinfo.Board".to_string(),
                server,
                lifecycle,
                query,
            )
            .await
            .unwrap();
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_connect_to_capability_not_available() -> Result<()> {
        for dir_type in vec![
            fsys::OpenDirType::ExposedDir,
            fsys::OpenDirType::NamespaceDir,
            fsys::OpenDirType::OutgoingDir,
        ] {
            let (_client, server) = zx::Channel::create();
            let lifecycle = setup_fake_lifecycle_controller();
            let query = setup_fake_realm_query(dir_type);
            let error = connect_to_capability_at_moniker(
                Moniker::try_from("./core/my_component").unwrap(),
                dir_type,
                "fuchsia.not.exposed".to_string(),
                server,
                lifecycle,
                query,
            )
            .await
            .unwrap_err();
            assert_eq!(error, rcs::ConnectCapabilityError::NoMatchingCapabilities);
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_connect_to_capability_not_available_in_subdirectory() -> Result<()> {
        for dir_type in vec![
            fsys::OpenDirType::ExposedDir,
            fsys::OpenDirType::NamespaceDir,
            fsys::OpenDirType::OutgoingDir,
        ] {
            let (_client, server) = zx::Channel::create();
            let lifecycle = setup_fake_lifecycle_controller();
            let query = setup_fake_realm_query(dir_type);
            let error = connect_to_capability_at_moniker(
                Moniker::try_from("./core/my_component").unwrap(),
                dir_type,
                "svc/fuchsia.not.exposed".to_string(),
                server,
                lifecycle,
                query,
            )
            .await
            .unwrap_err();
            assert_eq!(error, rcs::ConnectCapabilityError::NoMatchingCapabilities);
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_identify_host() -> Result<()> {
        let rcs_proxy = setup_rcs_proxy();

        let resp = rcs_proxy.identify_host().await.unwrap().unwrap();

        assert_eq!(resp.serial_number.unwrap(), SERIAL);
        assert_eq!(resp.board_config.unwrap(), BOARD_CONFIG);
        assert_eq!(resp.product_config.unwrap(), PRODUCT_CONFIG);
        assert_eq!(resp.nodename.unwrap(), NODENAME);

        let addrs = resp.addresses.unwrap();
        assert_eq!(
            addrs[..],
            [
                fnet::Subnet {
                    addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: IPV4_ADDR }),
                    prefix_len: 4,
                },
                fnet::Subnet {
                    addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: IPV6_ADDR }),
                    prefix_len: 110,
                }
            ]
        );

        assert_eq!(resp.boot_timestamp_nanos.unwrap(), BOOT_TIME);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_identify_host_sysinfo_serial() -> Result<()> {
        let (rcs_proxy, _) = setup_rcs_proxy_from_env(RcsEnv {
            system_info_proxy: Some(setup_fake_sysinfo_service(Ok(()))),
            ..Default::default()
        });

        let resp = rcs_proxy.identify_host().await.unwrap().unwrap();

        assert_eq!(resp.serial_number.unwrap(), SYSINFO_SERIAL);
        assert_eq!(resp.board_config.unwrap(), BOARD_CONFIG);
        assert_eq!(resp.product_config.unwrap(), PRODUCT_CONFIG);
        assert_eq!(resp.nodename.unwrap(), NODENAME);

        let addrs = resp.addresses.unwrap();
        assert_eq!(
            addrs[..],
            [
                fnet::Subnet {
                    addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: IPV4_ADDR }),
                    prefix_len: 4,
                },
                fnet::Subnet {
                    addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: IPV6_ADDR }),
                    prefix_len: 110,
                }
            ]
        );

        assert_eq!(resp.boot_timestamp_nanos.unwrap(), BOOT_TIME);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_ids_in_host_identify() -> Result<()> {
        let (rcs_proxy, connector_proxy) = setup_rcs_proxy_with_connector();

        let ident = rcs_proxy.identify_host().await.unwrap().unwrap();
        assert_eq!(ident.ids, Some(vec![]));

        let (pumpkin_a, _) = fidl::Socket::create_stream();
        let (pumpkin_b, _) = fidl::Socket::create_stream();
        let _node_ida = connector_proxy.establish_circuit(1234, pumpkin_a).await.unwrap();
        let _node_idb = connector_proxy.establish_circuit(4567, pumpkin_b).await.unwrap();

        let ident = rcs_proxy.identify_host().await.unwrap().unwrap();
        let ids = ident.ids.unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(1234u64, ids[0]);
        assert_eq!(4567u64, ids[1]);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_identify_default() -> Result<()> {
        let (rcs_proxy, _) =
            setup_rcs_proxy_from_env(RcsEnv { use_default_identifier: true, ..Default::default() });

        let resp = rcs_proxy.identify_host().await.unwrap().unwrap();

        assert_eq!(resp.nodename.unwrap(), "fuchsia-default-nodename");
        assert_eq!(resp.serial_number.unwrap(), "fuchsia-default-serial-number");
        assert_eq!(resp.board_config, None);
        assert_eq!(resp.product_config, None);
        assert_eq!(resp.addresses, None);
        assert_eq!(resp.boot_timestamp_nanos.unwrap(), BOOT_TIME);

        Ok(())
    }
}
