// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs, unreachable_patterns)]

//! Netemul utilities.

/// Methods for creating and interacting with virtualized guests in netemul tests.
pub mod guest;

use fuchsia_sync::Mutex;
use std::borrow::Cow;
use std::collections::HashSet;
use std::num::NonZeroU64;
use std::ops::DerefMut as _;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use zx::AsHandleRef;

use fidl::endpoints::{ProtocolMarker, Proxy as _};
use fidl_fuchsia_net_dhcp_ext::{self as fnet_dhcp_ext, ClientProviderExt};
use fidl_fuchsia_net_ext::{self as fnet_ext};
use fidl_fuchsia_net_interfaces_ext::admin::Control;
use fidl_fuchsia_net_interfaces_ext::{self as fnet_interfaces_ext};
use fnet_ext::{FromExt as _, IntoExt as _};

use fidl_fuchsia_hardware_network as fnetwork;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_neighbor as fnet_neighbor;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_stack as fnet_stack;
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_netemul_network as fnetemul_network;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_ext as fposix_socket_ext;
use fidl_fuchsia_posix_socket_packet as fposix_socket_packet;
use fidl_fuchsia_posix_socket_raw as fposix_socket_raw;

use anyhow::{Context as _, anyhow};
use futures::future::{FutureExt as _, LocalBoxFuture, TryFutureExt as _};
use futures::{SinkExt as _, TryStreamExt as _};
use net_types::SpecifiedAddr;
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6, Subnet};

type Result<T = ()> = std::result::Result<T, anyhow::Error>;

/// The default MTU used in netemul endpoint configurations.
pub const DEFAULT_MTU: u16 = 1500;

/// The devfs path at which endpoints show up.
pub const NETDEVICE_DEVFS_PATH: &'static str = "class/network";

/// Returns the full path for a device node `node_name` relative to devfs root.
pub fn devfs_device_path(node_name: &str) -> std::path::PathBuf {
    std::path::Path::new(NETDEVICE_DEVFS_PATH).join(node_name)
}

/// Creates a common netemul endpoint configuration for tests.
pub fn new_endpoint_config(
    mtu: u16,
    mac: Option<fnet::MacAddress>,
) -> fnetemul_network::EndpointConfig {
    fnetemul_network::EndpointConfig {
        mtu,
        mac: mac.map(Box::new),
        port_class: fnetwork::PortClass::Virtual,
        checksum_offload: false,
    }
}

/// A test sandbox backed by a [`fnetemul::SandboxProxy`].
///
/// `TestSandbox` provides various utility methods to set up network realms for
/// use in testing. The lifetime of the `TestSandbox` is tied to the netemul
/// sandbox itself, dropping it will cause all the created realms, networks, and
/// endpoints to be destroyed.
#[must_use]
pub struct TestSandbox {
    sandbox: fnetemul::SandboxProxy,
}

impl TestSandbox {
    /// Creates a new empty sandbox.
    pub fn new() -> Result<TestSandbox> {
        fuchsia_component::client::connect_to_protocol::<fnetemul::SandboxMarker>()
            .context("failed to connect to sandbox protocol")
            .map(|sandbox| TestSandbox { sandbox })
    }

    /// Creates a realm with `name` and `children`.
    pub fn create_realm<'a, I>(
        &'a self,
        name: impl Into<Cow<'a, str>>,
        children: I,
    ) -> Result<TestRealm<'a>>
    where
        I: IntoIterator,
        I::Item: Into<fnetemul::ChildDef>,
    {
        let (realm, server) = fidl::endpoints::create_proxy::<fnetemul::ManagedRealmMarker>();
        let name = name.into();
        self.sandbox.create_realm(
            server,
            fnetemul::RealmOptions {
                name: Some(name.clone().into_owned()),
                children: Some(children.into_iter().map(Into::into).collect()),
                ..Default::default()
            },
        )?;
        Ok(TestRealm(Arc::new(TestRealmInner {
            realm,
            name,
            _sandbox: self,
            shutdown_on_drop: Mutex::new(ShutdownOnDropConfig {
                enabled: true,
                ignore_monikers: HashSet::new(),
            }),
        })))
    }

    /// Creates a realm with no components.
    pub fn create_empty_realm<'a>(
        &'a self,
        name: impl Into<Cow<'a, str>>,
    ) -> Result<TestRealm<'a>> {
        self.create_realm(name, std::iter::empty::<fnetemul::ChildDef>())
    }

    /// Connects to the sandbox's `NetworkContext`.
    fn get_network_context(&self) -> Result<fnetemul_network::NetworkContextProxy> {
        let (ctx, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkContextMarker>();
        self.sandbox.get_network_context(server)?;
        Ok(ctx)
    }

    /// Connects to the sandbox's `NetworkManager`.
    pub fn get_network_manager(&self) -> Result<fnetemul_network::NetworkManagerProxy> {
        let ctx = self.get_network_context()?;
        let (network_manager, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkManagerMarker>();
        ctx.get_network_manager(server)?;
        Ok(network_manager)
    }

    /// Connects to the sandbox's `EndpointManager`.
    pub fn get_endpoint_manager(&self) -> Result<fnetemul_network::EndpointManagerProxy> {
        let ctx = self.get_network_context()?;
        let (ep_manager, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::EndpointManagerMarker>();
        ctx.get_endpoint_manager(server)?;
        Ok(ep_manager)
    }

    /// Creates a new empty network with default configurations and `name`.
    pub async fn create_network<'a>(
        &'a self,
        name: impl Into<Cow<'a, str>>,
    ) -> Result<TestNetwork<'a>> {
        let name = name.into();
        let netm = self.get_network_manager()?;
        let (status, network) = netm
            .create_network(
                &name,
                &fnetemul_network::NetworkConfig {
                    latency: None,
                    packet_loss: None,
                    reorder: None,
                    ..Default::default()
                },
            )
            .await
            .context("create_network FIDL error")?;
        zx::Status::ok(status).context("create_network failed")?;
        let network = network
            .ok_or_else(|| anyhow::anyhow!("create_network didn't return a valid network"))?
            .into_proxy();
        Ok(TestNetwork { network, name, sandbox: self })
    }

    /// Creates new networks and endpoints as specified in `networks`.
    pub async fn setup_networks<'a>(
        &'a self,
        networks: Vec<fnetemul_network::NetworkSetup>,
    ) -> Result<TestNetworkSetup<'a>> {
        let ctx = self.get_network_context()?;
        let (status, handle) = ctx.setup(&networks).await.context("setup FIDL error")?;
        zx::Status::ok(status).context("setup failed")?;
        let handle = handle
            .ok_or_else(|| anyhow::anyhow!("setup didn't return a valid handle"))?
            .into_proxy();
        Ok(TestNetworkSetup { _setup: handle, _sandbox: self })
    }

    /// Creates a new unattached endpoint with default configurations and `name`.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    pub async fn create_endpoint<'a, S>(&'a self, name: S) -> Result<TestEndpoint<'a>>
    where
        S: Into<Cow<'a, str>>,
    {
        self.create_endpoint_with(name, new_endpoint_config(DEFAULT_MTU, None)).await
    }

    /// Creates a new unattached endpoint with the provided configuration.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    pub async fn create_endpoint_with<'a>(
        &'a self,
        name: impl Into<Cow<'a, str>>,
        config: fnetemul_network::EndpointConfig,
    ) -> Result<TestEndpoint<'a>> {
        let name = name.into();
        let epm = self.get_endpoint_manager()?;
        let (status, endpoint) =
            epm.create_endpoint(&name, &config).await.context("create_endpoint FIDL error")?;
        zx::Status::ok(status).context("create_endpoint failed")?;
        let endpoint = endpoint
            .ok_or_else(|| anyhow::anyhow!("create_endpoint didn't return a valid endpoint"))?
            .into_proxy();
        Ok(TestEndpoint { endpoint, name, _sandbox: self })
    }
}

/// A set of virtual networks and endpoints.
///
/// Created through [`TestSandbox::setup_networks`].
#[must_use]
pub struct TestNetworkSetup<'a> {
    _setup: fnetemul_network::SetupHandleProxy,
    _sandbox: &'a TestSandbox,
}

impl TestNetworkSetup<'_> {
    /// Extracts the proxy to the backing setup handle.
    ///
    /// Note that this defeats the lifetime semantics that ensure the sandbox in
    /// which these networks were created lives as long as the networks. The caller
    /// of [`TestNetworkSetup::into_proxy`] is responsible for ensuring that the
    /// sandbox outlives the networks.
    pub fn into_proxy(self) -> fnetemul_network::SetupHandleProxy {
        let Self { _setup, _sandbox: _ } = self;
        _setup
    }
}

/// [`TestInterface`] configuration.
#[derive(Default)]
pub struct InterfaceConfig<'a> {
    /// Optional interface name.
    pub name: Option<Cow<'a, str>>,
    /// Optional default route metric.
    pub metric: Option<u32>,
    /// Number of DAD transmits to use before marking an IPv4 address as
    /// Assigned.
    pub ipv4_dad_transmits: Option<u16>,
    /// Number of DAD transmits to use before marking an IPv6 address as
    /// Assigned.
    pub ipv6_dad_transmits: Option<u16>,
    /// Whether to generate temporary SLAAC addresses.
    ///
    /// If `None`, the interface configuration will not be modified and will remain
    /// the netstack-chosen default.
    pub temporary_addresses: Option<bool>,
    /// Whether to use interface-local route table.
    ///
    /// If `None`, the interface configuration will not be modified and will remain
    /// the netstack-chosen default.
    pub netstack_managed_routes_designation:
        Option<fnet_interfaces_admin::NetstackManagedRoutesDesignation>,
}

impl InterfaceConfig<'_> {
    /// Creates a config that uses the interface local tables;
    pub fn use_local_table() -> Self {
        Self {
            netstack_managed_routes_designation: Some(
                fnet_interfaces_admin::NetstackManagedRoutesDesignation::InterfaceLocal(
                    fnet_interfaces_admin::Empty,
                ),
            ),
            ..Default::default()
        }
    }
}

#[derive(Debug)]
struct ShutdownOnDropConfig {
    enabled: bool,
    ignore_monikers: HashSet<String>,
}

struct TestRealmInner<'a> {
    realm: fnetemul::ManagedRealmProxy,
    name: Cow<'a, str>,
    _sandbox: &'a TestSandbox,
    shutdown_on_drop: Mutex<ShutdownOnDropConfig>,
}

