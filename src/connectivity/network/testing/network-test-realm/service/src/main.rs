// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, Result};
use assert_matches::assert_matches;
use async_utils::hanging_get::client::HangingGetStream;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_hardware_network::{self as fhwnet};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_dhcp_ext::{self as fnet_dhcp_ext, ClientProviderExt};
use fidl_fuchsia_net_dhcpv6 as fnet_dhcpv6;
use fidl_fuchsia_net_dhcpv6_ext as fnet_dhcpv6_ext;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_test_realm as fntr;
use fidl_fuchsia_posix_socket as fposix_socket;
use fidl_fuchsia_posix_socket_ext as fposix_socket_ext;
use fuchsia_async::{self as fasync, TimeoutExt as _};
use fuchsia_component::client::Service;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use futures_lite::FutureExt as _;
use log::{error, info, warn};
use net_types::ip::{Ipv4, Ipv6};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::convert::TryFrom as _;
use std::num::NonZeroU64;
use std::pin::pin;

/// URL for the realm that contains the hermetic network components with a
/// Netstack2 instance.
const HERMETIC_NETWORK_V2_URL: &'static str = "#meta/hermetic_network_v2.cm";

/// URL for the realm that contains the hermetic network components with a
/// Netstack3 instance.
const HERMETIC_NETWORK_V3_URL: &'static str = "#meta/hermetic_network_v3.cm";

/// Values for creating an interface on the hermetic Netstack.
///
/// Note that the topological path and the file path are not used by the
/// underlying Netstack. Consequently, fake values are defined here. Similarly,
/// the metric only needs to be a sensible value.
const DEFAULT_METRIC: u32 = 100;

/// Installs a netdevice with the provided `name` on the hermetic Netstack.
///
/// The `port_proxy` corresponds to a system netdevice.
async fn install_netdevice(
    name: &str,
    port_proxy: fhwnet::PortProxy,
    wait_any_ip_address: bool,
    connector: &HermeticNetworkConnector,
) -> Result<(), fntr::Error> {
    let port_id = port_proxy
        .get_info()
        .await
        .map_err(|e| {
            error!("failed to get port id: {:?}", e);
            fntr::Error::Internal
        })?
        .id
        .expect("must have a PortId");
    let installer = connector.connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>()?;
    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::DeviceControlMarker>();

    let (device, device_server_end) = fidl::endpoints::create_endpoints();
    port_proxy.get_device(device_server_end).map_err(|e| {
        error!("failed to get device: {:?}", e);
        fntr::Error::Internal
    })?;

    installer.install_device(device, device_control_server_end).map_err(|e| {
        error!("install_device failed: {:?}", e);
        fntr::Error::Internal
    })?;

    let (control, control_server_end) = fnet_interfaces_ext::admin::Control::create_endpoints()
        .map_err(|e| {
            error!("create_endpoints failed: {:?}", e);
            fntr::Error::Internal
        })?;

    device_control
        .create_interface(
            &port_id,
            control_server_end,
            fnet_interfaces_admin::Options {
                name: Some(name.to_string()),
                metric: Some(DEFAULT_METRIC),
                ..Default::default()
            },
        )
        .map_err(|e| {
            error!("create_interface failed: {:?}", e);
            fntr::Error::Internal
        })?;

    // Enable the interface that was newly added to the hermetic Netstack. It is
    // not enabled by default. Note that the `enable_interface` function could
    // be used here, but is not since using the `Control` type is simpler in
    // this case (no need to fetch the interface ID).
    let _did_enable = control
        .enable()
        .await
        .map_err(|e| match e {
            fnet_interfaces_ext::admin::TerminalError::Fidl(e) => {
                error!("enable interface failed: {:?}", e);
                fntr::Error::Internal
            }
            fnet_interfaces_ext::admin::TerminalError::Terminal(removed_reason) => {
                match removed_reason {
                    fnet_interfaces_admin::InterfaceRemovedReason::DuplicateName => {
                        fntr::Error::AlreadyExists
                    }
                    e => {
                        error!("interface removed: {:?}", e);
                        fntr::Error::Internal
                    }
                }
            }
        })?
        .map_err(|e| {
            error!("enable interface error: {:?}", e);
            fntr::Error::Internal
        })?;

    if wait_any_ip_address {
        wait_for_any_ip_address(
            control.get_id().await.map_err(|e| {
                error!("error getting ID of added interface: {:?}", e);
                fntr::Error::Internal
            })?,
            connector,
        )
        .await?;
    }

    // Extend the lifetime of the created interface beyond that of the `control`
    // and `device_control` types. Note that the lifetime of the created
    // interface is tied to the hermetic Netstack. That is, the interface will
    // be removed when the hermetic Netstack is shutdown.
    control.detach().map_err(|e| {
        error!("detatch failed for Control: {:?}", e);
        fntr::Error::Internal
    })?;
    device_control.detach().map_err(|e| {
        error!("detach failed for DeviceControl: {:?}", e);
        fntr::Error::Internal
    })
}

async fn wait_for_any_ip_address(
    id: u64,
    connector: &HermeticNetworkConnector,
) -> Result<(), fntr::Error> {
    let state_proxy = connector.connect_to_protocol::<fnet_interfaces::StateMarker>()?;
    let stream =
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &state_proxy,
            Default::default(),
        )
        .map_err(|e| {
            error!("failed to read interface stream: {:?}", e);
            fntr::Error::Internal
        })?;
    let addr = fnet_interfaces_ext::wait_interface_with_id(
        stream,
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(id),
        |properties_and_state| properties_and_state.properties.addresses.iter().next().cloned(),
    )
    .await
    .map_err(|e| {
        error!("error while waiting for autoconf IP address: {:?}", e);
        fntr::Error::Internal
    })?;
    info!("finished waiting for autoconf IP address once we saw {:?}", addr);
    Ok(())
}

