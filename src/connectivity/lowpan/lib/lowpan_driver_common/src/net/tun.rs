// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! LoWPAN Network Tunnel Abstraction
use super::debug::*;
use super::iface::*;
use crate::prelude_internal::*;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

use crate::spinel::Subnet;
use anyhow::Error;
use async_trait::async_trait;
use fidl::endpoints::{create_endpoints, create_proxy};
use fidl_fuchsia_hardware_network as fhwnet;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnetext;
use fidl_fuchsia_net_interfaces_admin as fnetifadmin;
use fidl_fuchsia_net_interfaces_ext as fnetifext;
use fidl_fuchsia_net_routes as fnetroutes;
use fidl_fuchsia_net_routes_admin as fnetroutesadmin;
use fidl_fuchsia_net_tun as ftun;
use fuchsia_async::net::DatagramSocket;
use fuchsia_component::client::{connect_channel_to_protocol, connect_to_protocol};
use fuchsia_sync::Mutex;
use futures::stream::BoxStream;
use net_types::ip::{Ip as _, Ipv6};
use socket2::{Domain, Protocol};
const TUN_PORT_ID: u8 = 0;

#[derive(Debug)]
pub struct TunNetworkInterface {
    tun_dev: ftun::DeviceProxy,
    tun_port: ftun::PortProxy,
    #[allow(unused)]
    // TODO(https://fxbug.dev/42143339): use `control` after converting methods to async.
    control: fnetifext::admin::Control,
    control_sync: Mutex<fnetifadmin::ControlSynchronousProxy>,
    route_set_v4_sync: Mutex<fnetroutesadmin::RouteSetV4SynchronousProxy>,
    route_set_v6_sync: Mutex<fnetroutesadmin::RouteSetV6SynchronousProxy>,
    routes: Mutex<HashMap<fnet::Subnet, HashSet<std::net::Ipv6Addr>>>,
    mcast_socket: DatagramSocket,
    id: u64,
}

/// Error types for RouteSet and interface authentication.
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteAdminError {
    /// An error returned by route set manipulation methods.
    #[error("RouteSet error: {0:?}")]
    RouteSet(fnetroutesadmin::RouteSetError),
    /// An error returned by interface authentication method.
    #[error("AuthenticateForInterface error: {0:?}")]
    AuthenticateForInterface(fnetroutesadmin::AuthenticateForInterfaceError),
    /// Route already exists in the route set.
    #[error("Route already exists")]
    AlreadyExists,
    /// Route was not found in the route set.
    #[error("Route not found")]
    NotFound,
}

// A macro is used here instead of a generic function because the generated V4 and V6
// synchronous FIDL proxies do not share common traits. Using a macro avoids writing
// considerable custom trait boilerplate.
macro_rules! init_route_set {
    ($routes_marker:ty, $routes_proxy:ty, $route_set_proxy:ty, $proof:expr, $name:expr) => {{
        let (routes_client, routes_server) = zx::Channel::create();
        connect_channel_to_protocol::<$routes_marker>(routes_server)
            .context(format!("failed to connect to RouteTable{}", $name))?;
        let routes_sync = <$routes_proxy>::new(routes_client);

        let (route_set_client, route_set_server) = zx::Channel::create();
        routes_sync
            .new_route_set(route_set_server.into())
            .context(format!("failed to call new_route_set {}", $name))?;
        let route_set_sync = <$route_set_proxy>::new(route_set_client);
        route_set_sync
            .authenticate_for_interface($proof, zx::MonotonicInstant::INFINITE)
            .context(format!("failed to call authenticate {}", $name))?
            .map_err(RouteAdminError::AuthenticateForInterface)
            .context(format!("failed to authenticate {}", $name))?;
        route_set_sync
    }};
}