impl Drop for TestRealmInner<'_> {
    fn drop(&mut self) {
        let ShutdownOnDropConfig { enabled, ignore_monikers } = self.shutdown_on_drop.get_mut();
        if !*enabled {
            return;
        }
        let ignore_monikers = std::mem::take(ignore_monikers);
        let mut crashed = match self.shutdown_sync() {
            Ok(crashed) => crashed,
            Err(e) => {
                // If we observe a FIDL closed error don't panic. This likely
                // just means the realm or listener were already shutdown, due
                // to a pipelined realm building error, for example.
                if !e.is_closed() {
                    panic!("error verifying clean shutdown on test realm {}: {:?}", self.name, e);
                }
                return;
            }
        };

        crashed.retain(|m| !ignore_monikers.contains(m));
        if !crashed.is_empty() {
            panic!(
                "TestRealm {} found unclean component stops with monikers: {:?}",
                self.name, crashed
            );
        }
    }
}

impl TestRealmInner<'_> {
    fn shutdown_sync(&self) -> std::result::Result<Vec<String>, fidl::Error> {
        let (listener, server_end) = fidl::endpoints::create_sync_proxy();
        self.realm.get_crash_listener(server_end)?;
        self.realm.shutdown()?;
        // Wait for the realm to go away.
        let _: zx::Signals = self
            .realm
            .as_channel()
            .as_handle_ref()
            .wait_one(zx::Signals::CHANNEL_PEER_CLOSED, zx::MonotonicInstant::INFINITE)
            .to_result()
            .expect("wait channel closed");
        // Drive the listener to get the monikers of any components that did not
        // exit cleanly.
        let mut unclean_stop = Vec::new();
        while let Some(unclean) =
            listener.next(zx::MonotonicInstant::INFINITE).map(|v| (!v.is_empty()).then_some(v))?
        {
            unclean_stop.extend(unclean);
        }
        Ok(unclean_stop)
    }
}

/// A realm within a netemul sandbox.
///
/// Note: A [`TestRealm`] by default attempts to perform a checked shutdown of
/// the realm on drop. Which panics if any unclean component exits occur. This
/// behavior can be opted out with
/// [`TestRealm::set_checked_shutdown_on_drop`].
#[must_use]
#[derive(Clone)]
pub struct TestRealm<'a>(Arc<TestRealmInner<'a>>);

impl<'a> std::fmt::Debug for TestRealm<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let Self(inner) = self;
        let TestRealmInner { realm: _, name, _sandbox, shutdown_on_drop } = &**inner;
        f.debug_struct("TestRealm")
            .field("name", name)
            .field("shutdown_on_drop", shutdown_on_drop)
            .finish_non_exhaustive()
    }
}

impl<'a> TestRealm<'a> {
    fn realm(&self) -> &fnetemul::ManagedRealmProxy {
        let Self(inner) = self;
        &inner.realm
    }

    /// Returns the realm name.
    pub fn name(&self) -> &str {
        let Self(inner) = self;
        &inner.name
    }

    /// Changes the behavior when `TestRealm` (and all its clones) are dropped.
    ///
    /// When `shutdown_on_drop` is `true` (the default value on creation), then
    /// dropping a `TestRealm` performs a synchornous shutdown of the entire
    /// component tree under the realm and *panics* if any unexpected
    /// termination codes are observed.
    pub fn set_checked_shutdown_on_drop(&self, shutdown_on_drop: bool) {
        let Self(inner) = self;
        inner.shutdown_on_drop.lock().enabled = shutdown_on_drop;
    }

    /// Adds `monikers` to a list of monikers whose exit status is _ignored_
    /// when `TestRealm` is dropped.
    ///
    /// This can be used for components that are not long-living or use nonzero
    /// exit codes that are caught as crashes otherwise.
    pub fn ignore_checked_shutdown_monikers(
        &self,
        monikers: impl IntoIterator<Item: Into<String>>,
    ) {
        let Self(inner) = self;
        inner
            .shutdown_on_drop
            .lock()
            .ignore_monikers
            .extend(monikers.into_iter().map(|m| m.into()));
    }

    /// Connects to a protocol within the realm.
    pub fn connect_to_protocol<S>(&self) -> Result<S::Proxy>
    where
        S: fidl::endpoints::DiscoverableProtocolMarker,
    {
        (|| {
            let (proxy, server_end) = fidl::endpoints::create_proxy::<S>();
            self.connect_to_protocol_with_server_end(server_end)
                .context("connect to protocol name with server end")?;
            Result::Ok(proxy)
        })()
        .context(S::DEBUG_NAME)
    }

    /// Connects to a protocol from a child within the realm.
    pub fn connect_to_protocol_from_child<S>(&self, child: &str) -> Result<S::Proxy>
    where
        S: fidl::endpoints::DiscoverableProtocolMarker,
    {
        (|| {
            let (proxy, server_end) = fidl::endpoints::create_proxy::<S>();
            self.connect_to_protocol_from_child_at_path_with_server_end(
                S::PROTOCOL_NAME,
                child,
                server_end,
            )
            .context("connect to protocol name with server end")?;
            Result::Ok(proxy)
        })()
        .with_context(|| format!("{} from {child}", S::DEBUG_NAME))
    }

    /// Opens the diagnostics directory of a component.
    pub fn open_diagnostics_directory(&self, child_name: &str) -> Result<fio::DirectoryProxy> {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        self.realm()
            .open_diagnostics_directory(child_name, server_end)
            .context("open diagnostics dir")?;
        Ok(proxy)
    }

    /// Connects to a protocol within the realm.
    pub fn connect_to_protocol_with_server_end<S: fidl::endpoints::DiscoverableProtocolMarker>(
        &self,
        server_end: fidl::endpoints::ServerEnd<S>,
    ) -> Result {
        self.realm()
            .connect_to_protocol(S::PROTOCOL_NAME, None, server_end.into_channel())
            .context("connect to protocol")
    }

    /// Connects to a protocol from a child at a path within the realm.
    pub fn connect_to_protocol_from_child_at_path_with_server_end<
        S: fidl::endpoints::DiscoverableProtocolMarker,
    >(
        &self,
        protocol_path: &str,
        child: &str,
        server_end: fidl::endpoints::ServerEnd<S>,
    ) -> Result {
        self.realm()
            .connect_to_protocol(protocol_path, Some(child), server_end.into_channel())
            .context("connect to protocol")
    }

    /// Gets the moniker of the root of the managed realm.
    pub async fn get_moniker(&self) -> Result<String> {
        self.realm().get_moniker().await.context("failed to call get moniker")
    }

    /// Starts the specified child component of the managed realm.
    pub async fn start_child_component(&self, child_name: &str) -> Result {
        self.realm()
            .start_child_component(child_name)
            .await?
            .map_err(zx::Status::err_from_raw)
            .with_context(|| format!("failed to start child component '{}'", child_name))
    }

    /// Stops the specified child component of the managed realm.
    pub async fn stop_child_component(&self, child_name: &str) -> Result {
        self.realm()
            .stop_child_component(child_name)
            .await?
            .map_err(zx::Status::err_from_raw)
            .with_context(|| format!("failed to stop child component '{}'", child_name))
    }

    /// Use default endpoint/interface configuration and the specified address
    /// configuration to create a test interface.
    ///
    /// Characters may be dropped from the front of `ep_name` if it exceeds the
    /// maximum length.
    pub async fn join_network<S>(
        &self,
        network: &TestNetwork<'a>,
        ep_name: S,
    ) -> Result<TestInterface<'a>>
    where
        S: Into<Cow<'a, str>>,
    {
        self.join_network_with_if_config(network, ep_name, Default::default()).await
    }

    /// Use default endpoint configuration and the specified interface/address
    /// configuration to create a test interface.
    ///
    /// Characters may be dropped from the front of `ep_name` if it exceeds the
    /// maximum length.
    pub async fn join_network_with_if_config<S>(
        &self,
        network: &TestNetwork<'a>,
        ep_name: S,
        if_config: InterfaceConfig<'a>,
    ) -> Result<TestInterface<'a>>
    where
        S: Into<Cow<'a, str>>,
    {
        let endpoint =
            network.create_endpoint(ep_name).await.context("failed to create endpoint")?;
        self.install_endpoint(endpoint, if_config).await
    }

    /// Joins `network` with by creating an endpoint with `ep_config` and
    /// installing it into the realm with `if_config`.
    ///
    /// Returns a [`TestInterface`] corresponding to the added interface. The
    /// interface is guaranteed to have its link up and be enabled when this
    /// async function resolves.
    ///
    /// Note that this realm needs a Netstack for this operation to succeed.
    ///
    /// Characters may be dropped from the front of `ep_name` if it exceeds the maximum length.
    pub async fn join_network_with(
        &self,
        network: &TestNetwork<'a>,
        ep_name: impl Into<Cow<'a, str>>,
        ep_config: fnetemul_network::EndpointConfig,
        if_config: InterfaceConfig<'a>,
    ) -> Result<TestInterface<'a>> {
        let installer = self
            .connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>()
            .context("failed to connect to fuchsia.net.interfaces.admin.Installer")?;
        let interface_state = self
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .context("failed to connect to fuchsia.net.interfaces.State")?;
        let (endpoint, id, control, device_control) = self
            .join_network_with_installer(
                network,
                installer,
                interface_state,
                ep_name,
                ep_config,
                if_config,
            )
            .await?;