/// Stops the device session on the system Netstack that corresponds to the
/// interface with the provided `mac_address`. As a result of stopping the
/// session, the interface(s) will be removed.
///
/// If an interface with the wanted MAC is not found on the system netstack,
/// then there would be no conflicting sessions, the function returns `Ok`.
async fn stop_device_session_on_system_netstack(
    expected_mac_address: &fnet_ext::MacAddress,
) -> Result<(), fntr::Error> {
    let state_proxy = SystemConnector.connect_to_protocol::<fnet_interfaces::StateMarker>()?;
    let stream =
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &state_proxy,
            Default::default(),
        )
        .map_err(|e| {
            error!("failed to read interface stream: {:?}", e);
            fntr::Error::Internal
        })?;

    let interfaces = fnet_interfaces_ext::existing(
        stream,
        HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .map_err(|e| {
        error!("failed to read existing interfaces: {:?}", e);
        fntr::Error::Internal
    })?;

    let root_interfaces_proxy =
        SystemConnector.connect_to_protocol::<fnet_root::InterfacesMarker>()?;

    let interfaces_stream = futures::stream::iter(interfaces.into_values());

    let results = interfaces_stream.filter_map(
        |fnet_interfaces_ext::PropertiesAndState {
             properties: fnet_interfaces_ext::Properties { id, .. },
             state: _,
         }| {
            let root_interfaces_proxy = &root_interfaces_proxy;
            async move {
                match root_interfaces_proxy.get_mac(id.get()).await {
                    Err(e) => {
                        let _: fidl::Error = e;
                        warn!("get_mac failure: {:?}", e);
                        None
                    }
                    Ok(result) => match result {
                        Err(fnet_root::InterfacesGetMacError::NotFound) => {
                            warn!("get_mac interface not found for ID: {}", id);
                            None
                        }
                        Ok(mac_address) => mac_address.and_then(|mac_address| {
                            (mac_address.octets == expected_mac_address.octets)
                                .then(move || id.get())
                        }),
                    },
                }
            }
        },
    );

    let mut results = pin!(results);

    let Some(interface_id) = results.next().await else {
        return Ok(());
    };

    let debug_interfaces =
        SystemConnector.connect_to_protocol::<fidl_fuchsia_net_debug::InterfacesMarker>()?;
    debug_interfaces
        .close_backing_session(interface_id)
        .await
        .map_err(|err| {
            error!("fidl error for removing backing device: {err:?}");
            fntr::Error::Internal
        })?
        .map_err(|err| {
            error!("cannot remove the backing device: {err:?}");
            fntr::Error::Internal
        })
}

/// Returns the first port that has `mac_address`.
///
/// If a port matching `mac_address` is not found, then an error is
/// returned.
async fn find_port(
    expected_mac_address: &fnet_ext::MacAddress,
) -> Result<fhwnet::PortProxy, fntr::Error> {
    let service = Service::open(fhwnet::ServiceMarker).map_err(|e| {
        error!("failed to open service: {:?}", e);
        fntr::Error::Internal
    })?;
    for instance in service.enumerate().await.map_err(|e| {
        error!("failed to enumerate devices: {:?}", e);
        fntr::Error::Internal
    })? {
        let device = instance.connect_to_device().map_err(|e| {
            error!("failed to connect to device: {:?}", e);
            fntr::Error::Internal
        })?;
        let (port_watcher, port_watcher_server_end) =
            fidl::endpoints::create_proxy::<fhwnet::PortWatcherMarker>();
        device.get_port_watcher(port_watcher_server_end).map_err(|e| {
            error!("failed to get port watcher: {:?}", e);
            fntr::Error::Internal
        })?;
        let port_stream = HangingGetStream::new(port_watcher, fhwnet::PortWatcherProxy::watch);
        let port_ids = port_stream
            .try_take_while(|event| {
                futures::future::ready(Ok(!matches!(event, fhwnet::DevicePortEvent::Idle(_))))
            })
            .map_ok(|event| {
                assert_matches!(event,
                    fhwnet::DevicePortEvent::Existing(port_id) => port_id)
            })
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                error!("cannot get existing ports on device: {e:?}");
                fntr::Error::Internal
            })?;

        for port_id in port_ids {
            let (port_proxy, port_proxy_server_end) =
                fidl::endpoints::create_proxy::<fhwnet::PortMarker>();
            device.get_port(&port_id, port_proxy_server_end).map_err(|e| {
                error!("failed to get port: {:?}", e);
                fntr::Error::Internal
            })?;
            let (mac_addressing_proxy, mac_addressing_server_end) =
                fidl::endpoints::create_proxy::<fhwnet::MacAddressingMarker>();
            port_proxy.get_mac(mac_addressing_server_end).map_err(|e| {
                error!("failed to get mac addressing: {:?}", e);
                fntr::Error::Internal
            })?;
            let mac = mac_addressing_proxy.get_unicast_address().await.map_err(|e| {
                error!("failed to get unicast address: {:?}", e);
                fntr::Error::Internal
            })?;
            if mac.octets == expected_mac_address.octets {
                return Ok(port_proxy);
            }
        }
    }
    return Err(fntr::Error::InterfaceNotFound);
}

/// Returns a `fnet_interfaces_admin::ControlProxy` that can be used to
/// manipulate the interface that has the provided `id`.
async fn connect_to_interface_admin_control(
    id: u64,
    connector: &impl Connector,
) -> Result<fnet_interfaces_ext::admin::Control, fntr::Error> {
    let root_interfaces_proxy = connector.connect_to_protocol::<fnet_root::InterfacesMarker>()?;
    let (control, server) =
        fnet_interfaces_ext::admin::Control::create_endpoints().map_err(|e| {
            error!("create_endpoints failure: {:?}", e);
            fntr::Error::Internal
        })?;
    root_interfaces_proxy.get_admin(id, server).map_err(|e| {
        error!("get_admin failure: {:?}", e);
        fntr::Error::Internal
    })?;
    Ok(control)
}

fn create_child_decl(child_name: &str, url: &str) -> fdecl::Child {
    fdecl::Child {
        name: Some(child_name.to_string()),
        url: Some(url.to_string()),
        // TODO(https://fxbug.dev/42171498): Remove the startup field when the
        // child is being created in a single_run collection. In such a case,
        // this field is currently required to be set to
        // `fdecl::StartupMode::Lazy` even though it is a no-op.
        startup: Some(fdecl::StartupMode::Lazy),
        ..Default::default()
    }
}