impl TunNetworkInterface {
    pub async fn try_new(name: Option<String>) -> Result<TunNetworkInterface, Error> {
        let tun_control = connect_to_protocol::<ftun::ControlMarker>()?;

        let (tun_dev, req) = create_proxy::<ftun::DeviceMarker>();

        tun_control
            .create_device(&ftun::DeviceConfig { blocking: Some(true), ..Default::default() }, req)
            .context("failed to create tun pair")?;

        let (tun_port, port_req) = create_proxy::<ftun::PortMarker>();
        tun_dev
            .add_port(
                &ftun::DevicePortConfig {
                    base: Some(ftun::BasePortConfig {
                        id: Some(TUN_PORT_ID),
                        mtu: Some(Ipv6::MINIMUM_LINK_MTU.get()),
                        rx_types: Some(vec![fhwnet::FrameType::Ipv6, fhwnet::FrameType::Ipv4]),
                        tx_types: Some(vec![
                            fhwnet::FrameTypeSupport {
                                type_: fhwnet::FrameType::Ipv6,
                                features: fhwnet::FRAME_FEATURES_RAW,
                                supported_flags: fhwnet::TxFlags::empty(),
                            },
                            fhwnet::FrameTypeSupport {
                                type_: fhwnet::FrameType::Ipv4,
                                features: fhwnet::FRAME_FEATURES_RAW,
                                supported_flags: fhwnet::TxFlags::empty(),
                            },
                        ]),
                        port_class: Some(fhwnet::PortClass::Lowpan),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                port_req,
            )
            .context("failed to add device port")?;

        let (device, device_req) = create_endpoints::<fhwnet::DeviceMarker>();

        tun_dev.get_device(device_req).context("get device failed")?;

        let (control, control_sync) = {
            let installer = connect_to_protocol::<fnetifadmin::InstallerMarker>()?;
            let (device_control, server_end) = create_proxy::<fnetifadmin::DeviceControlMarker>();
            installer.install_device(device, server_end).context("install_device failed")?;
            // Interface lifetime is already tied to us because of tun device,
            // no need to keep this extra channel around.
            device_control.detach().context("device control detach failed")?;

            let (port, server_end) = create_proxy::<fhwnet::PortMarker>();
            tun_port.get_port(server_end).context("get_port failed")?;
            let port_id = port
                .get_info()
                .await
                .context("get_info failed")?
                .id
                .ok_or_else(|| anyhow::anyhow!("port id missing from info"))?;

            let (control_sync_client_channel, control_sync_server) = zx::Channel::create();
            let control_sync =
                fnetifadmin::ControlSynchronousProxy::new(control_sync_client_channel);
            device_control
                .create_interface(
                    &port_id,
                    control_sync_server.into(),
                    fnetifadmin::Options { name: name.clone(), ..Default::default() },
                )
                .context("create_interface failed")?;

            let (control, server_end) = fnetifext::admin::Control::create_endpoints()?;
            device_control
                .create_interface(
                    &port_id,
                    server_end,
                    fnetifadmin::Options { name, ..Default::default() },
                )
                .context("create_interface failed")?;

            (control, Mutex::new(control_sync))
        };

        let id =
            control_sync.lock().get_id(zx::MonotonicInstant::INFINITE).context("get_id failed")?;
        let _was_disabled: bool = control_sync
            .lock()
            .enable(zx::MonotonicInstant::INFINITE)
            .context("enable error")?
            .map_err(|e| anyhow::anyhow!("enable failed {:?}", e))?;

        let grant = control_sync
            .lock()
            .get_authorization_for_interface(zx::MonotonicInstant::INFINITE)
            .context("failed to get authorization for interface")?;
        let proof_v4 = fnetifext::admin::proof_from_grant(&grant);
        let proof_v6 = fnetifext::admin::proof_from_grant(&grant);

        let route_set_v4_sync = init_route_set!(
            fnetroutesadmin::RouteTableV4Marker,
            fnetroutesadmin::RouteTableV4SynchronousProxy,
            fnetroutesadmin::RouteSetV4SynchronousProxy,
            proof_v4,
            "V4"
        );

        let route_set_v6_sync = init_route_set!(
            fnetroutesadmin::RouteTableV6Marker,
            fnetroutesadmin::RouteTableV6SynchronousProxy,
            fnetroutesadmin::RouteSetV6SynchronousProxy,
            proof_v6,
            "V6"
        );

        let mcast_socket =
            DatagramSocket::new(Domain::IPV6, Some(Protocol::UDP)).expect("DatagramSocket::new()");

        Ok(TunNetworkInterface {
            tun_dev,
            tun_port,
            control,
            control_sync,
            route_set_v4_sync: Mutex::new(route_set_v4_sync),
            route_set_v6_sync: Mutex::new(route_set_v6_sync),
            mcast_socket,
            routes: Mutex::new(HashMap::new()),
            id,
        })
    }
}

#[async_trait]
impl NetworkInterface for TunNetworkInterface {
    fn get_index(&self) -> u64 {
        self.id
    }

    fn get_nicid(&self) -> NonZeroU64 {
        NonZeroU64::new(self.id).expect("TUN interface ID is guaranteed to be non-zero")
    }

    async fn outbound_packet_from_stack(&self) -> Result<Vec<u8>, Error> {
        let frame = self
            .tun_dev
            .read_frame()
            .await
            .context("FIDL error on read_frame")?
            .map_err(zx::Status::err_from_raw)
            .context("Error calling read_frame")?;

        if let Some(packet) = frame.data.as_ref() {
            trace!("TunNetworkInterface: Packet arrived from stack: {:?}", Ipv6PacketDebug(packet));
        }

        #[allow(clippy::or_fun_call)]
        Ok(frame.data.ok_or(format_err!("data field was absent"))?)
    }

    async fn inbound_packet_to_stack(
        &self,
        packet: &[u8],
        frame_type: fhwnet::FrameType,
    ) -> Result<(), Error> {
        trace!("TunNetworkInterface: Packet sent to stack: {:?}", Ipv6PacketDebug(packet));

        Ok(self
            .tun_dev
            .write_frame(&ftun::Frame {
                port: Some(TUN_PORT_ID),
                frame_type: Some(frame_type),
                data: Some(packet.to_vec()),
                meta: None,
                ..Default::default()
            })
            .await?
            .map_err(zx::Status::err_from_raw)?)
    }

    async fn set_online(&self, online: bool) -> Result<(), Error> {
        info!("TunNetworkInterface: Interface online: {:?}", online);

        if online {
            self.tun_port.set_online(true).await?;
            let _was_disabled: bool = self
                .control_sync
                .lock()
                .enable(zx::MonotonicInstant::INFINITE)
                .context("enable error")?
                .map_err(|e| anyhow::anyhow!("enable failed {:?}", e))?;
        } else {
            self.tun_port.set_online(false).await?;
        }

        Ok(())
    }

    async fn set_enabled(&self, enabled: bool) -> Result<(), Error> {
        info!("TunNetworkInterface: Interface enabled: {:?}", enabled);
        if enabled {
            let _was_disabled: bool = self
                .control_sync
                .lock()
                .enable(zx::MonotonicInstant::INFINITE)
                .context("enable error")?
                .map_err(|e| anyhow::anyhow!("enable failed {:?}", e))?;
        } else {
            let _was_enabled: bool = self
                .control_sync
                .lock()
                .disable(zx::MonotonicInstant::INFINITE)
                .context("disable error")?
                .map_err(|e| anyhow::anyhow!("disable failed {:?}", e))?;
        }
        Ok(())
    }

    fn add_address_from_spinel_subnet(&self, addr: &Subnet) -> Result<(), Error> {
        let device_addr = fnet::Subnet {
            addr: fnetext::IpAddress(addr.addr.into()).into(),
            prefix_len: addr.prefix_len,
        };
        self.add_address(device_addr)
    }

    fn add_address(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        info!("TunNetworkInterface: Adding Address: {:?}", addr);
        let (address_state_provider, server_end) = fidl::endpoints::create_proxy::<
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
        >();
        address_state_provider.detach()?;

        let device_addr = addr;
        self.control_sync.lock().add_address(
            &device_addr,
            &fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
            server_end,
        )?;

        info!("TunNetworkInterface: Successfully added address {:?}", addr);

        self.add_forwarding_entry(device_addr)
    }

    fn remove_address_from_spinel_subnet(&self, addr: &Subnet) -> Result<(), Error> {
        let device_addr = fnet::Subnet {
            addr: fnetext::IpAddress(addr.addr.into()).into(),
            prefix_len: addr.prefix_len,
        };
        self.remove_address(device_addr)
    }

    fn remove_address(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        info!("TunNetworkInterface: Removing Address: {:?}", addr);

        self.control_sync
            .lock()
            .remove_address(&addr, zx::MonotonicInstant::INFINITE)?
            .expect("control_sync.remove_address");

        info!("TunNetworkInterface: Successfully removed address {:?}", addr);

        self.remove_forwarding_entry(addr)
    }

    fn add_forwarding_entry(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        let subnet = fnetext::apply_subnet_mask(addr);

        match subnet.addr {
            fidl_fuchsia_net::IpAddress::Ipv4(addr) => {
                let route = fnetroutes::RouteV4 {
                    destination: fnet::Ipv4AddressWithPrefix {
                        addr,
                        prefix_len: subnet.prefix_len,
                    },
                    action: fnetroutes::RouteActionV4::Forward(fnetroutes::RouteTargetV4 {
                        outbound_interface: self.id,
                        next_hop: None,
                    }),
                    properties: fnetroutes::RoutePropertiesV4 {
                        specified_properties: Some(fnetroutes::SpecifiedRouteProperties {
                            metric: Some(fnetroutes::SpecifiedMetric::InheritedFromInterface(
                                fnetroutes::Empty,
                            )),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                };
                let did_add = self
                    .route_set_v4_sync
                    .lock()
                    .add_route(&route, zx::MonotonicInstant::INFINITE)
                    .context("failed to call add_route IPv4")?
                    .map_err(RouteAdminError::RouteSet)
                    .context("failed to add IPv4 forwarding entry")?;
                if !did_add {
                    return Err(anyhow::Error::new(RouteAdminError::AlreadyExists));
                }
            }
            fidl_fuchsia_net::IpAddress::Ipv6(addr) => {
                let mut routes = self.routes.lock();
                if let Some(addresses) = routes.get_mut(&subnet) {
                    addresses.insert(addr.addr.into());
                } else {
                    let route = fnetroutes::RouteV6 {
                        destination: fnet::Ipv6AddressWithPrefix {
                            addr,
                            prefix_len: subnet.prefix_len,
                        },
                        action: fnetroutes::RouteActionV6::Forward(fnetroutes::RouteTargetV6 {
                            outbound_interface: self.id,
                            next_hop: None,
                        }),
                        properties: fnetroutes::RoutePropertiesV6 {
                            specified_properties: Some(fnetroutes::SpecifiedRouteProperties {
                                metric: Some(fnetroutes::SpecifiedMetric::InheritedFromInterface(
                                    fnetroutes::Empty,
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    };
                    let did_add = self
                        .route_set_v6_sync
                        .lock()
                        .add_route(&route, zx::MonotonicInstant::INFINITE)
                        .context("failed to call add_route IPv6")?
                        .map_err(RouteAdminError::RouteSet)
                        .context("failed to add IPv6 forwarding entry")?;
                    if !did_add {
                        return Err(anyhow::Error::new(RouteAdminError::AlreadyExists));
                    }
                    routes.insert(subnet, HashSet::from([addr.addr.into()]));
                    info!(
                        "TunNetworkInterface: Successfully added forwarding entry for {}",
                        std::net::Ipv6Addr::from(addr.addr)
                    );
                }
            }
        };
        Ok(())
    }

    fn remove_forwarding_entry(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        let subnet = fnetext::apply_subnet_mask(addr);

        match subnet.addr {
            fidl_fuchsia_net::IpAddress::Ipv4(addr) => {
                let route = fnetroutes::RouteV4 {
                    destination: fnet::Ipv4AddressWithPrefix {
                        addr,
                        prefix_len: subnet.prefix_len,
                    },
                    action: fnetroutes::RouteActionV4::Forward(fnetroutes::RouteTargetV4 {
                        outbound_interface: self.id,
                        next_hop: None,
                    }),
                    properties: fnetroutes::RoutePropertiesV4 {
                        specified_properties: Some(fnetroutes::SpecifiedRouteProperties {
                            metric: Some(fnetroutes::SpecifiedMetric::InheritedFromInterface(
                                fnetroutes::Empty,
                            )),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                };

                let did_remove = self
                    .route_set_v4_sync
                    .lock()
                    .remove_route(&route, zx::MonotonicInstant::INFINITE)
                    .context("failed to call remove_route IPv4")?
                    .map_err(RouteAdminError::RouteSet)
                    .context("failed to remove IPv4 forwarding entry")?;
                if !did_remove {
                    return Err(anyhow::Error::new(RouteAdminError::NotFound));
                }
            }
            fidl_fuchsia_net::IpAddress::Ipv6(addr) => {
                let mut routes = self.routes.lock();
                if let Some(addresses) = routes.get_mut(&subnet) {
                    addresses.remove(&std::net::Ipv6Addr::from(addr.addr));
                    if addresses.is_empty() {
                        routes.remove(&subnet);

                        let route = fnetroutes::RouteV6 {
                            destination: fnet::Ipv6AddressWithPrefix {
                                addr,
                                prefix_len: subnet.prefix_len,
                            },
                            action: fnetroutes::RouteActionV6::Forward(fnetroutes::RouteTargetV6 {
                                outbound_interface: self.id,
                                next_hop: None,
                            }),
                            properties: fnetroutes::RoutePropertiesV6 {
                                specified_properties: Some(fnetroutes::SpecifiedRouteProperties {
                                    metric: Some(
                                        fnetroutes::SpecifiedMetric::InheritedFromInterface(
                                            fnetroutes::Empty,
                                        ),
                                    ),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        };

                        let did_remove = self
                            .route_set_v6_sync
                            .lock()
                            .remove_route(&route, zx::MonotonicInstant::INFINITE)
                            .context("failed to call remove_route IPv6")?
                            .map_err(RouteAdminError::RouteSet)
                            .context("failed to remove IPv6 forwarding entry")?;
                        if !did_remove {
                            return Err(anyhow::Error::new(RouteAdminError::NotFound));
                        }
                        info!(
                            "TunNetworkInterface: Successfully removed forwarding entry for {}",
                            std::net::Ipv6Addr::from(addr.addr)
                        );
                    }
                }
            }
        }

        Ok(())
    }

    fn add_external_route(&self, addr: &Subnet) -> Result<(), Error> {
        let subnet_addr: std::net::Ipv6Addr = addr.addr;
        let subnet_length = addr.prefix_len;
        info!(
            "TunNetworkInterface: Adding external route: {} with prefix length {} \
            (CURRENTLY IGNORED)",
            subnet_addr, subnet_length
        );
        Ok(())
    }

    fn remove_external_route(&self, addr: &Subnet) -> Result<(), Error> {
        let subnet_addr: std::net::Ipv6Addr = addr.addr;
        let subnet_length = addr.prefix_len;
        info!(
            "TunNetworkInterface: Removing external route: {} with prefix length {} \
            (CURRENTLY IGNORED)",
            subnet_addr, subnet_length
        );
        Ok(())
    }

    /// Has the interface join the given multicast group.
    fn join_mcast_group(&self, addr: &std::net::Ipv6Addr) -> Result<(), Error> {
        info!("TunNetworkInterface: Joining multicast group: {}", addr);
        self.mcast_socket.as_ref().join_multicast_v6(addr, self.id.try_into().unwrap())?;
        Ok(())
    }

    /// Has the interface leave the given multicast group.
    fn leave_mcast_group(&self, addr: &std::net::Ipv6Addr) -> Result<(), Error> {
        info!("TunNetworkInterface: Leaving multicast group: {}", addr);
        self.mcast_socket.as_ref().leave_multicast_v6(addr, self.id.try_into().unwrap())?;
        Ok(())
    }

    fn take_event_stream(&self) -> BoxStream<'_, Result<NetworkInterfaceEvent, Error>> {
        let enabled_stream = futures::stream::try_unfold((), move |()| async move {
            loop {
                if let ftun::InternalState { has_session: Some(has_session), .. } =
                    self.tun_port.watch_state().await?
                {
                    break Ok(Some((
                        NetworkInterfaceEvent::InterfaceEnabledChanged(has_session),
                        (),
                    )));
                }
            }
        });

        use fidl_fuchsia_net_interfaces::*;
        use std::convert::TryInto;

        struct EventState {
            prev_prop: Properties,
            watcher: Option<WatcherProxy>,
            next_events: Vec<NetworkInterfaceEvent>,
        }
        let init_state = EventState {
            prev_prop: Properties::default(),
            watcher: None,
            next_events: Vec::default(),
        };

        let if_event_stream = futures::stream::try_unfold(init_state, move |mut state| {
            async move {
                if state.watcher.is_none() {
                    let fnif_state = connect_to_protocol::<StateMarker>()?;
                    let (watcher, req) = create_proxy::<WatcherMarker>();
                    fnif_state.get_watcher(&WatcherOptions::default(), req)?;
                    state.watcher = Some(watcher);
                }

                loop {
                    // Flush out any pending events
                    if let Some(event) = state.next_events.pop() {
                        return Ok(Some((event, state)));
                    }

                    match state.watcher.as_ref().unwrap().watch().await? {
                        Event::Existing(prop) if prop.id == Some(self.id) => {
                            assert!(
                                state.prev_prop.id.is_none(),
                                "Got Event::Existing twice for same interface"
                            );
                            state.prev_prop = prop;
                            continue;
                        }
                        Event::Idle(_) => {
                            if state.prev_prop.id.is_none() {
                                return Err(format_err!("Interface no longer exists"));
                            }
                        }
                        Event::Removed(id) if id == self.id => return Ok(None),

                        Event::Changed(prop) if prop.id == Some(self.id) => {
                            assert!(state.prev_prop.id.is_some());

                            traceln!("TunNetworkInterface: Got Event::Changed({:#?})", prop);

                            if let Some(addrs) = prop.addresses.as_ref() {
                                let empty_addrs = vec![];
                                let prev_addrs =
                                    state.prev_prop.addresses.as_ref().unwrap_or(&empty_addrs);
                                state.next_events.extend(
                                    addrs.iter().filter(|x| !prev_addrs.contains(x)).filter_map(
                                        |Address { addr, valid_until: _, .. }| {
                                            addr.unwrap()
                                                .try_into()
                                                .ok()
                                                .map(NetworkInterfaceEvent::AddressWasAdded)
                                        },
                                    ),
                                );
                                state.next_events.extend(
                                    prev_addrs.iter().filter(|x| !addrs.contains(x)).filter_map(
                                        |Address { addr, valid_until: _, .. }| {
                                            addr.unwrap()
                                                .try_into()
                                                .ok()
                                                .map(NetworkInterfaceEvent::AddressWasRemoved)
                                        },
                                    ),
                                );
                            }

                            traceln!(
                                "TunNetworkInterface: Queued events: {:#?}",
                                state.next_events
                            );

                            state.prev_prop = prop;
                        }

                        _ => continue,
                    }
                }
            }
        });

        futures::stream::select(enabled_stream, if_event_stream).boxed()
    }

    async fn set_ipv6_forwarding_enabled(&self, enabled: bool) -> Result<(), Error> {
        // Ignore the configuration before our change was applied.
        let _: fnetifadmin::Configuration = self
            .control_sync
            .lock()
            .set_configuration(
                &fnetifadmin::Configuration {
                    ipv6: Some(fnetifadmin::Ipv6Configuration {
                        unicast_forwarding: Some(enabled),
                        multicast_forwarding: Some(enabled),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                zx::MonotonicInstant::INFINITE,
            )
            .map_err(anyhow::Error::new)
            .and_then(|res| {
                res.map_err(|e: fnetifadmin::ControlSetConfigurationError| {
                    anyhow::anyhow!("{:?}", e)
                })
            })
            .context("set configuration")?;

        Ok(())
    }

    async fn set_ipv4_forwarding_enabled(&self, enabled: bool) -> Result<(), Error> {
        // Ignore the configuration before our change was applied.
        let _: fnetifadmin::Configuration = self
            .control_sync
            .lock()
            .set_configuration(
                &fnetifadmin::Configuration {
                    ipv4: Some(fnetifadmin::Ipv4Configuration {
                        unicast_forwarding: Some(enabled),
                        multicast_forwarding: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                zx::MonotonicInstant::INFINITE,
            )
            .map_err(anyhow::Error::new)
            .and_then(|res| {
                res.map_err(|e: fnetifadmin::ControlSetConfigurationError| {
                    anyhow::anyhow!("{:?}", e)
                })
            })
            .context("set configuration")?;

        Ok(())
    }
}