        Ok(TestInterface {
            endpoint,
            id,
            realm: self.clone(),
            control,
            device_control: Some(device_control),
            dhcp_client_task: futures::lock::Mutex::default(),
        })
    }

    /// Joins `network` by creating an endpoint with `ep_config` and installing it with
    /// `installer` and `if_config`.
    ///
    /// This inherits the lifetime of `self`, so there's an assumption that `installer` is served
    /// by something in this [`TestRealm`], but there's nothing enforcing this.
    ///
    /// Returns the created endpoint, the interface ID, and the associated interface
    /// [`Control`] and [`fnet_interfaces_admin::DeviceControlProxy`].
    ///
    /// Characters may be dropped from the front of `ep_name` if it exceeds the maximum length.
    pub async fn join_network_with_installer(
        &self,
        network: &TestNetwork<'a>,
        installer: fnet_interfaces_admin::InstallerProxy,
        interface_state: fnet_interfaces::StateProxy,
        ep_name: impl Into<Cow<'a, str>>,
        ep_config: fnetemul_network::EndpointConfig,
        if_config: InterfaceConfig<'a>,
    ) -> Result<(TestEndpoint<'a>, u64, Control, fnet_interfaces_admin::DeviceControlProxy)> {
        let endpoint = network
            .create_endpoint_with(ep_name, ep_config)
            .await
            .context("failed to create endpoint")?;
        let (id, control, device_control) = self
            .install_endpoint_with_installer(installer, interface_state, &endpoint, if_config)
            .await?;
        Ok((endpoint, id, control, device_control))
    }

    /// Installs and configures the endpoint as an interface. Uses `interface_state` to observe that
    /// the interface is up after it is installed.
    ///
    /// This inherits the lifetime of `self`, so there's an assumption that `installer` is served
    /// by something in this [`TestRealm`], but there's nothing enforcing this.
    ///
    /// Note that if `name` is not `None`, the string must fit within interface name limits.
    pub async fn install_endpoint_with_installer(
        &self,
        installer: fnet_interfaces_admin::InstallerProxy,
        interface_state: fnet_interfaces::StateProxy,
        endpoint: &TestEndpoint<'a>,
        if_config: InterfaceConfig<'a>,
    ) -> Result<(u64, Control, fnet_interfaces_admin::DeviceControlProxy)> {
        let (id, control, device_control) =
            endpoint.install(installer, if_config).await.context("failed to add endpoint")?;

        endpoint.set_link_up(true).await.context("failed to start endpoint")?;
        let _did_enable: bool = control
            .enable()
            .await
            .map_err(anyhow::Error::new)
            .and_then(|res| {
                res.map_err(|e: fnet_interfaces_admin::ControlEnableError| {
                    anyhow::anyhow!("{:?}", e)
                })
            })
            .context("failed to enable interface")?;

        // Wait for Netstack to observe interface up so callers can safely
        // assume the state of the world on return.
        fnet_interfaces_ext::wait_interface_with_id(
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
                &interface_state,
                Default::default(),
            )?,
            &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(id),
            |properties_and_state| properties_and_state.properties.online.then_some(()),
        )
        .await
        .context("failed to observe interface up")?;

        Ok((id, control, device_control))
    }

    /// Installs and configures the endpoint in this realm.
    ///
    /// Note that if `name` is not `None`, the string must fit within interface name limits.
    pub async fn install_endpoint(
        &self,
        endpoint: TestEndpoint<'a>,
        if_config: InterfaceConfig<'a>,
    ) -> Result<TestInterface<'a>> {
        let installer = self
            .connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>()
            .context("failed to connect to fuchsia.net.interfaces.admin.Installer")?;
        let interface_state = self
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .context("failed to connect to fuchsia.net.interfaces.State")?;
        let (id, control, device_control) = self
            .install_endpoint_with_installer(installer, interface_state, &endpoint, if_config)
            .await?;
        Ok(TestInterface {
            endpoint,
            id,
            realm: self.clone(),
            control,
            device_control: Some(device_control),
            dhcp_client_task: futures::lock::Mutex::default(),
        })
    }

    /// Adds a raw device connector to the realm's devfs.
    pub async fn add_raw_device(
        &self,
        path: &Path,
        device: fidl::endpoints::ClientEnd<fnetemul_network::DeviceProxy_Marker>,
    ) -> Result {
        let path = path.to_str().with_context(|| format!("convert {} to str", path.display()))?;
        self.realm()
            .add_device(path, device)
            .await
            .context("add device")?
            .map_err(zx::Status::err_from_raw)
            .context("add device error")
    }

    /// Adds a device to the realm's virtual device filesystem.
    pub async fn add_virtual_device(&self, e: &TestEndpoint<'_>, path: &Path) -> Result {
        let (device, device_server_end) =
            fidl::endpoints::create_endpoints::<fnetemul_network::DeviceProxy_Marker>();
        e.get_proxy_(device_server_end).context("get proxy")?;

        self.add_raw_device(path, device).await
    }

    /// Removes a device from the realm's virtual device filesystem.
    pub async fn remove_virtual_device(&self, path: &Path) -> Result {
        let path = path.to_str().with_context(|| format!("convert {} to str", path.display()))?;
        self.realm()
            .remove_device(path)
            .await
            .context("remove device")?
            .map_err(zx::Status::err_from_raw)
            .context("remove device error")
    }

    /// Creates a Datagram [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket/Provider` in this realm.
    pub async fn datagram_socket(
        &self,
        domain: fposix_socket::Domain,
        proto: fposix_socket::DatagramSocketProtocol,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("failed to connect to socket provider")?;

        fposix_socket_ext::datagram_socket(&socket_provider, domain, proto)
            .await
            .context("failed to call socket")?
            .context("failed to create socket")
    }

    /// Creates a Datagram [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket/Provider` in this realm and with the specified
    /// options.
    pub async fn datagram_socket_with_options(
        &self,
        domain: fposix_socket::Domain,
        proto: fposix_socket::DatagramSocketProtocol,
        options: fposix_socket::SocketCreationOptions,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("failed to connect to socket provider")?;

        fposix_socket_ext::datagram_socket_with_options(&socket_provider, domain, proto, options)
            .await
            .context("failed to call socket")?
            .context("failed to create socket")
    }

    /// Creates a raw [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket.raw/Provider` in this realm.
    pub async fn raw_socket(
        &self,
        domain: fposix_socket::Domain,
        association: fposix_socket_raw::ProtocolAssociation,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket_raw::ProviderMarker>()
            .context("failed to connect to socket provider")?;
        let sock = socket_provider
            .socket(domain, &association)
            .await
            .context("failed to call socket")?
            .map_err(|e| std::io::Error::from_raw_os_error(e.into_primitive()))
            .context("failed to create socket")?;

        Ok(fdio::create_fd(sock.into()).context("failed to create fd")?.into())
    }

    /// Creates a [`socket2::Socket`] backed by the implementation of
    /// [`fuchsia.posix.socket.packet/Provider`] in this realm.
    ///
    /// [`fuchsia.posix.socket.packet/Provider`]: fposix_socket_packet::ProviderMarker
    pub async fn packet_socket(&self, kind: fposix_socket_packet::Kind) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket_packet::ProviderMarker>()
            .context("failed to connect to socket provider")?;

        fposix_socket_ext::packet_socket(&socket_provider, kind)
            .await
            .context("failed to call socket")?
            .context("failed to create socket")
    }

    /// Creates a Stream [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket/Provider` in this realm.
    pub async fn stream_socket(
        &self,
        domain: fposix_socket::Domain,
        proto: fposix_socket::StreamSocketProtocol,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("failed to connect to socket provider")?;
        let sock = socket_provider
            .stream_socket(domain, proto)
            .await
            .context("failed to call socket")?
            .map_err(|e| std::io::Error::from_raw_os_error(e.into_primitive()))
            .context("failed to create socket")?;

        Ok(fdio::create_fd(sock.into()).context("failed to create fd")?.into())
    }

    /// Creates a Stream [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket/Provider` in this realm and with the specified
    /// options.
    pub async fn stream_socket_with_options(
        &self,
        domain: fposix_socket::Domain,
        proto: fposix_socket::StreamSocketProtocol,
        options: fposix_socket::SocketCreationOptions,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("failed to connect to socket provider")?;
        let sock = socket_provider
            .stream_socket_with_options(domain, proto, options)
            .await
            .context("failed to call socket")?
            .map_err(|e| std::io::Error::from_raw_os_error(e.into_primitive()))
            .context("failed to create socket")?;

        Ok(fdio::create_fd(sock.into()).context("failed to create fd")?.into())
    }
    /// Shuts down the realm.
    ///
    /// It is often useful to call this method to ensure that the realm
    /// completes orderly shutdown before allowing other resources to be dropped
    /// and get cleaned up, such as [`TestEndpoint`]s, which components in the
    /// realm might be interacting with.
    pub async fn shutdown(&self) -> Result {
        self.realm().shutdown().context("call shutdown")?;
        // Turn off shutdown on drop from now on, we're already shutting down
        // here.
        self.set_checked_shutdown_on_drop(false);
        let events = self
            .realm()
            .take_event_stream()
            .try_collect::<Vec<_>>()
            .await
            .context("error on realm event stream")?;
        // Ensure there are no more events sent on the event stream after `OnShutdown`.
        assert_matches::assert_matches!(events[..], [fnetemul::ManagedRealmEvent::OnShutdown {}]);
        Ok(())
    }

    /// Returns a stream that yields monikers of unclean exits in the realm.
    pub async fn get_crash_stream(&self) -> Result<impl futures::Stream<Item = Result<String>>> {
        let (listener, server_end) = fidl::endpoints::create_proxy();
        self.realm().get_crash_listener(server_end).context("creating CrashListener")?;
        Ok(futures::stream::try_unfold(listener, |listener| async move {
            let next = listener.next().await.context("listener fetch next moniker")?;
            Result::Ok(if next.is_empty() {
                None
            } else {
                Some((futures::stream::iter(next.into_iter().map(Ok)), listener))
            })
        })
        .try_flatten())
    }

    /// Constructs an ICMP socket.
    pub async fn icmp_socket<Ip: ping::FuchsiaIpExt>(
        &self,
    ) -> Result<fuchsia_async::net::DatagramSocket> {
        let sock = self
            .datagram_socket(Ip::DOMAIN_FIDL, fposix_socket::DatagramSocketProtocol::IcmpEcho)
            .await
            .context("failed to create ICMP datagram socket")?;
        fuchsia_async::net::DatagramSocket::new_from_socket(sock)
            .context("failed to create async ICMP datagram socket")
    }

    /// Sends a single ICMP echo request to `addr`, and waits for the echo reply.
    pub async fn ping_once<Ip: ping::FuchsiaIpExt>(&self, addr: Ip::SockAddr, seq: u16) -> Result {
        let icmp_sock = self.icmp_socket::<Ip>().await?;

        const MESSAGE: &'static str = "hello, world";
        let (mut sink, mut stream) = ping::new_unicast_sink_and_stream::<
            Ip,
            _,
            { MESSAGE.len() + ping::ICMP_HEADER_LEN },
        >(&icmp_sock, &addr, MESSAGE.as_bytes());

        let send_fut = sink.send(seq).map_err(anyhow::Error::new);
        let recv_fut = stream.try_next().map(|r| match r {
            Ok(Some(got)) if got == seq => Ok(()),
            Ok(Some(got)) => Err(anyhow!("unexpected echo reply; got: {}, want: {}", got, seq)),
            Ok(None) => Err(anyhow!("echo reply stream ended unexpectedly")),
            Err(e) => Err(anyhow::Error::from(e)),
        });

        let ((), ()) = futures::future::try_join(send_fut, recv_fut)
            .await
            .with_context(|| format!("failed to ping from {} to {}", self.name(), addr,))?;
        Ok(())
    }

    /// Add a static neighbor entry.
    ///
    /// Useful to prevent NUD resolving too slow and causing spurious test failures.
    pub async fn add_neighbor_entry(
        &self,
        interface: u64,
        addr: fnet::IpAddress,
        mac: fnet::MacAddress,
    ) -> Result {
        let controller = self
            .connect_to_protocol::<fnet_neighbor::ControllerMarker>()
            .context("connect to protocol")?;
        controller
            .add_entry(interface, &addr, &mac)
            .await
            .context("add_entry")?
            .map_err(|e| anyhow::anyhow!("add_entry failed: {e:?}"))
    }

    /// Get a stream of interface events from a new watcher with default
    /// interest.
    pub fn get_interface_event_stream(
        &self,
    ) -> Result<
        impl futures::Stream<
            Item = std::result::Result<
                fnet_interfaces_ext::EventWithInterest<fnet_interfaces_ext::DefaultInterest>,
                fidl::Error,
            >,
        >,
    > {
        self.get_interface_event_stream_with_interest::<fnet_interfaces_ext::DefaultInterest>()
    }

    /// Get a stream of interface events from a new watcher with specified
    /// interest.
    pub fn get_interface_event_stream_with_interest<I: fnet_interfaces_ext::FieldInterests>(
        &self,
    ) -> Result<
        impl futures::Stream<
            Item = std::result::Result<fnet_interfaces_ext::EventWithInterest<I>, fidl::Error>,
        >,
    > {
        let interface_state = self
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .context("connect to protocol")?;
        fnet_interfaces_ext::event_stream_from_state::<I>(&interface_state, Default::default())
            .context("get interface event stream")
    }

    /// Gets the table ID for the main route table.
    pub async fn main_table_id<
        I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
    >(
        &self,
    ) -> u32 {
        let main_route_table = self
            .connect_to_protocol::<I::RouteTableMarker>()
            .expect("failed to connect to main route table");
        fnet_routes_ext::admin::get_table_id::<I>(&main_route_table)
            .await
            .expect("failed to get_table_id")
            .get()
    }
}