/// Creates a child component named `child_name` within the provided
/// `collection_name`.
///
/// The `url` corresponds to the URL of the component to add. The `connector`
/// connects to the desired realm.
async fn create_child(
    collection_ref: fdecl::CollectionRef,
    child: fdecl::Child,
    connector: &impl Connector,
) -> Result<(), fntr::Error> {
    let realm_proxy = connector.connect_to_protocol::<fcomponent::RealmMarker>()?;

    realm_proxy
        .create_child(&collection_ref, &child, fcomponent::CreateChildArgs::default())
        .await
        .map_err(|e| {
            error!("create_child failed: {:?}", e);
            fntr::Error::Internal
        })?
        .map_err(|e| {
            match e {
                // Variants that may be returned by the `CreateChild` method.
                fcomponent::Error::InstanceCannotResolve => fntr::Error::ComponentNotFound,
                fcomponent::Error::InstanceCannotUnresolve => fntr::Error::ComponentNotFound,
                fcomponent::Error::InvalidArguments => fntr::Error::InvalidArguments,
                fcomponent::Error::CollectionNotFound
                | fcomponent::Error::InstanceAlreadyExists
                | fcomponent::Error::InstanceDied
                | fcomponent::Error::ResourceUnavailable
                // Variants that are not returned by the `CreateChild` method.
                | fcomponent::Error::AccessDenied
                | fcomponent::Error::InstanceAlreadyStarted
                | fcomponent::Error::InstanceCannotStart
                | fcomponent::Error::InstanceNotFound
                | fcomponent::Error::Internal
                | fcomponent::Error::ResourceNotFound
                | fcomponent::Error::Unsupported
                | fcomponent::Error::DependencyCycle
                | fcomponent::ErrorUnknown!() => {
                    error!("create_child error: {:?}", e);
                    fntr::Error::Internal
                }
            }
        })
}

#[derive(thiserror::Error, Debug)]
enum DestroyChildError {
    #[error("Internal error")]
    Internal,
    #[error("Component not running")]
    NotRunning,
}

/// Destroys the child component that corresponds to `child_ref`.
///
/// The `connector` connects to the desired realm. A `not_running_error` will be
/// returned if the provided `child_ref` does not exist.
async fn destroy_child(
    child_ref: fdecl::ChildRef,
    connector: &impl Connector,
) -> Result<(), DestroyChildError> {
    let realm_proxy = connector
        .connect_to_protocol::<fcomponent::RealmMarker>()
        .map_err(|_e| DestroyChildError::Internal)?;

    realm_proxy
        .destroy_child(&child_ref)
        .await
        .map_err(|e| {
            error!("destroy_child failed: {:?}", e);
            DestroyChildError::Internal
        })?
        .map_err(|e| {
            match e {
            // Variants that may be returned by the `DestroyChild`
            // method. `CollectionNotFound` and `InstanceNotFound`
            // mean that the hermetic network realm does not exist. All
            // other errors are propagated as internal errors.
            fcomponent::Error::CollectionNotFound
            | fcomponent::Error::InstanceNotFound =>
                DestroyChildError::NotRunning,
            fcomponent::Error::InstanceDied
            | fcomponent::Error::InvalidArguments
            // Variants that are not returned by the `DestroyChild`
            // method.
            | fcomponent::Error::AccessDenied
            | fcomponent::Error::InstanceAlreadyExists
            | fcomponent::Error::InstanceAlreadyStarted
            | fcomponent::Error::InstanceCannotResolve
            | fcomponent::Error::InstanceCannotUnresolve
            | fcomponent::Error::InstanceCannotStart
            | fcomponent::Error::Internal
            | fcomponent::Error::ResourceNotFound
            | fcomponent::Error::ResourceUnavailable
            | fcomponent::Error::Unsupported
            | fcomponent::Error::DependencyCycle
            | fcomponent::ErrorUnknown!() => {
                error!("destroy_child error: {:?}", e);
                DestroyChildError::Internal
            }
    }
        })
}

async fn has_stub(connector: &impl Connector) -> Result<bool, fntr::Error> {
    let realm_proxy = connector.connect_to_protocol::<fcomponent::RealmMarker>()?;
    network_test_realm::has_stub(&realm_proxy).await.map_err(|e| {
        error!("failed to check for hermetic network realm: {:?}", e);
        fntr::Error::Internal
    })
}

/// A type that can connect to a FIDL protocol within a particular realm.
trait Connector {
    fn connect_to_protocol<P: fidl::endpoints::DiscoverableProtocolMarker>(
        &self,
    ) -> Result<P::Proxy, fntr::Error>;
}

/// Connects to protocols that are exposed to the Network Test Realm.
struct SystemConnector;

impl Connector for SystemConnector {
    fn connect_to_protocol<P: fidl::endpoints::DiscoverableProtocolMarker>(
        &self,
    ) -> Result<P::Proxy, fntr::Error> {
        fuchsia_component::client::connect_to_protocol::<P>().map_err(|e| {
            error!("failed to connect to {} with error: {:?}", P::PROTOCOL_NAME, e);
            fntr::Error::Internal
        })
    }
}

/// Connects to protocols within the hermetic-network realm.
struct HermeticNetworkConnector {
    child_directory: fio::DirectoryProxy,
}

impl HermeticNetworkConnector {
    async fn new() -> Result<Self, fntr::Error> {
        Ok(Self {
            child_directory: fuchsia_component::client::open_childs_exposed_directory(
                network_test_realm::HERMETIC_NETWORK_REALM_NAME.to_string(),
                Some(network_test_realm::HERMETIC_NETWORK_COLLECTION_NAME.to_string()),
            )
            .await
            .map_err(|e| {
                error!("open_childs_exposed_directory failed: {:?}", e);
                fntr::Error::Internal
            })?,
        })
    }
}

impl Connector for HermeticNetworkConnector {
    fn connect_to_protocol<P: fidl::endpoints::DiscoverableProtocolMarker>(
        &self,
    ) -> Result<P::Proxy, fntr::Error> {
        fuchsia_component::client::connect_to_protocol_at_dir_root::<P>(&self.child_directory)
            .map_err(|e| {
                error!("failed to connect to {} with error: {:?}", P::PROTOCOL_NAME, e);
                fntr::Error::Internal
            })
    }
}

async fn create_socket(
    domain: fposix_socket::Domain,
    protocol: fposix_socket::DatagramSocketProtocol,
    connector: &HermeticNetworkConnector,
) -> Result<socket2::Socket, fntr::Error> {
    let socket_provider = connector.connect_to_protocol::<fposix_socket::ProviderMarker>()?;

    fposix_socket_ext::datagram_socket(&socket_provider, domain, protocol)
        .await
        .map_err(|e| {
            error!("datagram_socket failed: {:?}", e);
            fntr::Error::Internal
        })?
        .map_err(|e| {
            error!("datagram_socket error: {:?}", e);
            fntr::Error::Internal
        })
}

async fn create_icmp_socket(
    domain: fposix_socket::Domain,
    connector: &HermeticNetworkConnector,
) -> Result<fasync::net::DatagramSocket, fntr::Error> {
    Ok(fasync::net::DatagramSocket::new_from_socket(
        create_socket(domain, fposix_socket::DatagramSocketProtocol::IcmpEcho, connector).await?,
    )
    .map_err(|e| {
        error!("new_from_socket failed: {:?}", e);
        fntr::Error::Internal
    })?)
}

async fn bind_udp_socket(
    domain: fposix_socket::Domain,
    connector: &HermeticNetworkConnector,
) -> Result<fasync::net::UdpSocket, fntr::Error> {
    let socket =
        create_socket(domain, fposix_socket::DatagramSocketProtocol::Udp, connector).await?;
    let address: std::net::SocketAddr = (
        match domain {
            fposix_socket::Domain::Ipv4 => std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            fposix_socket::Domain::Ipv6 => std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        },
        0,
    )
        .into();
    socket.bind(&address.into()).map_err(|e| {
        error!("error binding socket to address {:?}: {:?}", address, e);
        fntr::Error::Internal
    })?;
    Ok(fasync::net::UdpSocket::from_socket(socket.into()).map_err(|e| {
        error!("error converting socket to fuchsia_async::net::UdpSocket: {:?}", e);
        fntr::Error::Internal
    })?)
}

/// Returns the scope ID needed for the provided `address`.
///
/// See https://tools.ietf.org/html/rfc2553#section-3.3 for more information.
async fn get_interface_scope_id(
    interface_name: &Option<String>,
    address: &std::net::Ipv6Addr,
    connector: &impl Connector,
) -> Result<u32, fntr::Error> {
    const DEFAULT_SCOPE_ID: u32 = 0;
    let is_link_local_address =
        net_types::ip::Ipv6Addr::from_bytes(address.octets()).is_unicast_link_local();

    match (interface_name, is_link_local_address) {
        // If a link-local address is specified, then an interface name
        // must be provided.
        (None, true) => Err(fntr::Error::InvalidArguments),
        // The default scope ID should be used for any non link-local
        // address.
        (Some(_), false) | (None, false) => Ok(DEFAULT_SCOPE_ID),
        (Some(interface_name), true) => network_test_realm::get_interface_id(
            &interface_name,
            &connector.connect_to_protocol::<fnet_interfaces::StateMarker>()?,
        )
        .await
        .map_err(|e| {
            error!(
                "failed to obtain interface ID for interface named: {} with error: {:?}",
                interface_name, e
            );
            fntr::Error::Internal
        })?
        .ok_or(fntr::Error::InterfaceNotFound)
        .and_then(|id| {
            u32::try_from(id).map_err(|e| {
                error!("failed to convert interface ID to u32, {:?}", e);
                fntr::Error::Internal
            })
        }),
    }
}

/// Sends a single ICMP echo request to `address`.
async fn ping_once<Ip: ping::FuchsiaIpExt>(
    address: Ip::SockAddr,
    payload_length: usize,
    interface_name: Option<String>,
    timeout: zx::MonotonicDuration,
    connector: &HermeticNetworkConnector,
) -> Result<(), fntr::Error> {
    let socket = create_icmp_socket(Ip::DOMAIN_FIDL, connector).await?;

    if let Some(interface_name) = interface_name {
        socket.bind_device(Some(interface_name.as_bytes())).map_err(|e| match e.kind() {
            std::io::ErrorKind::InvalidInput => fntr::Error::InterfaceNotFound,
            _ if e.raw_os_error() == Some(libc::ENODEV) => fntr::Error::InterfaceNotFound,
            _kind => {
                error!("bind_device for interface: {} failed: {:?}", interface_name, e);
                fntr::Error::Internal
            }
        })?;
    }

    // The body of the packet is filled with `payload_length` 0 bytes.
    let payload: Vec<u8> = std::iter::repeat(0).take(payload_length).collect();

    let (mut sink, stream) = ping::new_unicast_sink_and_stream::<Ip, _, { u16::MAX as usize }>(
        &socket, &address, &payload,
    );

    // Address clippy::large_futures lint by boxing the `PingStream`, which
    // takes up around `u16::MAX` bytes on the stack.
    // See https://rust-lang.github.io/rust-clippy/stable/index.html#/large_futures.
    let mut stream = Box::pin(stream);

    const SEQ: u16 = 1;
    sink.send(SEQ).await.map_err(|e| {
        warn!("failed to send ping: {:?}", e);
        match e {
            ping::PingError::Send(error) => match error.kind() {
                // `InvalidInput` corresponds to an oversized `payload_length`.
                std::io::ErrorKind::InvalidInput => fntr::Error::InvalidArguments,
                // TODO(https://github.com/rust-lang/rust/issues/86442): Consider
                // defining more granular error codes once the relevant
                // `std::io::Error` variants are stable (e.g. `HostUnreachable`,
                // `NetworkUnreachable`, etc.).
                _kind => fntr::Error::PingFailed,
            },
            ping::PingError::Body { .. }
            | ping::PingError::Parse
            | ping::PingError::Recv(_)
            | ping::PingError::ReplyCode(_)
            | ping::PingError::ReplyType { .. }
            | ping::PingError::SendLength { .. } => fntr::Error::PingFailed,
        }
    })?;

    if timeout.into_nanos() <= 0 {
        return Ok(());
    }

    match stream.try_next().map(Some).on_timeout(timeout, || None).await {
        None => Err(fntr::Error::TimeoutExceeded),
        Some(Err(e)) => {
            warn!("failed to receive ping response: {:?}", e);
            Err(fntr::Error::PingFailed)
        }
        Some(Ok(None)) => {
            error!("ping reply stream ended unexpectedly");
            Err(fntr::Error::Internal)
        }
        Some(Ok(Some(got))) if got == SEQ => Ok(()),
        Some(Ok(Some(got))) => {
            error!("received unexpected ping sequence number; got: {}, want: {}", got, SEQ);
            Err(fntr::Error::PingFailed)
        }
    }
}

/// Creates a socket if `socket` is None. Otherwise, returns a reference to the
/// `socket` value.
///
/// If a socket is created, then the value of `socket` is replaced with the
/// newly created socket.
async fn get_or_insert_socket<'a>(
    socket: &'a mut Option<socket2::Socket>,
    domain: fposix_socket::Domain,
    connector: &'a HermeticNetworkConnector,
) -> Result<&'a socket2::Socket, fntr::Error> {
    match socket {
        None => Ok(socket.insert(
            create_socket(domain, fposix_socket::DatagramSocketProtocol::Udp, connector).await?,
        )),
        Some(value) => Ok(value),
    }
}