/// A virtual Network.
///
/// `TestNetwork` is a single virtual broadcast domain backed by Netemul.
/// Created through [`TestSandbox::create_network`].
#[must_use]
pub struct TestNetwork<'a> {
    network: fnetemul_network::NetworkProxy,
    name: Cow<'a, str>,
    sandbox: &'a TestSandbox,
}

impl<'a> std::fmt::Debug for TestNetwork<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let Self { name, network: _, sandbox: _ } = self;
        f.debug_struct("TestNetwork").field("name", name).finish_non_exhaustive()
    }
}

impl<'a> TestNetwork<'a> {
    /// Extracts the proxy to the backing network.
    ///
    /// Note that this defeats the lifetime semantics that ensure the sandbox in
    /// which this network was created lives as long as the network. The caller of
    /// [`TestNetwork::into_proxy`] is responsible for ensuring that the sandbox
    /// outlives the network.
    pub fn into_proxy(self) -> fnetemul_network::NetworkProxy {
        let Self { network, name: _, sandbox: _ } = self;
        network
    }

    /// Gets a FIDL client for the backing network.
    async fn get_client_end_clone(
        &self,
    ) -> Result<fidl::endpoints::ClientEnd<fnetemul_network::NetworkMarker>> {
        let network_manager =
            self.sandbox.get_network_manager().context("get_network_manager failed")?;
        let client = network_manager
            .get_network(&self.name)
            .await
            .context("get_network failed")?
            .with_context(|| format!("no network found with name {}", self.name))?;
        Ok(client)
    }

    /// Sets the configuration for this network to `config`.
    pub async fn set_config(&self, config: fnetemul_network::NetworkConfig) -> Result<()> {
        let status = self.network.set_config(&config).await.context("call set_config")?;
        zx::Status::ok(status).context("set config")
    }

    /// Attaches `ep` to this network.
    pub async fn attach_endpoint(&self, ep: &TestEndpoint<'a>) -> Result<()> {
        let status =
            self.network.attach_endpoint(&ep.name).await.context("attach_endpoint FIDL error")?;
        zx::Status::ok(status).context("attach_endpoint failed")?;
        Ok(())
    }

    /// Creates a new endpoint with `name` attached to this network.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    pub async fn create_endpoint<S>(&self, name: S) -> Result<TestEndpoint<'a>>
    where
        S: Into<Cow<'a, str>>,
    {
        let ep = self
            .sandbox
            .create_endpoint(name)
            .await
            .with_context(|| format!("failed to create endpoint for network {}", self.name))?;
        self.attach_endpoint(&ep).await.with_context(|| {
            format!("failed to attach endpoint {} to network {}", ep.name, self.name)
        })?;
        Ok(ep)
    }

    /// Creates a new endpoint with `name` and `config` attached to this network.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    pub async fn create_endpoint_with(
        &self,
        name: impl Into<Cow<'a, str>>,
        config: fnetemul_network::EndpointConfig,
    ) -> Result<TestEndpoint<'a>> {
        let ep = self
            .sandbox
            .create_endpoint_with(name, config)
            .await
            .with_context(|| format!("failed to create endpoint for network {}", self.name))?;
        self.attach_endpoint(&ep).await.with_context(|| {
            format!("failed to attach endpoint {} to network {}", ep.name, self.name)
        })?;
        Ok(ep)
    }

    /// Returns a fake endpoint.
    pub fn create_fake_endpoint(&self) -> Result<TestFakeEndpoint<'a>> {
        let (endpoint, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::FakeEndpointMarker>();
        self.network.create_fake_endpoint(server)?;
        return Ok(TestFakeEndpoint { endpoint, _sandbox: self.sandbox });
    }

    /// Starts capturing packet in this network.
    ///
    /// The packet capture will be stored under a predefined directory:
    /// `/custom_artifacts`. More details can be found here:
    /// https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework?hl=en#custom-artifacts
    pub async fn start_capture(&self, name: &str) -> Result<PacketCapture> {
        let manager = self.sandbox.get_network_manager()?;
        let client = manager.get_network(&self.name).await?.expect("network must exist");
        zx::ok(self.network.start_capture(name).await?)?;
        let sync_proxy = fnetemul_network::NetworkSynchronousProxy::new(client.into_channel());
        Ok(PacketCapture { sync_proxy })
    }

    /// Stops packet capture in this network.
    pub async fn stop_capture(&self) -> Result<()> {
        Ok(self.network.stop_capture().await?)
    }
}

/// The object that has the same life as the packet capture, once the object is
/// dropped, the underlying packet capture will be stopped.
pub struct PacketCapture {
    sync_proxy: fnetemul_network::NetworkSynchronousProxy,
}

impl Drop for PacketCapture {
    fn drop(&mut self) {
        self.sync_proxy
            .stop_capture(zx::MonotonicInstant::INFINITE)
            .expect("failed to stop packet capture")
    }
}

/// A virtual network endpoint backed by Netemul.
#[must_use]
pub struct TestEndpoint<'a> {
    endpoint: fnetemul_network::EndpointProxy,
    name: Cow<'a, str>,
    _sandbox: &'a TestSandbox,
}

impl<'a> TestEndpoint<'a> {
    /// Returns the KOID  of the `zx::Event` identifier for the port backing
    /// this endpoint.
    pub async fn get_port_identity_koid(&self) -> Result<zx::Koid> {
        let (client, server) = fidl::endpoints::create_proxy::<fnetwork::PortMarker>();
        self.get_port(server)?;
        let identity = client.get_identity().await?;
        Ok(identity.koid()?)
    }
}

impl<'a> std::fmt::Debug for TestEndpoint<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let Self { endpoint: _, name, _sandbox } = self;
        f.debug_struct("TestEndpoint").field("name", name).finish_non_exhaustive()
    }
}

impl<'a> std::ops::Deref for TestEndpoint<'a> {
    type Target = fnetemul_network::EndpointProxy;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

/// A virtual fake network endpoint backed by Netemul.
#[must_use]
pub struct TestFakeEndpoint<'a> {
    endpoint: fnetemul_network::FakeEndpointProxy,
    _sandbox: &'a TestSandbox,
}

impl<'a> std::ops::Deref for TestFakeEndpoint<'a> {
    type Target = fnetemul_network::FakeEndpointProxy;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

impl<'a> TestFakeEndpoint<'a> {
    /// Return a stream of frames.
    ///
    /// Frames will be yielded as they are read from the fake endpoint.
    pub fn frame_stream(
        &self,
    ) -> impl futures::Stream<Item = std::result::Result<(Vec<u8>, u64), fidl::Error>> + '_ {
        futures::stream::try_unfold(&self.endpoint, |ep| ep.read().map_ok(move |r| Some((r, ep))))
    }
}

/// Helper function to retrieve device and port information from a port
/// instance.
async fn to_netdevice_inner(
    port: fidl::endpoints::ClientEnd<fnetwork::PortMarker>,
) -> Result<(fidl::endpoints::ClientEnd<fnetwork::DeviceMarker>, fnetwork::PortId)> {
    let port = port.into_proxy();
    let (device, server_end) = fidl::endpoints::create_endpoints::<fnetwork::DeviceMarker>();
    port.get_device(server_end)?;
    let port_id = port
        .get_info()
        .await
        .context("get port info")?
        .id
        .ok_or_else(|| anyhow::anyhow!("missing port id"))?;
    Ok((device, port_id))
}

impl<'a> TestEndpoint<'a> {
    /// Extracts the proxy to the backing endpoint.
    ///
    /// Note that this defeats the lifetime semantics that ensure the sandbox in
    /// which this endpoint was created lives as long as the endpoint. The caller of
    /// [`TestEndpoint::into_proxy`] is responsible for ensuring that the sandbox
    /// outlives the endpoint.
    pub fn into_proxy(self) -> fnetemul_network::EndpointProxy {
        let Self { endpoint, name: _, _sandbox: _ } = self;
        endpoint
    }

    /// Gets access to this device's virtual Network device.
    ///
    /// Note that an error is returned if the Endpoint is not a
    /// [`fnetemul_network::DeviceConnection::NetworkDevice`].
    pub async fn get_netdevice(
        &self,
    ) -> Result<(fidl::endpoints::ClientEnd<fnetwork::DeviceMarker>, fnetwork::PortId)> {
        let (port, server_end) = fidl::endpoints::create_endpoints();
        self.get_port(server_end)
            .with_context(|| format!("failed to get device connection for {}", self.name))?;
        to_netdevice_inner(port).await
    }

    /// Installs the [`TestEndpoint`] via the provided [`fnet_interfaces_admin::InstallerProxy`].
    ///
    /// Returns the interface ID, and the associated interface
    /// [`Control`] and [`fnet_interfaces_admin::DeviceControlProxy`] on
    /// success.
    pub async fn install(
        &self,
        installer: fnet_interfaces_admin::InstallerProxy,
        InterfaceConfig {
            name,
            metric,
            ipv4_dad_transmits,
            ipv6_dad_transmits,
            temporary_addresses,
            netstack_managed_routes_designation,
        }: InterfaceConfig<'_>,
    ) -> Result<(u64, Control, fnet_interfaces_admin::DeviceControlProxy)> {
        let name = name.map(|n| {
            truncate_dropping_front(n.into(), fnet_interfaces::INTERFACE_NAME_LENGTH.into())
                .to_string()
        });
        let (device, port_id) = self.get_netdevice().await?;
        let device_control = {
            let (control, server_end) =
                fidl::endpoints::create_proxy::<fnet_interfaces_admin::DeviceControlMarker>();
            installer.install_device(device, server_end).context("install device")?;
            control
        };
        let (control, server_end) = Control::create_endpoints().context("create endpoints")?;
        device_control
            .create_interface(
                &port_id,
                server_end,
                fnet_interfaces_admin::Options {
                    name,
                    metric,
                    netstack_managed_routes_designation,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            )
            .context("create interface")?;
        if let Some(ipv4_dad_transmits) = ipv4_dad_transmits {
            let _: Option<u16> = set_ipv4_dad_transmits(&control, ipv4_dad_transmits)
                .await
                .context("set dad transmits")?;
        }
        if let Some(ipv6_dad_transmits) = ipv6_dad_transmits {
            let _: Option<u16> = set_ipv6_dad_transmits(&control, ipv6_dad_transmits)
                .await
                .context("set dad transmits")?;
        }
        if let Some(enabled) = temporary_addresses {
            set_temporary_address_generation_enabled(&control, enabled)
                .await
                .context("set temporary addresses")?;
        }

        let id = control.get_id().await.context("get id")?;
        Ok((id, control, device_control))
    }

    /// Adds the [`TestEndpoint`] to the provided `realm` with an optional
    /// interface name.
    ///
    /// Returns the interface ID and control protocols on success.
    pub async fn add_to_stack(
        &self,
        realm: &TestRealm<'a>,
        config: InterfaceConfig<'a>,
    ) -> Result<(u64, Control, fnet_interfaces_admin::DeviceControlProxy)> {
        let installer = realm
            .connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>()
            .context("connect to protocol")?;

        self.install(installer, config).await
    }

    /// Like `into_interface_realm_with_name` but with default parameters.
    pub async fn into_interface_in_realm(self, realm: &TestRealm<'a>) -> Result<TestInterface<'a>> {
        self.into_interface_in_realm_with_name(realm, Default::default()).await
    }

    /// Consumes this `TestEndpoint` and tries to add it to the Netstack in
    /// `realm`, returning a [`TestInterface`] on success.
    pub async fn into_interface_in_realm_with_name(
        self,
        realm: &TestRealm<'a>,
        config: InterfaceConfig<'a>,
    ) -> Result<TestInterface<'a>> {
        let installer = realm
            .connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>()
            .context("connect to protocol")?;

        let (id, control, device_control) =
            self.install(installer, config).await.context("failed to install")?;

        Ok(TestInterface {
            endpoint: self,
            id,
            realm: realm.clone(),
            control,
            device_control: Some(device_control),
            dhcp_client_task: futures::lock::Mutex::default(),
        })
    }
}

/// The DHCP client version.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum DhcpClientVersion {
    /// The in-Netstack2 DHCP client.
    InStack,
    /// The out-of-stack DHCP client.
    OutOfStack,
}

/// Abstraction for how DHCP client functionality is provided.
pub trait DhcpClient {
    /// The DHCP client version to be used.
    const DHCP_CLIENT_VERSION: DhcpClientVersion;
}

/// The in-Netstack2 DHCP client.
pub enum InStack {}

impl DhcpClient for InStack {
    const DHCP_CLIENT_VERSION: DhcpClientVersion = DhcpClientVersion::InStack;
}

/// The out-of-stack DHCP client.
pub enum OutOfStack {}

impl DhcpClient for OutOfStack {
    const DHCP_CLIENT_VERSION: DhcpClientVersion = DhcpClientVersion::OutOfStack;
}

/// A [`TestEndpoint`] that is installed in a realm's Netstack.
///
/// Note that a [`TestInterface`] adds to the reference count of the underlying
/// realm of its [`TestRealm`]. That is, a [`TestInterface`] that outlives the
/// [`TestRealm`] it created is sufficient to keep the underlying realm alive.
#[must_use]
pub struct TestInterface<'a> {
    endpoint: TestEndpoint<'a>,
    realm: TestRealm<'a>,
    id: u64,
    control: Control,
    device_control: Option<fnet_interfaces_admin::DeviceControlProxy>,
    dhcp_client_task: futures::lock::Mutex<Option<fnet_dhcp_ext::testutil::DhcpClientTask>>,
}

impl<'a> std::fmt::Debug for TestInterface<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let Self { endpoint, id, realm: _, control: _, device_control: _, dhcp_client_task: _ } =
            self;
        f.debug_struct("TestInterface")
            .field("endpoint", endpoint)
            .field("id", id)
            .finish_non_exhaustive()
    }
}

impl<'a> std::ops::Deref for TestInterface<'a> {
    type Target = fnetemul_network::EndpointProxy;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

impl<'a> TestInterface<'a> {
    /// Gets the interface identifier.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns the endpoint associated with the interface.
    pub fn endpoint(&self) -> &TestEndpoint<'a> {
        &self.endpoint
    }

    /// Returns the interface's control handle.
    pub fn control(&self) -> &Control {
        &self.control
    }

    /// Returns the authorization token for this interface.
    pub async fn get_authorization(
        &self,
    ) -> Result<fnet_resources::GrantForInterfaceAuthorization> {
        Ok(self.control.get_authorization_for_interface().await?)
    }

    /// Connects to fuchsia.net.stack in this interface's realm.
    pub fn connect_stack(&self) -> Result<fnet_stack::StackProxy> {
        self.realm.connect_to_protocol::<fnet_stack::StackMarker>()
    }

    /// Installs a route in the realm's netstack's global route table with `self` as the outgoing
    /// interface with the given `destination` and `metric`, optionally via the `next_hop`.
    ///
    /// Returns whether the route was newly added to the stack.
    async fn add_route<
        I: Ip + fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
    >(
        &self,
        destination: Subnet<I::Addr>,
        next_hop: Option<SpecifiedAddr<I::Addr>>,
        metric: fnet_routes::SpecifiedMetric,
    ) -> Result<bool> {
        let route_set = self.create_authenticated_global_route_set::<I>().await?;
        fnet_routes_ext::admin::add_route::<I>(
            &route_set,
            &fnet_routes_ext::Route::<I>::new_forward(destination, self.id(), next_hop, metric)
                .try_into()
                .expect("convert to FIDL should succeed"),
        )
        .await
        .context("FIDL error adding route")?
        .map_err(|e| anyhow::anyhow!("error adding route: {e:?}"))
    }

    /// Installs a route in the realm's netstack's global route table with `self` as the outgoing
    /// interface with the given `destination` and `metric`, optionally via the `next_hop`.
    ///
    /// Returns whether the route was newly added to the stack. Returns `Err` if `destination` and
    /// `next_hop` don't share the same IP version.
    pub async fn add_route_either(
        &self,
        destination: fnet::Subnet,
        next_hop: Option<fnet::IpAddress>,
        metric: fnet_routes::SpecifiedMetric,
    ) -> Result<bool> {
        let fnet::Subnet { addr: destination_addr, prefix_len } = destination;
        match destination_addr {
            fnet::IpAddress::Ipv4(destination_addr) => {
                let next_hop = match next_hop {
                    Some(fnet::IpAddress::Ipv4(next_hop)) => Some(
                        SpecifiedAddr::new(net_types::ip::Ipv4Addr::from_ext(next_hop))
                            .ok_or_else(|| {
                                anyhow::anyhow!("next hop must not be unspecified address")
                            })?,
                    ),
                    Some(fnet::IpAddress::Ipv6(_)) => {
                        return Err(anyhow::anyhow!(
                            "next hop must be same IP version as destination"
                        ));
                    }
                    None => None,
                };
                self.add_route::<Ipv4>(
                    Subnet::new(destination_addr.into_ext(), prefix_len)
                        .map_err(|e| anyhow::anyhow!("invalid subnet: {e:?}"))?,
                    next_hop,
                    metric,
                )
                .await
            }
            fnet::IpAddress::Ipv6(destination_addr) => {
                let next_hop = match next_hop {
                    Some(fnet::IpAddress::Ipv6(next_hop)) => Some(
                        SpecifiedAddr::new(net_types::ip::Ipv6Addr::from_ext(next_hop))
                            .ok_or_else(|| {
                                anyhow::anyhow!("next hop must not be unspecified address")
                            })?,
                    ),
                    Some(fnet::IpAddress::Ipv4(_)) => {
                        return Err(anyhow::anyhow!(
                            "next hop must be same IP version as destination"
                        ));
                    }
                    None => None,
                };
                self.add_route::<Ipv6>(
                    Subnet::new(destination_addr.into_ext(), prefix_len)
                        .map_err(|e| anyhow::anyhow!("invalid subnet: {e:?}"))?,
                    next_hop,
                    metric,
                )
                .await
            }
        }
    }

    /// Removes a route from the realm's netstack's global route table with `self` as the outgoing
    /// interface with the given `destination` and `metric`, optionally via the `next_hop`.
    ///
    /// Returns whether the route actually existed in the stack before it was removed.
    async fn remove_route<
        I: Ip + fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
    >(
        &self,
        destination: Subnet<I::Addr>,
        next_hop: Option<SpecifiedAddr<I::Addr>>,
        metric: fnet_routes::SpecifiedMetric,
    ) -> Result<bool> {
        let route_set = self.create_authenticated_global_route_set::<I>().await?;
        fnet_routes_ext::admin::remove_route::<I>(
            &route_set,
            &fnet_routes_ext::Route::<I>::new_forward(destination, self.id(), next_hop, metric)
                .try_into()
                .expect("convert to FIDL should succeed"),
        )
        .await
        .context("FIDL error removing route")?
        .map_err(|e| anyhow::anyhow!("error removing route: {e:?}"))
    }