/// A controller for creating and manipulating the Network Test Realm.
///
/// The Network Test Realm corresponds to a hermetic network realm with a
/// Netstack under test. The `Controller` is responsible for configuring
/// this realm, the Netstack under test, and the system's Netstack. Once
/// configured, the controller is expected to yield the following component
/// topology:
///
/// ```
///            network-test-realm (this controller component)
///                    |
///             enclosed-network (a collection)
///                    |
///             hermetic-network
///            /       |        \
///      netstack     ...      stubs (a collection)
///                              |
///                          test-stub (a configurable component)
/// ```
///
/// This topology enables the `Controller` to interact with the system's
/// Netstack and the Netstack in the "hermetic-network" realm. Additionally, it
/// enables capabilities to be routed from the hermetic Netstack to siblings
/// (via the "hermetic-network" parent) while remaining isolated from the rest
/// of the system.
struct Controller {
    /// Connector to access protocols within the hermetic-network realm. If the
    /// hermetic-network realm does not exist, then this will be `None`.
    hermetic_network_connector: Option<HermeticNetworkConnector>,

    /// Socket for joining and leaving Ipv4 multicast groups. This field is
    /// lazily instantiated the first time an Ipv4 multicast group is joined or
    /// left. Note that the lifetime of Ipv4 multicast memberships (those added
    /// via the `join_multicast_group` method) are tied to this field.
    multicast_v4_socket: Option<socket2::Socket>,

    /// Socket for joining and leaving Ipv6 multicast groups. This field is
    /// lazily instantiated the first time an Ipv6 multicast group is joined or
    /// left. Note that the lifetime of Ipv6 multicast memberships (those added
    /// via the `join_multicast_group` method) are tied to this field.
    multicast_v6_socket: Option<socket2::Socket>,

    /// Stream of DHCPv6 client watcher events.
    dhcpv6_client_stream_map: async_utils::stream::StreamMap<
        u64,
        futures::stream::BoxStream<'static, (u64, Result<fnet_dhcpv6_ext::WatchItem, fidl::Error>)>,
    >,

    // Task used to interact with the out-of-stack DHCP client.
    dhcp_client_tasks: HashMap<NonZeroU64, fnet_dhcp_ext::testutil::DhcpClientTask>,
}

impl Controller {
    fn new() -> Self {
        Self {
            hermetic_network_connector: None,
            multicast_v4_socket: None,
            multicast_v6_socket: None,
            dhcpv6_client_stream_map: async_utils::stream::StreamMap::empty(),
            dhcp_client_tasks: HashMap::new().into(),
        }
    }