    /// Removes a route from the realm's netstack's global route table with `self` as the outgoing
    /// interface with the given `destination` and `metric`, optionally via the `next_hop`.
    ///
    /// Returns whether the route actually existed in the stack before it was removed. Returns `Err`
    /// if `destination` and `next_hop` don't share the same IP version.
    async fn remove_route_either(
        &self,
        destination: fnet::Subnet,
        next_hop: Option<fnet::IpAddress>,
        metric: fnet_routes::SpecifiedMetric,
    ) -> Result<bool> {
        let fnet::Subnet { addr: destination_addr, prefix_len } = destination;
        match destination_addr {
            fnet::IpAddress::Ipv4(destination_addr) => {
                let next_hop = match next_hop {
                    Some(fnet::IpAddress::Ipv4(next_hop)) => Some(
                        SpecifiedAddr::new(net_types::ip::Ipv4Addr::from_ext(next_hop))
                            .ok_or_else(|| {
                                anyhow::anyhow!("next hop must not be unspecified address")
                            })?,
                    ),
                    Some(fnet::IpAddress::Ipv6(_)) => {
                        return Err(anyhow::anyhow!(
                            "next hop must be same IP version as destination"
                        ));
                    }
                    None => None,
                };
                self.remove_route::<Ipv4>(
                    Subnet::new(destination_addr.into_ext(), prefix_len)
                        .map_err(|e| anyhow::anyhow!("invalid subnet: {e:?}"))?,
                    next_hop,
                    metric,
                )
                .await
            }
            fnet::IpAddress::Ipv6(destination_addr) => {
                let next_hop = match next_hop {
                    Some(fnet::IpAddress::Ipv6(next_hop)) => Some(
                        SpecifiedAddr::new(net_types::ip::Ipv6Addr::from_ext(next_hop))
                            .ok_or_else(|| {
                                anyhow::anyhow!("next hop must not be unspecified address")
                            })?,
                    ),
                    Some(fnet::IpAddress::Ipv4(_)) => {
                        return Err(anyhow::anyhow!(
                            "next hop must be same IP version as destination"
                        ));
                    }
                    None => None,
                };
                self.remove_route::<Ipv6>(
                    Subnet::new(destination_addr.into_ext(), prefix_len)
                        .map_err(|e| anyhow::anyhow!("invalid subnet: {e:?}"))?,
                    next_hop,
                    metric,
                )
                .await
            }
        }
    }

    /// Add a direct route from the interface to the given subnet.
    pub async fn add_subnet_route(&self, subnet: fnet::Subnet) -> Result<()> {
        let subnet = fnet_ext::apply_subnet_mask(subnet);
        let newly_added = self
            .add_route_either(
                subnet,
                None,
                fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            )
            .await?;

        if !newly_added {
            Err(anyhow::anyhow!(
                "route to {subnet:?} on {} should not have already existed",
                self.id()
            ))
        } else {
            Ok(())
        }
    }

    /// Delete a direct route from the interface to the given subnet.
    pub async fn del_subnet_route(&self, subnet: fnet::Subnet) -> Result<()> {
        let subnet = fnet_ext::apply_subnet_mask(subnet);
        let newly_removed = self
            .remove_route_either(
                subnet,
                None,
                fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            )
            .await?;

        if !newly_removed {
            Err(anyhow::anyhow!(
                "route to {subnet:?} on {} should have previously existed before being removed",
                self.id()
            ))
        } else {
            Ok(())
        }
    }

    /// Add a default route through the given `next_hop` with the given `metric`.
    pub async fn add_default_route_with_metric(
        &self,
        next_hop: fnet::IpAddress,
        metric: fnet_routes::SpecifiedMetric,
    ) -> Result<()> {
        let corresponding_default_subnet = match next_hop {
            fnet::IpAddress::Ipv4(_) => net_declare::fidl_subnet!("0.0.0.0/0"),
            fnet::IpAddress::Ipv6(_) => net_declare::fidl_subnet!("::/0"),
        };

        let newly_added =
            self.add_route_either(corresponding_default_subnet, Some(next_hop), metric).await?;

        if !newly_added {
            Err(anyhow::anyhow!(
                "default route through {} via {next_hop:?} already exists",
                self.id()
            ))
        } else {
            Ok(())
        }
    }

    /// Add a default route through the given `next_hop` with the given `metric`.
    pub async fn add_default_route_with_explicit_metric(
        &self,
        next_hop: fnet::IpAddress,
        metric: u32,
    ) -> Result<()> {
        self.add_default_route_with_metric(
            next_hop,
            fnet_routes::SpecifiedMetric::ExplicitMetric(metric),
        )
        .await
    }

    /// Add a default route through the given `next_hop`.
    pub async fn add_default_route(&self, next_hop: fnet::IpAddress) -> Result<()> {
        self.add_default_route_with_metric(
            next_hop,
            fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
        )
        .await
    }

    /// Remove a default route through the given address.
    pub async fn remove_default_route(&self, next_hop: fnet::IpAddress) -> Result<()> {
        let corresponding_default_subnet = match next_hop {
            fnet::IpAddress::Ipv4(_) => net_declare::fidl_subnet!("0.0.0.0/0"),
            fnet::IpAddress::Ipv6(_) => net_declare::fidl_subnet!("::/0"),
        };

        let newly_removed = self
            .remove_route_either(
                corresponding_default_subnet,
                Some(next_hop),
                fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            )
            .await?;

        if !newly_removed {
            Err(anyhow::anyhow!(
                "default route through {} via {next_hop:?} does not exist",
                self.id()
            ))
        } else {
            Ok(())
        }
    }

    /// Add a route to the given `destination` subnet via the given `next_hop`.
    pub async fn add_gateway_route(
        &self,
        destination: fnet::Subnet,
        next_hop: fnet::IpAddress,
    ) -> Result<()> {
        let newly_added = self
            .add_route_either(
                destination,
                Some(next_hop),
                fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
            )
            .await?;

        if !newly_added {
            Err(anyhow::anyhow!(
                "should have newly added route to {destination:?} via {next_hop:?} through {}",
                self.id()
            ))
        } else {
            Ok(())
        }
    }