    async fn handle_request(
        &mut self,
        request: fntr::ControllerRequest,
    ) -> Result<(), fidl::Error> {
        match request {
            fntr::ControllerRequest::StartHermeticNetworkRealm { netstack, responder } => {
                let result = self.start_hermetic_network_realm(netstack).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::StopHermeticNetworkRealm { responder } => {
                let result = self.stop_hermetic_network_realm().await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::AddInterface {
                mac_address,
                name,
                responder,
                wait_any_ip_address,
            } => {
                let mac_address = fnet_ext::MacAddress::from(mac_address);
                let result = self.add_interface(mac_address, &name, wait_any_ip_address).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::StartStub { component_url, responder } => {
                let result = self.start_stub(&component_url).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::StopStub { responder } => {
                let result = self.stop_stub().await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::PollUdp {
                target,
                payload,
                timeout,
                num_retries,
                responder,
            } => {
                let mut rx_buffer = vec![0; fntr::MAX_UDP_POLL_LENGTH.into()];
                let fnet_ext::SocketAddress(target) = target.into();
                let result = self
                    .poll_udp(
                        target,
                        &payload,
                        zx::MonotonicDuration::from_nanos(timeout),
                        num_retries,
                        &mut rx_buffer,
                    )
                    .await
                    .map(|num_bytes| &rx_buffer[..num_bytes]);
                responder.send(result)?;
            }
            fntr::ControllerRequest::Ping {
                target,
                payload_length,
                interface_name,
                timeout,
                responder,
            } => {
                let result = self
                    .ping(
                        target,
                        payload_length,
                        interface_name,
                        zx::MonotonicDuration::from_nanos(timeout),
                    )
                    .await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::JoinMulticastGroup { address, interface_id, responder } => {
                let result = self.join_multicast_group(address, interface_id).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::LeaveMulticastGroup { address, interface_id, responder } => {
                let result = self.leave_multicast_group(address, interface_id).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::StartDhcpv6Client { params, responder } => {
                let result = self.start_dhcpv6_client(params).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::StopDhcpv6Client { responder } => {
                let result = if self.dhcpv6_client_stream_map.inner_mut().drain().count() == 0 {
                    Err(fntr::Error::Dhcpv6ClientNotRunning)
                } else {
                    Ok(())
                };
                responder.send(result)?;
            }
            fntr::ControllerRequest::StartOutOfStackDhcpv4Client {
                payload: fntr::ControllerStartOutOfStackDhcpv4ClientRequest { interface_id, .. },
                responder,
            } => {
                let result = self.start_dhcpv4_client_out_of_stack(interface_id).await;
                responder.send(result)?;
            }
            fntr::ControllerRequest::StopOutOfStackDhcpv4Client {
                payload: fntr::ControllerStopOutOfStackDhcpv4ClientRequest { interface_id, .. },
                responder,
            } => {
                let result = self.stop_dhcpv4_out_of_stack(interface_id).await;
                responder.send(result)?;
            }
        }
        Ok(())
    }

    async fn start_dhcpv4_client_out_of_stack(
        &mut self,
        id: Option<u64>,
    ) -> Result<(), fntr::Error> {
        let Self {
            hermetic_network_connector,
            multicast_v4_socket: _,
            multicast_v6_socket: _,
            dhcpv6_client_stream_map: _,
            dhcp_client_tasks,
        } = self;
        let id = id.ok_or(fntr::Error::InvalidArguments)?;
        let id = NonZeroU64::new(id).ok_or(fntr::Error::InvalidArguments)?;
        let vacant_entry = match dhcp_client_tasks.entry(id) {
            Entry::Occupied(_) => return Err(fntr::Error::InvalidArguments),
            Entry::Vacant(entry) => entry,
        };

        let hermetic_network_connector = hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        let provider =
            hermetic_network_connector.connect_to_protocol::<fnet_dhcp::ClientProviderMarker>()?;
        provider
            .check_presence()
            .await
            .expect("dhcp client should be included in hermetic network realm");

        let client = provider.new_client_ext(id, fnet_dhcp_ext::default_new_client_params());
        let control =
            connect_to_interface_admin_control(id.into(), hermetic_network_connector).await?;
        let route_set_provider = hermetic_network_connector
            .connect_to_protocol::<fnet_routes_admin::RouteTableV4Marker>()?;
        let (route_set, server_end) =
            fidl::endpoints::create_proxy::<fnet_routes_admin::RouteSetV4Marker>();
        route_set_provider.new_route_set(server_end).expect("calling new_route_set should succeed");
        let poll_task =
            fnet_dhcp_ext::testutil::DhcpClientTask::new(client, id, route_set, control);
        let _: &mut fnet_dhcp_ext::testutil::DhcpClientTask = vacant_entry.insert(poll_task);
        Ok(())
    }

    async fn stop_dhcpv4_out_of_stack(&mut self, id: Option<u64>) -> Result<(), fntr::Error> {
        let Self {
            hermetic_network_connector: _,
            multicast_v4_socket: _,
            multicast_v6_socket: _,
            dhcpv6_client_stream_map: _,
            dhcp_client_tasks,
        } = self;
        let id = id.ok_or(fntr::Error::InvalidArguments)?;
        let id = NonZeroU64::new(id).ok_or(fntr::Error::InvalidArguments)?;
        dhcp_client_tasks
            .remove(&id)
            .ok_or(fntr::Error::Dhcpv4ClientNotRunning)?
            .shutdown()
            .await
            .map_err(|err| {
                error!("failed to shutdown client with err {}", err);
                fntr::Error::Dhcpv4ClientShutdownFailed
            })
    }

    /// Returns the `socket2::Socket` that should be used join or leave
    /// multicast groups for `address`.
    ///
    /// Creates a socket on the first invocation for the relevant IP version.
    /// Subsequent invocations return the existing socket.
    async fn get_or_create_multicast_socket(
        &mut self,
        address: std::net::IpAddr,
    ) -> Result<&socket2::Socket, fntr::Error> {
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        match address {
            std::net::IpAddr::V4(_) => {
                get_or_insert_socket(
                    &mut self.multicast_v4_socket,
                    fposix_socket::Domain::Ipv4,
                    hermetic_network_connector,
                )
                .await
            }
            std::net::IpAddr::V6(_) => {
                get_or_insert_socket(
                    &mut self.multicast_v6_socket,
                    fposix_socket::Domain::Ipv6,
                    hermetic_network_connector,
                )
                .await
            }
        }
    }

    /// Leaves the multicast group `address` using the provided `interface_id`.
    async fn leave_multicast_group(
        &mut self,
        address: fnet::IpAddress,
        interface_id: u64,
    ) -> Result<(), fntr::Error> {
        let fnet_ext::IpAddress(address) = address.into();
        let interface_id = u32::try_from(interface_id).map_err(|e| {
            error!("failed to convert interface ID to u32, {:?}", e);
            fntr::Error::Internal
        })?;
        let socket = self.get_or_create_multicast_socket(address).await?;

        match address {
            std::net::IpAddr::V4(addr) => socket.leave_multicast_v4_n(
                &addr,
                &socket2::InterfaceIndexOrAddress::Index(interface_id),
            ),
            std::net::IpAddr::V6(addr) => socket.leave_multicast_v6(&addr, interface_id),
        }
        .map_err(|e| match e.kind() {
            // The group `address` was not previously joined.
            std::io::ErrorKind::AddrNotAvailable => fntr::Error::AddressNotAvailable,
            // The specified `interface_id` does not exist or the `address`
            // does not correspond to a valid multicast address.
            std::io::ErrorKind::InvalidInput => fntr::Error::InvalidArguments,
            _kind => {
                error!("leave_multicast_group failed: {:?}", e);
                fntr::Error::Internal
            }
        })
    }

    /// Joins the multicast group `address` using the provided `interface_id`.
    async fn join_multicast_group(
        &mut self,
        address: fnet::IpAddress,
        interface_id: u64,
    ) -> Result<(), fntr::Error> {
        let fnet_ext::IpAddress(address) = address.into();
        let interface_id = u32::try_from(interface_id).map_err(|e| {
            error!("failed to convert interface ID to u32, {:?}", e);
            fntr::Error::Internal
        })?;
        let socket = self.get_or_create_multicast_socket(address).await?;

        match address {
            std::net::IpAddr::V4(addr) => socket
                .join_multicast_v4_n(&addr, &socket2::InterfaceIndexOrAddress::Index(interface_id)),
            std::net::IpAddr::V6(addr) => socket.join_multicast_v6(&addr, interface_id),
        }
        .map_err(|e| match e.kind() {
            // The group `address` was already joined.
            std::io::ErrorKind::AddrInUse => fntr::Error::AddressInUse,
            // The specified `interface_id` does not exist or the `address`
            // does not correspond to a valid multicast address.
            std::io::ErrorKind::InvalidInput => fntr::Error::InvalidArguments,
            _kind => {
                error!("join_multicast_group failed: {:?}", e);
                fntr::Error::Internal
            }
        })
    }

    async fn start_dhcpv6_client(
        &mut self,
        params @ fnet_dhcpv6::NewClientParams {
            interface_id,
            address: _,
            config: _,
            ..
        }: fnet_dhcpv6::NewClientParams,
    ) -> Result<(), fntr::Error> {
        let interface_id = interface_id.ok_or(fntr::Error::InvalidArguments)?;
        if self.dhcpv6_client_stream_map.contains_key(&interface_id) {
            return Err(fntr::Error::AlreadyExists);
        }
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        let client_provider = hermetic_network_connector
            .connect_to_protocol::<fnet_dhcpv6::ClientProviderMarker>()?;
        let (client_proxy, client_server_end) =
            fidl::endpoints::create_proxy::<fnet_dhcpv6::ClientMarker>();
        client_provider.new_client(&params, client_server_end).map_err(|e| {
            error!("failed to start DHCPv6 client: {}", e);
            fntr::Error::Internal
        })?;
        if let Some(_) = self.dhcpv6_client_stream_map.insert(
            interface_id,
            Box::pin(
                fnet_dhcpv6_ext::into_watch_stream(client_proxy).map(move |v| (interface_id, v)),
            ),
        ) {
            unreachable!(
                "already verified that no DHCPv6 client is running on interface {}",
                interface_id
            );
        }
        Ok(())
    }

    /// Starts a test stub within the hermetic-network realm.
    async fn start_stub(&self, component_url: &str) -> Result<(), fntr::Error> {
        // Stubs exist only within the hermetic-network realm. Therefore,
        // the hermetic-network realm must exist.
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        if has_stub(hermetic_network_connector).await? {
            // The `Controller` only configures one test stub at a time. As a
            // result, any existing stub must be stopped before a new one is
            // started.
            self.stop_stub().await.map_err(|e| match e {
                fntr::Error::StubNotRunning => {
                    error!("attempted to stop stub that was not running");
                    fntr::Error::Internal
                }
                fntr::Error::AddressInUse
                | fntr::Error::AddressUnreachable
                | fntr::Error::AddressNotAvailable
                | fntr::Error::AlreadyExists
                | fntr::Error::ComponentNotFound
                | fntr::Error::HermeticNetworkRealmNotRunning
                | fntr::Error::Internal
                | fntr::Error::InterfaceNotFound
                | fntr::Error::InvalidArguments
                | fntr::Error::PingFailed
                | fntr::Error::TimeoutExceeded
                | fntr::Error::Dhcpv6ClientNotRunning
                | fntr::Error::Dhcpv4ClientNotRunning
                | fntr::Error::Dhcpv4ClientShutdownFailed => e,
            })?;
        }

        create_child(
            fdecl::CollectionRef { name: network_test_realm::STUB_COLLECTION_NAME.to_string() },
            create_child_decl(network_test_realm::STUB_COMPONENT_NAME, component_url),
            hermetic_network_connector,
        )
        .await
    }

    /// Stops the test stub within the hermetic-network realm.
    async fn stop_stub(&self) -> Result<(), fntr::Error> {
        // Stubs exist only within the hermetic-network realm. Therefore,
        // the hermetic-network realm must exist.
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        destroy_child(network_test_realm::create_stub_child_ref(), hermetic_network_connector)
            .await
            .map_err(|e| match e {
                DestroyChildError::Internal => fntr::Error::Internal,
                DestroyChildError::NotRunning => fntr::Error::StubNotRunning,
            })
    }

    async fn poll_udp(
        &self,
        target: std::net::SocketAddr,
        payload: &[u8],
        timeout: zx::MonotonicDuration,
        num_retries: u16,
        rx_buffer: &mut [u8],
    ) -> Result<usize, fntr::Error> {
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        let socket = bind_udp_socket(
            match &target {
                std::net::SocketAddr::V4(_) => fposix_socket::Domain::Ipv4,
                std::net::SocketAddr::V6(_) => fposix_socket::Domain::Ipv6,
            },
            hermetic_network_connector,
        )
        .await?;

        let socket = &socket;

        let fold_result = async_utils::fold::try_fold_while(
            futures::stream::iter(0..num_retries)
                .then(|_| socket.send_to(payload, target))
                .map_err(|e| {
                    // TODO(https://github.com/rust-lang/rust/issues/86442): once
                    // std::io::ErrorKind::HostUnreachable is stable, we should use that instead.
                    match e.raw_os_error() {
                        // TODO(https://fxbug.dev/42051708): Return ENETUNREACH when no route found in
                        // Netstack2.
                        Some(libc::EHOSTUNREACH) | Some(libc::ENETUNREACH) => {
                            fntr::Error::AddressUnreachable
                        }
                        Some(_) | None => {
                            error!("error while sending udp datagram to {:?}: {:?}", target, e);
                            fntr::Error::Internal
                        }
                    }
                }),
            rx_buffer,
            |rx_buffer, num_bytes_sent| async move {
                if num_bytes_sent < payload.len() {
                    error!(
                        "expected to send full payload length {}, sent {} bytes instead",
                        payload.len(),
                        num_bytes_sent
                    );
                    return Err(fntr::Error::Internal);
                }

                let timelimited_socket_receive = socket
                    .recv_from(rx_buffer)
                    .map(Ok)
                    .or(fasync::Timer::new(timeout).map(Err))
                    .await;

                match timelimited_socket_receive
                {
                    Ok(received_result) => {
                        let (received, from_addr) = received_result.map_err(|e| {
                            error!("error while receiving udp datagram: {:?}", e);
                            fntr::Error::Internal
                        })?;
                        if from_addr != target {
                            warn!(
                                "received udp datagram from {:?} while listening for datagrams\
                                 from {:?}",
                                from_addr,
                                target,
                            );
                            return Ok(async_utils::fold::FoldWhile::Continue(rx_buffer));
                        }
                        Ok(async_utils::fold::FoldWhile::Done(received))
                    }
                    Err((/* timed out */)) => {
                        Ok(async_utils::fold::FoldWhile::Continue(rx_buffer))
                    }
                }
            },
        )
        .await?;

        match fold_result {
            async_utils::fold::FoldResult::StreamEnded(_rx_buffer) => {
                Err(fntr::Error::TimeoutExceeded)
            }
            async_utils::fold::FoldResult::ShortCircuited(num_bytes_received) => {
                Ok(num_bytes_received)
            }
        }
    }

    /// Pings the `target` using a socket created on the hermetic Netstack.
    async fn ping(
        &self,
        target: fnet::IpAddress,
        payload_length: u16,
        interface_name: Option<String>,
        timeout: zx::MonotonicDuration,
    ) -> Result<(), fntr::Error> {
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        let fnet_ext::IpAddress(target) = target.into();
        const UNSPECIFIED_PORT: u16 = 0;
        match target {
            std::net::IpAddr::V4(addr) => {
                ping_once::<Ipv4>(
                    std::net::SocketAddrV4::new(addr, UNSPECIFIED_PORT),
                    payload_length.into(),
                    interface_name,
                    timeout,
                    hermetic_network_connector,
                )
                .await
            }
            std::net::IpAddr::V6(addr) => {
                const DEFAULT_FLOW_INFO: u32 = 0;
                let scope_id =
                    get_interface_scope_id(&interface_name, &addr, hermetic_network_connector)
                        .await?;
                ping_once::<Ipv6>(
                    std::net::SocketAddrV6::new(
                        addr,
                        UNSPECIFIED_PORT,
                        DEFAULT_FLOW_INFO,
                        scope_id,
                    ),
                    payload_length.into(),
                    interface_name,
                    timeout,
                    hermetic_network_connector,
                )
                .await
            }
        }
    }

    /// Adds an interface to the hermetic Netstack.
    ///
    /// An interface will only be added if the system has an interface with a
    /// matching `mac_address`. The added interface will have the provided
    /// `name`. Additionally, the backing session of the matching interface will
    /// be closed on the system's Netstack, resulting in its removal.
    async fn add_interface(
        &mut self,
        mac_address: fnet_ext::MacAddress,
        name: &str,
        wait_any_ip_address: bool,
    ) -> Result<(), fntr::Error> {
        // A hermetic Netstack must be running for an interface to be
        // added.
        let hermetic_network_connector = self
            .hermetic_network_connector
            .as_ref()
            .ok_or(fntr::Error::HermeticNetworkRealmNotRunning)?;

        stop_device_session_on_system_netstack(&mac_address).await?;
        let port = find_port(&mac_address).await?;
        install_netdevice(name, port, wait_any_ip_address, hermetic_network_connector).await?;
        Ok(())
    }

    /// Tears down the "hermetic-network" realm.
    async fn stop_hermetic_network_realm(&mut self) -> Result<(), fntr::Error> {
        destroy_child(
            network_test_realm::create_hermetic_network_realm_child_ref(),
            &SystemConnector,
        )
        .await
        .map_err(|e| match e {
            DestroyChildError::NotRunning => {
                self.hermetic_network_connector = None;
                fntr::Error::HermeticNetworkRealmNotRunning
            }
            DestroyChildError::Internal => fntr::Error::Internal,
        })?;

        self.hermetic_network_connector = None;
        self.multicast_v4_socket = None;
        self.multicast_v6_socket = None;
        self.dhcpv6_client_stream_map.inner_mut().clear();
        Ok(())
    }

    /// Starts the "hermetic-network" realm with the provided `netstack`.
    ///
    /// Adds the "hermetic-network" component to the "enclosed-network"
    /// collection.
    async fn start_hermetic_network_realm(
        &mut self,
        netstack: fntr::Netstack,
    ) -> Result<(), fntr::Error> {
        if let Some(_hermetic_network_connector) = &self.hermetic_network_connector {
            // The `Controller` only configures one hermetic network realm
            // at a time. As a result, any existing realm must be stopped before
            // a new one is started.
            self.stop_hermetic_network_realm().await.map_err(|e| match e {
                fntr::Error::HermeticNetworkRealmNotRunning => {
                    panic!("attempted to stop hermetic network realm that was not running")
                }
                fntr::Error::AddressInUse
                | fntr::Error::AddressNotAvailable
                | fntr::Error::AddressUnreachable
                | fntr::Error::AlreadyExists
                | fntr::Error::ComponentNotFound
                | fntr::Error::Internal
                | fntr::Error::InterfaceNotFound
                | fntr::Error::InvalidArguments
                | fntr::Error::PingFailed
                | fntr::Error::StubNotRunning
                | fntr::Error::TimeoutExceeded
                | fntr::Error::Dhcpv6ClientNotRunning
                | fntr::Error::Dhcpv4ClientNotRunning
                | fntr::Error::Dhcpv4ClientShutdownFailed => e,
            })?;
        }

        let url = match netstack {
            fntr::Netstack::V2 => HERMETIC_NETWORK_V2_URL,
            fntr::Netstack::V3 => HERMETIC_NETWORK_V3_URL,
        };

        create_child(
            fdecl::CollectionRef {
                name: network_test_realm::HERMETIC_NETWORK_COLLECTION_NAME.to_string(),
            },
            create_child_decl(network_test_realm::HERMETIC_NETWORK_REALM_NAME, url),
            &SystemConnector,
        )
        .await?;

        self.hermetic_network_connector = Some(HermeticNetworkConnector::new().await?);
        Ok(())
    }
}

async fn time_skew_watchdog() {
    const TICK: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(1);
    const WARN_THRESHOLD: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(2);
    let mut timer = pin!(fasync::Timer::new(fasync::MonotonicInstant::now()));
    (&mut timer).await;
    loop {
        let now = fasync::MonotonicInstant::now();
        timer.as_mut().reset(now + TICK);
        (&mut timer).await;
        let later = fasync::MonotonicInstant::now();
        let delta = later - now;
        if delta >= WARN_THRESHOLD {
            warn!(
                "timer skew watchdog observed {}ms, expected {}ms",
                delta.into_millis(),
                TICK.into_millis()
            );
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = fuchsia_component::server::ServiceFs::new_local();
    let _: &mut fuchsia_component::server::ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service(|s: fntr::ControllerRequestStream| s);

    let _: &mut fuchsia_component::server::ServiceFs<_> =
        fs.take_and_serve_directory_handle().context("failed to serve ServiceFs directory")?;

    let mut controller = Controller::new();

    let mut requests = fs.fuse().flatten_unordered(None);

    enum Event {
        ControllerRequest(Option<Result<fntr::ControllerRequest, fidl::Error>>),
        Dhcpv6ClientWatchItem(Option<(u64, Result<fnet_dhcpv6_ext::WatchItem, fidl::Error>)>),
    }

    let _watchdog_task = fasync::Task::spawn(time_skew_watchdog());

    loop {
        let event = futures::select! {
            request_item = requests.next() => {
                Event::ControllerRequest(request_item)
            }
            dhcpv6_client_watch_item = controller.dhcpv6_client_stream_map.next() => {
                Event::Dhcpv6ClientWatchItem(dhcpv6_client_watch_item)
            }
        };
        match event {
            Event::ControllerRequest(controller_request) => {
                let controller_request = controller_request.expect("stopped serving requests");
                match futures::future::ready(controller_request)
                    .and_then(|req| controller.handle_request(req))
                    .await
                {
                    Ok(()) => {}
                    Err(e) => {
                        if !fidl::Error::is_closed(&e) {
                            error!("handle_request failed: {:?}", e);
                        } else {
                            warn!("handle_request closed: {:?}", e);
                        }
                    }
                }
            }
            Event::Dhcpv6ClientWatchItem(opt) => {
                let (interface_id, watch_item): (u64, _) =
                    opt.expect("DHCPv6 client streams must not terminate");
                match watch_item {
                    Ok(event) => {
                        error!("handling of DHCPv6 client events is unimplemented: {:?}", event);
                    }
                    Err(e) => {
                        error!("DHCPv6 client on interface {} error: {}", interface_id, e);
                    }
                }
            }
        }
    }
}