    /// Create a root route set authenticated to manage routes through this interface.
    pub async fn create_authenticated_global_route_set<
        I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
    >(
        &self,
    ) -> Result<<I::RouteSetMarker as ProtocolMarker>::Proxy> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Out<'a, I: fnet_routes_ext::admin::FidlRouteAdminIpExt>(
            LocalBoxFuture<'a, <I::RouteSetMarker as ProtocolMarker>::Proxy>,
        );

        let Out(proxy_fut) = I::map_ip_out(
            self,
            |this| {
                Out(this
                    .get_global_route_set_v4()
                    .map(|result| result.expect("get global route set"))
                    .boxed_local())
            },
            |this| {
                Out(this
                    .get_global_route_set_v6()
                    .map(|result| result.expect("get global route set"))
                    .boxed_local())
            },
        );

        let route_set = proxy_fut.await;
        let fnet_resources::GrantForInterfaceAuthorization { interface_id, token } =
            self.get_authorization().await.expect("get interface grant");
        fnet_routes_ext::admin::authenticate_for_interface::<I>(
            &route_set,
            fnet_resources::ProofOfInterfaceAuthorization { interface_id, token },
        )
        .await
        .expect("authentication should not have FIDL error")
        .expect("authentication should succeed");
        Ok(route_set)
    }

    async fn get_global_route_set_v4(&self) -> Result<fnet_routes_admin::RouteSetV4Proxy> {
        let root_routes = self
            .realm
            .connect_to_protocol::<fnet_root::RoutesV4Marker>()
            .expect("get fuchsia.net.root.RoutesV4");
        let (route_set, server_end) =
            fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();
        root_routes.global_route_set(server_end).expect("calling global_route_set should succeed");
        Ok(route_set)
    }

    async fn get_global_route_set_v6(&self) -> Result<fnet_routes_admin::RouteSetV6Proxy> {
        let root_routes = self
            .realm
            .connect_to_protocol::<fnet_root::RoutesV6Marker>()
            .expect("get fuchsia.net.root.RoutesV6");
        let (route_set, server_end) =
            fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV6Marker>();
        root_routes.global_route_set(server_end).expect("calling global_route_set should succeed");
        Ok(route_set)
    }

    /// Gets the interface's properties with assigned addresses.
    async fn get_properties(
        &self,
        included_addresses: fnet_interfaces_ext::IncludedAddresses,
    ) -> Result<fnet_interfaces_ext::Properties<fnet_interfaces_ext::AllInterest>> {
        let interface_state = self.realm.connect_to_protocol::<fnet_interfaces::StateMarker>()?;
        let properties = fnet_interfaces_ext::existing(
            fnet_interfaces_ext::event_stream_from_state(
                &interface_state,
                fnet_interfaces_ext::WatchOptions { included_addresses, ..Default::default() },
            )?,
            fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(self.id),
        )
        .await
        .context("failed to get existing interfaces")?;
        match properties {
            fnet_interfaces_ext::InterfaceState::Unknown(id) => Err(anyhow::anyhow!(
                "could not find interface {} for endpoint {}",
                id,
                self.endpoint.name
            )),
            fnet_interfaces_ext::InterfaceState::Known(
                fnet_interfaces_ext::PropertiesAndState { properties, state: () },
            ) => Ok(properties),
        }
    }

    /// Gets the interface's addresses.
    pub async fn get_addrs(
        &self,
        included_addresses: fnet_interfaces_ext::IncludedAddresses,
    ) -> Result<Vec<fnet_interfaces_ext::Address<fnet_interfaces_ext::AllInterest>>> {
        let fnet_interfaces_ext::Properties { addresses, .. } =
            self.get_properties(included_addresses).await?;
        Ok(addresses)
    }

    /// Gets the interface's device name.
    pub async fn get_interface_name(&self) -> Result<String> {
        let fnet_interfaces_ext::Properties { name, .. } =
            self.get_properties(Default::default()).await?;
        Ok(name)
    }

    /// Gets the interface's port class.
    pub async fn get_port_class(&self) -> Result<fnet_interfaces_ext::PortClass> {
        let fnet_interfaces_ext::Properties { port_class, .. } =
            self.get_properties(Default::default()).await?;
        Ok(port_class)
    }

    /// Gets the interface's MAC address.
    pub async fn mac(&self) -> fnet::MacAddress {
        let (port, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_network::PortMarker>();
        self.get_port(server_end).expect("get_port");
        let (mac_addressing, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_network::MacAddressingMarker>();
        port.get_mac(server_end).expect("get_mac");
        mac_addressing.get_unicast_address().await.expect("get_unicast_address")
    }

    async fn set_dhcp_client_enabled(&self, enable: bool) -> Result<()> {
        self.connect_stack()
            .context("connect stack")?
            .set_dhcp_client_enabled(self.id, enable)
            .await
            .context("failed to call SetDhcpClientEnabled")?
            .map_err(|e| anyhow!("{:?}", e))
    }

    /// Starts DHCP on this interface.
    pub async fn start_dhcp<D: DhcpClient>(&self) -> Result<()> {
        match D::DHCP_CLIENT_VERSION {
            DhcpClientVersion::InStack => self.start_dhcp_in_stack().await,
            DhcpClientVersion::OutOfStack => self.start_dhcp_client_out_of_stack().await,
        }
    }

    async fn start_dhcp_in_stack(&self) -> Result<()> {
        self.set_dhcp_client_enabled(true).await.context("failed to start dhcp client")
    }

    async fn start_dhcp_client_out_of_stack(&self) -> Result<()> {
        let Self { endpoint: _, realm, id, control, device_control: _, dhcp_client_task } = self;
        let id = NonZeroU64::new(*id).expect("interface ID should be nonzero");
        let mut dhcp_client_task = dhcp_client_task.lock().await;
        let dhcp_client_task = dhcp_client_task.deref_mut();

        let provider = realm
            .connect_to_protocol::<fnet_dhcp::ClientProviderMarker>()
            .expect("get fuchsia.net.dhcp.ClientProvider");

        provider.check_presence().await.expect("check presence should succeed");

        let client = provider.new_client_ext(id, fnet_dhcp_ext::default_new_client_params());
        let control = control.clone();
        let route_set_provider = realm
            .connect_to_protocol::<fnet_routes_admin::RouteTableV4Marker>()
            .expect("get fuchsia.net.routes.RouteTableV4");
        let (route_set, server_end) =
            fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();
        route_set_provider.new_route_set(server_end).expect("calling new_route_set should succeed");
        let task = fnet_dhcp_ext::testutil::DhcpClientTask::new(client, id, route_set, control);
        *dhcp_client_task = Some(task);
        Ok(())
    }

    /// Stops DHCP on this interface.
    pub async fn stop_dhcp<D: DhcpClient>(&self) -> Result<()> {
        match D::DHCP_CLIENT_VERSION {
            DhcpClientVersion::InStack => self.stop_dhcp_in_stack().await,
            DhcpClientVersion::OutOfStack => {
                self.stop_dhcp_out_of_stack().await;
                Ok(())
            }
        }
    }

    async fn stop_dhcp_in_stack(&self) -> Result<()> {
        self.set_dhcp_client_enabled(false).await.context("failed to stop dhcp client")
    }

    async fn stop_dhcp_out_of_stack(&self) {
        let Self { endpoint: _, realm: _, id: _, control: _, device_control: _, dhcp_client_task } =
            self;
        let mut dhcp_client_task = dhcp_client_task.lock().await;
        if let Some(task) = dhcp_client_task.deref_mut().take() {
            task.shutdown().await.expect("client shutdown should succeed");
        }
    }

    /// Resolves when the out-of-stack DHCP client, if any, has shut down.
    pub async fn wait_dhcp_out_of_stack_stopped(&self) {
        let Self { endpoint: _, realm: _, id: _, control: _, device_control: _, dhcp_client_task } =
            self;
        let fut = {
            let guard = dhcp_client_task.lock().await;
            guard.as_ref().map(|task| task.wait_shutdown())
        };
        if let Some(fut) = fut {
            fut.await;
        }
    }

    /// Adds an address, and waits for its assignment state.
    pub async fn add_address_and_wait_until(
        &self,
        subnet: fnet::Subnet,
        state: fnet_interfaces::AddressAssignmentState,
    ) -> Result<()> {
        let (address_state_provider, server) =
            fidl::endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
        address_state_provider.detach().context("detach address lifetime")?;
        self.control
            .add_address(&subnet, &fnet_interfaces_admin::AddressParameters::default(), server)
            .context("FIDL error")?;

        let mut state_stream =
            fnet_interfaces_ext::admin::assignment_state_stream(address_state_provider);
        fnet_interfaces_ext::admin::wait_assignment_state(&mut state_stream, state).await?;
        Ok(())
    }

    /// Adds an address, waiting until the address assignment state is
    /// `ASSIGNED`.
    pub async fn add_address(&self, subnet: fnet::Subnet) -> Result<()> {
        self.add_address_and_wait_until(subnet, fnet_interfaces::AddressAssignmentState::Assigned)
            .await
    }

    /// Adds an address and a subnet route, waiting until the address assignment
    /// state is `ASSIGNED`.
    pub async fn add_address_and_subnet_route(&self, subnet: fnet::Subnet) -> Result<()> {
        let (address_state_provider, server) =
            fidl::endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
        address_state_provider.detach().context("detach address lifetime")?;
        self.control
            .add_address(
                &subnet,
                &fnet_interfaces_admin::AddressParameters {
                    add_subnet_route: Some(true),
                    ..Default::default()
                },
                server,
            )
            .context("FIDL error")?;

        let state_stream =
            fnet_interfaces_ext::admin::assignment_state_stream(address_state_provider);
        let mut state_stream = pin!(state_stream);

        fnet_interfaces_ext::admin::wait_assignment_state(
            &mut state_stream,
            fnet_interfaces::AddressAssignmentState::Assigned,
        )
        .await
        .context("assignment state")?;
        Ok(())
    }

    /// Removes an address and its corresponding subnet route.
    pub async fn del_address_and_subnet_route(
        &self,
        addr_with_prefix: fnet::Subnet,
    ) -> Result<bool> {
        let did_remove =
            self.control.remove_address(&addr_with_prefix).await.context("FIDL error").and_then(
                |res| {
                    res.map_err(|e: fnet_interfaces_admin::ControlRemoveAddressError| {
                        anyhow::anyhow!("{:?}", e)
                    })
                },
            )?;

        if did_remove {
            let destination = fnet_ext::apply_subnet_mask(addr_with_prefix);
            let newly_removed_route = self
                .remove_route_either(
                    destination,
                    None,
                    fnet_routes::SpecifiedMetric::InheritedFromInterface(fnet_routes::Empty),
                )
                .await?;

            // We don't assert on the route having been newly-removed because it could also
            // be removed due to the AddressStateProvider going away.
            let _: bool = newly_removed_route;
        }
        Ok(did_remove)
    }

    /// Removes all IPv6 LinkLocal addresses on the interface.
    ///
    /// Useful to purge the interface of autogenerated SLAAC addresses.
    pub async fn remove_ipv6_linklocal_addresses(
        &self,
    ) -> Result<Vec<fnet_interfaces_ext::Address<fnet_interfaces_ext::AllInterest>>> {
        let mut result = Vec::new();
        for address in self.get_addrs(fnet_interfaces_ext::IncludedAddresses::All).await? {
            let fnet_interfaces_ext::Address { addr: fnet::Subnet { addr, prefix_len }, .. } =
                &address;
            match addr {
                fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address { addr: _ }) => {
                    continue;
                }
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address { addr }) => {
                    let v6_addr = net_types::ip::Ipv6Addr::from_bytes(*addr);
                    if !v6_addr.is_unicast_link_local() {
                        continue;
                    }
                }
            }
            let _newly_removed: bool = self
                .del_address_and_subnet_route(fnet::Subnet { addr: *addr, prefix_len: *prefix_len })
                .await?;
            result.push(address);
        }
        Ok(result)
    }

    /// Set configuration on this interface.
    ///
    /// Returns an error if the operation is unsupported or a no-op.
    ///
    /// Note that this function should not be made public and should only be
    /// used to implement helpers for setting specific pieces of configuration,
    /// as it cannot be guaranteed that this function is kept up-to-date with
    /// the underlying FIDL types and thus may not always be able to uphold the
    /// error return contract.
    async fn set_configuration(&self, config: fnet_interfaces_admin::Configuration) -> Result<()> {
        let fnet_interfaces_admin::Configuration {
            ipv4: previous_ipv4, ipv6: previous_ipv6, ..
        } = self
            .control()
            .set_configuration(&config.clone())
            .await
            .context("FIDL error")?
            .map_err(|e| anyhow!("set configuration error: {:?}", e))?;

        fn verify_config_changed<T: Eq>(previous: Option<T>, current: Option<T>) -> Result<()> {
            if let Some(current) = current {
                let previous = previous.ok_or_else(|| anyhow!("configuration not supported"))?;
                if previous == current {
                    return Err(anyhow!("configuration change is a no-op"));
                }
            }
            Ok(())
        }

        let fnet_interfaces_admin::Configuration { ipv4, ipv6, .. } = config;
        if let Some(fnet_interfaces_admin::Ipv4Configuration {
            unicast_forwarding,
            multicast_forwarding,
            ..
        }) = ipv4
        {
            let fnet_interfaces_admin::Ipv4Configuration {
                unicast_forwarding: previous_unicast_forwarding,
                multicast_forwarding: previous_multicast_forwarding,
                ..
            } = previous_ipv4.ok_or_else(|| anyhow!("IPv4 configuration not supported"))?;
            verify_config_changed(previous_unicast_forwarding, unicast_forwarding)
                .context("IPv4 unicast forwarding")?;
            verify_config_changed(previous_multicast_forwarding, multicast_forwarding)
                .context("IPv4 multicast forwarding")?;
        }
        if let Some(fnet_interfaces_admin::Ipv6Configuration {
            unicast_forwarding,
            multicast_forwarding,
            ..
        }) = ipv6
        {
            let fnet_interfaces_admin::Ipv6Configuration {
                unicast_forwarding: previous_unicast_forwarding,
                multicast_forwarding: previous_multicast_forwarding,
                ..
            } = previous_ipv6.ok_or_else(|| anyhow!("IPv6 configuration not supported"))?;
            verify_config_changed(previous_unicast_forwarding, unicast_forwarding)
                .context("IPv6 unicast forwarding")?;
            verify_config_changed(previous_multicast_forwarding, multicast_forwarding)
                .context("IPv6 multicast forwarding")?;
        }
        Ok(())
    }

    /// Enable/disable IPv6 forwarding on this interface.
    pub async fn set_ipv6_forwarding_enabled(&self, enabled: bool) -> Result<()> {
        self.set_configuration(fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                unicast_forwarding: Some(enabled),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    }

    /// Enable/disable IPv4 forwarding on this interface.
    pub async fn set_ipv4_forwarding_enabled(&self, enabled: bool) -> Result<()> {
        self.set_configuration(fnet_interfaces_admin::Configuration {
            ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
                unicast_forwarding: Some(enabled),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    }

    /// Consumes this [`TestInterface`] and removes the associated interface
    /// in the Netstack, returning the device lifetime-carrying channels.
    pub async fn remove(
        self,
    ) -> Result<(fnetemul_network::EndpointProxy, Option<fnet_interfaces_admin::DeviceControlProxy>)>
    {
        let Self {
            endpoint: TestEndpoint { endpoint, name: _, _sandbox: _ },
            id: _,
            realm: _,
            control,
            device_control,
            dhcp_client_task: _,
        } = self;
        // For Network Devices, the `control` handle  is tied to the lifetime of
        // the interface; dropping it triggers interface removal in the
        // Netstack. For Ethernet devices this is a No-Op.
        std::mem::drop(control);
        Ok((endpoint, device_control))
    }

    /// Consumes this [`TestInterface`] and removes the underlying device. The
    /// Netstack will implicitly remove the interface and clients can expect to
    /// observe a `PEER_CLOSED` event on the returned control channel.
    pub fn remove_device(self) -> (Control, Option<fnet_interfaces_admin::DeviceControlProxy>) {
        let Self {
            endpoint: TestEndpoint { endpoint, name: _, _sandbox: _ },
            id: _,
            realm: _,
            control,
            device_control,
            dhcp_client_task: _,
        } = self;
        std::mem::drop(endpoint);
        (control, device_control)
    }

    /// Waits for this interface to signal that it's been removed.
    pub async fn wait_removal(self) -> Result<fnet_interfaces_admin::InterfaceRemovedReason> {
        let Self {
            // Keep this alive, we don't want to trigger removal.
            endpoint: _endpoint,
            id: _,
            realm: _,
            control,
            dhcp_client_task: _,
            // Keep this alive, we don't want to trigger removal.
            device_control: _device_control,
        } = self;
        match control.wait_termination().await {
            fnet_interfaces_ext::admin::TerminalError::Fidl(e) => {
                Err(e).context("waiting interface control termination")
            }
            fnet_interfaces_ext::admin::TerminalError::Terminal(reason) => Ok(reason),
        }
    }

    /// Sets the number of IPv6 DAD transmits on this interface.
    ///
    /// Returns the previous configuration value, if reported by the API.
    pub async fn set_ipv4_dad_transmits(&self, dad_transmits: u16) -> Result<Option<u16>> {
        set_ipv4_dad_transmits(self.control(), dad_transmits).await
    }

    /// Sets the number of IPv6 DAD transmits on this interface.
    ///
    /// Returns the previous configuration value, if reported by the API.
    pub async fn set_ipv6_dad_transmits(&self, dad_transmits: u16) -> Result<Option<u16>> {
        set_ipv6_dad_transmits(self.control(), dad_transmits).await
    }

    /// Sets whether temporary SLAAC address generation is enabled
    /// or disabled on this interface.
    pub async fn set_temporary_address_generation_enabled(&self, enabled: bool) -> Result<()> {
        set_temporary_address_generation_enabled(self.control(), enabled).await
    }
}

async fn set_ipv4_dad_transmits(control: &Control, dad_transmits: u16) -> Result<Option<u16>> {
    control
        .set_configuration(&fnet_interfaces_admin::Configuration {
            ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
                arp: Some(fnet_interfaces_admin::ArpConfiguration {
                    dad: Some(fnet_interfaces_admin::DadConfiguration {
                        transmits: Some(dad_transmits),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await?
        .map(|config| config.ipv4?.arp?.dad?.transmits)
        .map_err(|e| anyhow::anyhow!("set configuration error {e:?}"))
}

async fn set_ipv6_dad_transmits(control: &Control, dad_transmits: u16) -> Result<Option<u16>> {
    control
        .set_configuration(&fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    dad: Some(fnet_interfaces_admin::DadConfiguration {
                        transmits: Some(dad_transmits),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await?
        .map(|config| config.ipv6?.ndp?.dad?.transmits)
        .map_err(|e| anyhow::anyhow!("set configuration error {e:?}"))
}

async fn set_temporary_address_generation_enabled(control: &Control, enabled: bool) -> Result<()> {
    let _config: fnet_interfaces_admin::Configuration = control
        .set_configuration(&fnet_interfaces_admin::Configuration {
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    slaac: Some(fnet_interfaces_admin::SlaacConfiguration {
                        temporary_address: Some(enabled),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .context("FIDL error")?
        .map_err(|e| anyhow::anyhow!("set configuration error {e:?}"))?;
    Ok(())
}

/// Get the [`socket2::Domain`] for `addr`.
fn get_socket2_domain(addr: &std::net::SocketAddr) -> fposix_socket::Domain {
    let domain = match addr {
        std::net::SocketAddr::V4(_) => fposix_socket::Domain::Ipv4,
        std::net::SocketAddr::V6(_) => fposix_socket::Domain::Ipv6,
    };

    domain
}

/// Trait describing UDP sockets that can be bound in a testing realm.
pub trait RealmUdpSocket: Sized {
    /// Creates a UDP socket in `realm` bound to `addr`.
    fn bind_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;

    /// Creates a UDP socket in `realm` bound to `addr` with the given options.
    fn bind_in_realm_with_options<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        options: fposix_socket::SocketCreationOptions,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;
}

impl RealmUdpSocket for std::net::UdpSocket {
    fn bind_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        async move {
            let sock = realm
                .datagram_socket(
                    get_socket2_domain(&addr),
                    fposix_socket::DatagramSocketProtocol::Udp,
                )
                .await
                .context("failed to create socket")?;

            sock.bind(&addr.into()).context("bind failed")?;

            Result::Ok(sock.into())
        }
        .boxed_local()
    }

    fn bind_in_realm_with_options<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        options: fposix_socket::SocketCreationOptions,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        async move {
            let sock = realm
                .datagram_socket_with_options(
                    get_socket2_domain(&addr),
                    fposix_socket::DatagramSocketProtocol::Udp,
                    options,
                )
                .await
                .context("failed to create socket")?;

            sock.bind(&addr.into()).context("bind failed")?;

            Result::Ok(sock.into())
        }
        .boxed_local()
    }
}

impl RealmUdpSocket for fuchsia_async::net::UdpSocket {
    fn bind_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        std::net::UdpSocket::bind_in_realm(realm, addr)
            .and_then(|udp| {
                futures::future::ready(
                    fuchsia_async::net::UdpSocket::from_socket(udp)
                        .context("failed to create fuchsia_async socket"),
                )
            })
            .boxed_local()
    }

    fn bind_in_realm_with_options<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        options: fposix_socket::SocketCreationOptions,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        std::net::UdpSocket::bind_in_realm_with_options(realm, addr, options)
            .and_then(|udp| {
                futures::future::ready(
                    fuchsia_async::net::UdpSocket::from_socket(udp)
                        .context("failed to create fuchsia_async socket"),
                )
            })
            .boxed_local()
    }
}

/// Trait describing TCP listeners bound in a testing realm.
pub trait RealmTcpListener: Sized {
    /// Creates a TCP listener in `realm` bound to `addr`.
    fn listen_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        Self::listen_in_realm_with(realm, addr, |_: &socket2::Socket| Ok(()))
    }

    /// Creates a TCP listener by creating a Socket2 socket in `realm`. Closure `setup` is called
    /// with the reference of the socket before the socket is bound to `addr`.
    fn listen_in_realm_with<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        setup: impl FnOnce(&socket2::Socket) -> Result<()> + 'a,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;
}

impl RealmTcpListener for std::net::TcpListener {
    fn listen_in_realm_with<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        setup: impl FnOnce(&socket2::Socket) -> Result<()> + 'a,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        async move {
            let sock = realm
                .stream_socket(get_socket2_domain(&addr), fposix_socket::StreamSocketProtocol::Tcp)
                .await
                .context("failed to create server socket")?;
            setup(&sock)?;
            sock.bind(&addr.into()).context("failed to bind server socket")?;
            // Use 128 for the listen() backlog, same as the original implementation of TcpListener
            // in Rust std (see https://doc.rust-lang.org/src/std/sys_common/net.rs.html#386).
            sock.listen(128).context("failed to listen on server socket")?;

            Result::Ok(sock.into())
        }
        .boxed_local()
    }
}

impl RealmTcpListener for fuchsia_async::net::TcpListener {
    fn listen_in_realm_with<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        setup: impl FnOnce(&socket2::Socket) -> Result<()> + 'a,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        std::net::TcpListener::listen_in_realm_with(realm, addr, setup)
            .and_then(|listener| {
                futures::future::ready(
                    fuchsia_async::net::TcpListener::from_std(listener)
                        .context("failed to create fuchsia_async socket"),
                )
            })
            .boxed_local()
    }
}

/// Trait describing TCP streams in a testing realm.
pub trait RealmTcpStream: Sized {
    /// Creates a TCP stream in `realm` connected to `addr`.
    fn connect_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;

    /// Creates a TCP stream in `realm` bound to `local` and connected to `dst`.
    fn bind_and_connect_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        local: std::net::SocketAddr,
        dst: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;

    /// Creates a TCP stream in `realm` connected to `addr`.
    ///
    /// Closure `with_sock` is called with the reference of the socket before
    /// the socket is connected to `addr`.
    fn connect_in_realm_with_sock<'a, F: FnOnce(&socket2::Socket) -> Result + 'a>(
        realm: &'a TestRealm<'a>,
        dst: std::net::SocketAddr,
        with_sock: F,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;

    // TODO: Implement this trait for std::net::TcpStream.
}

impl RealmTcpStream for fuchsia_async::net::TcpStream {
    fn connect_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        Self::connect_in_realm_with_sock(realm, addr, |_: &socket2::Socket| Ok(()))
    }

    fn bind_and_connect_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        local: std::net::SocketAddr,
        dst: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        Self::connect_in_realm_with_sock(realm, dst, move |sock| {
            sock.bind(&local.into()).context("failed to bind")
        })
    }

    fn connect_in_realm_with_sock<'a, F: FnOnce(&socket2::Socket) -> Result + 'a>(
        realm: &'a TestRealm<'a>,
        dst: std::net::SocketAddr,
        with_sock: F,
    ) -> futures::future::LocalBoxFuture<'a, Result<fuchsia_async::net::TcpStream>> {
        async move {
            let sock = realm
                .stream_socket(get_socket2_domain(&dst), fposix_socket::StreamSocketProtocol::Tcp)
                .await
                .context("failed to create socket")?;

            with_sock(&sock)?;

            let stream = fuchsia_async::net::TcpStream::connect_from_raw(sock, dst)
                .context("failed to create client tcp stream")?
                .await
                .context("failed to connect to server")?;

            Result::Ok(stream)
        }
        .boxed_local()
    }
}

fn truncate_dropping_front(s: Cow<'_, str>, len: usize) -> Cow<'_, str> {
    match s.len().checked_sub(len) {
        None => s,
        Some(start) => {
            // NB: Drop characters from the front because it's likely that a name that
            // exceeds the length limit is the full name of a test whose suffix is more
            // informative because nesting of test cases appends suffixes.
            match s {
                Cow::Borrowed(s) => Cow::Borrowed(&s[start..]),
                Cow::Owned(mut s) => {
                    let _: std::string::Drain<'_> = s.drain(..start);
                    Cow::Owned(s)
                }
            }
        }
    }
}
