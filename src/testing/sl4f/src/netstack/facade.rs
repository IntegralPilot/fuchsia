// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use component_debug::dirs::{OpenDirType, connect_to_instance_protocol};
use fidl_fuchsia_net_stackmigrationdeprecated as fnet_stack_migration;
use once_cell::sync::OnceCell;
use serde::Serialize;
use serde_json::Value;

fn serialize_ipv4<S: serde::Serializer>(
    addresses: &Vec<std::net::Ipv4Addr>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(addresses.iter().map(|address| address.octets()))
}

fn serialize_ipv6<S: serde::Serializer>(
    addresses: &Vec<std::net::Ipv6Addr>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(addresses.iter().map(|address| address.octets()))
}

fn serialize_mac<S: serde::Serializer>(
    mac: &Option<fidl_fuchsia_net_ext::MacAddress>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match mac {
        None => serializer.serialize_none(),
        Some(fidl_fuchsia_net_ext::MacAddress { octets }) => serializer.collect_seq(octets.iter()),
    }
}

#[derive(Serialize)]
enum DeviceClass {
    Loopback,
    Blackhole,
    Virtual,
    Ethernet,
    WlanClient,
    Ppp,
    Bridge,
    WlanAp,
    Lowpan,
}

#[derive(Serialize)]
pub struct Properties {
    id: u64,
    name: String,
    device_class: DeviceClass,
    online: bool,
    #[serde(serialize_with = "serialize_ipv4")]
    ipv4_addresses: Vec<std::net::Ipv4Addr>,
    #[serde(serialize_with = "serialize_ipv6")]
    ipv6_addresses: Vec<std::net::Ipv6Addr>,
    #[serde(serialize_with = "serialize_mac")]
    mac: Option<fidl_fuchsia_net_ext::MacAddress>,
}

impl
    From<(
        fidl_fuchsia_net_interfaces_ext::Properties<fidl_fuchsia_net_interfaces_ext::AllInterest>,
        Option<fidl_fuchsia_net::MacAddress>,
    )> for Properties
{
    fn from(
        t: (
            fidl_fuchsia_net_interfaces_ext::Properties<
                fidl_fuchsia_net_interfaces_ext::AllInterest,
            >,
            Option<fidl_fuchsia_net::MacAddress>,
        ),
    ) -> Self {
        use itertools::Itertools as _;

        let (
            fidl_fuchsia_net_interfaces_ext::Properties {
                id,
                name,
                port_class,
                online,
                addresses,
                has_default_ipv4_route: _,
                has_default_ipv6_route: _,
                port_identity_koid: _,
            },
            mac,
        ) = t;
        let device_class = match port_class {
            fidl_fuchsia_net_interfaces_ext::PortClass::Loopback => DeviceClass::Loopback,
            fidl_fuchsia_net_interfaces_ext::PortClass::Blackhole => DeviceClass::Blackhole,
            fidl_fuchsia_net_interfaces_ext::PortClass::Virtual => DeviceClass::Virtual,
            fidl_fuchsia_net_interfaces_ext::PortClass::Ethernet => DeviceClass::Ethernet,
            fidl_fuchsia_net_interfaces_ext::PortClass::WlanClient => DeviceClass::WlanClient,
            fidl_fuchsia_net_interfaces_ext::PortClass::WlanAp => DeviceClass::WlanAp,
            fidl_fuchsia_net_interfaces_ext::PortClass::Ppp => DeviceClass::Ppp,
            fidl_fuchsia_net_interfaces_ext::PortClass::Bridge => DeviceClass::Bridge,
            fidl_fuchsia_net_interfaces_ext::PortClass::Lowpan => DeviceClass::Lowpan,
        };
        let (ipv4_addresses, ipv6_addresses) =
            addresses.into_iter().partition_map::<_, _, _, std::net::Ipv4Addr, std::net::Ipv6Addr>(
                |fidl_fuchsia_net_interfaces_ext::Address {
                     addr,
                     valid_until: _,
                     preferred_lifetime_info: _,
                     assignment_state,
                 }| {
                    // Event stream is created with `IncludedAddresses::OnlyAssigned`.
                    assert_eq!(
                        assignment_state,
                        fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned,
                        "Support for unassigned addresses have not been implemented",
                    );
                    let fidl_fuchsia_net_ext::Subnet { addr, prefix_len: _ } = addr.into();
                    let fidl_fuchsia_net_ext::IpAddress(addr) = addr;
                    match addr {
                        std::net::IpAddr::V4(addr) => itertools::Either::Left(addr),
                        std::net::IpAddr::V6(addr) => itertools::Either::Right(addr),
                    }
                },
            );
        Self {
            id: id.get(),
            name,
            device_class,
            online,
            ipv4_addresses,
            ipv6_addresses,
            mac: mac.map(Into::into),
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
/// A serializable alternative to [`fnet_stack_migration::NetstackVersion`].
pub enum NetstackVersion {
    Netstack2,
    Netstack3,
}

impl From<fnet_stack_migration::NetstackVersion> for NetstackVersion {
    fn from(version: fnet_stack_migration::NetstackVersion) -> NetstackVersion {
        match version {
            fnet_stack_migration::NetstackVersion::Netstack2 => NetstackVersion::Netstack2,
            fnet_stack_migration::NetstackVersion::Netstack3 => NetstackVersion::Netstack3,
        }
    }
}
impl From<NetstackVersion> for fnet_stack_migration::NetstackVersion {
    fn from(version: NetstackVersion) -> fnet_stack_migration::NetstackVersion {
        match version {
            NetstackVersion::Netstack2 => fnet_stack_migration::NetstackVersion::Netstack2,
            NetstackVersion::Netstack3 => fnet_stack_migration::NetstackVersion::Netstack3,
        }
    }
}

impl From<fnet_stack_migration::VersionSetting> for NetstackVersion {
    fn from(version: fnet_stack_migration::VersionSetting) -> NetstackVersion {
        let fnet_stack_migration::VersionSetting { version } = version;
        version.into()
    }
}

impl From<NetstackVersion> for fnet_stack_migration::VersionSetting {
    fn from(version: NetstackVersion) -> fnet_stack_migration::VersionSetting {
        fnet_stack_migration::VersionSetting { version: version.into() }
    }
}

impl TryFrom<Value> for NetstackVersion {
    type Error = Error;
    fn try_from(value: Value) -> Result<NetstackVersion, Error> {
        match value {
            Value::String(value) => match value.to_lowercase().as_str() {
                "ns2" | "netstack2" => Ok(NetstackVersion::Netstack2),
                "ns3" | "netstack3" => Ok(NetstackVersion::Netstack3),
                _ => Err(anyhow!("unrecognized netstack version: {}", value)),
            },
            _ => Err(anyhow!("unrecognized netstack version: {:?}", value)),
        }
    }
}

#[derive(Serialize)]
/// A serializable alternative to [`fnet_stack_migration::InEffectVersion`].
pub struct InEffectNetstackVersion {
    current_boot: NetstackVersion,
    automated_selection: Option<NetstackVersion>,
    user_selection: Option<NetstackVersion>,
}

impl From<fnet_stack_migration::InEffectVersion> for InEffectNetstackVersion {
    fn from(in_effect: fnet_stack_migration::InEffectVersion) -> InEffectNetstackVersion {
        let fnet_stack_migration::InEffectVersion { current_boot, automated, user } = in_effect;
        InEffectNetstackVersion {
            current_boot: current_boot.into(),
            automated_selection: automated.map(|selection| (*selection).into()),
            user_selection: user.map(|selection| (*selection).into()),
        }
    }
}

/// Network stack operations.
#[derive(Debug, Default)]
pub struct NetstackFacade {
    interfaces_state: OnceCell<fidl_fuchsia_net_interfaces::StateProxy>,
    root_interfaces: OnceCell<fidl_fuchsia_net_root::InterfacesProxy>,
    netstack_migration_state: OnceCell<fnet_stack_migration::StateProxy>,
    netstack_migration_control: OnceCell<fnet_stack_migration::ControlProxy>,
}

async fn get_netstack_proxy<P: fidl::endpoints::DiscoverableProtocolMarker>()
-> Result<P::Proxy, Error> {
    let query =
        fuchsia_component::client::connect_to_protocol::<fidl_fuchsia_sys2::RealmQueryMarker>()?;
    let moniker = "./core/network/netstack".try_into()?;
    let proxy = connect_to_instance_protocol::<P>(&moniker, OpenDirType::Exposed, &query).await?;
    Ok(proxy)
}

async fn get_netstack_migration_proxy<P: fidl::endpoints::DiscoverableProtocolMarker>()
-> Result<P::Proxy, Error> {
    let query =
        fuchsia_component::client::connect_to_protocol::<fidl_fuchsia_sys2::RealmQueryMarker>()?;
    let moniker = "./core/network/netstack-migration".try_into()?;
    let proxy = connect_to_instance_protocol::<P>(&moniker, OpenDirType::Exposed, &query).await?;
    Ok(proxy)
}

impl NetstackFacade {
    async fn get_interfaces_state(
        &self,
    ) -> Result<&fidl_fuchsia_net_interfaces::StateProxy, Error> {
        let Self {
            interfaces_state,
            root_interfaces: _,
            netstack_migration_state: _,
            netstack_migration_control: _,
        } = self;
        if let Some(state_proxy) = interfaces_state.get() {
            Ok(state_proxy)
        } else {
            let state_proxy =
                get_netstack_proxy::<fidl_fuchsia_net_interfaces::StateMarker>().await?;
            interfaces_state.set(state_proxy).unwrap();
            let state_proxy = interfaces_state.get().unwrap();
            Ok(state_proxy)
        }
    }

    async fn get_root_interfaces(&self) -> Result<&fidl_fuchsia_net_root::InterfacesProxy, Error> {
        let Self {
            interfaces_state: _,
            root_interfaces,
            netstack_migration_state: _,
            netstack_migration_control: _,
        } = self;
        if let Some(interfaces_proxy) = root_interfaces.get() {
            Ok(interfaces_proxy)
        } else {
            let interfaces_proxy =
                get_netstack_proxy::<fidl_fuchsia_net_root::InterfacesMarker>().await?;
            root_interfaces.set(interfaces_proxy).unwrap();
            let interfaces_proxy = root_interfaces.get().unwrap();
            Ok(interfaces_proxy)
        }
    }

    async fn get_netstack_migration_state(
        &self,
    ) -> Result<&fnet_stack_migration::StateProxy, Error> {
        let Self {
            interfaces_state: _,
            root_interfaces: _,
            netstack_migration_state,
            netstack_migration_control: _,
        } = self;
        if let Some(state_proxy) = netstack_migration_state.get() {
            Ok(state_proxy)
        } else {
            let state_proxy =
                get_netstack_migration_proxy::<fnet_stack_migration::StateMarker>().await?;
            netstack_migration_state.set(state_proxy).unwrap();
            let state_proxy = netstack_migration_state.get().unwrap();
            Ok(state_proxy)
        }
    }

    async fn get_netstack_migration_control(
        &self,
    ) -> Result<&fnet_stack_migration::ControlProxy, Error> {
        let Self {
            interfaces_state: _,
            root_interfaces: _,
            netstack_migration_state: _,
            netstack_migration_control,
        } = self;
        if let Some(control_proxy) = netstack_migration_control.get() {
            Ok(control_proxy)
        } else {
            let control_proxy =
                get_netstack_migration_proxy::<fnet_stack_migration::ControlMarker>().await?;
            netstack_migration_control.set(control_proxy).unwrap();
            let control_proxy = netstack_migration_control.get().unwrap();
            Ok(control_proxy)
        }
    }

    async fn get_control(
        &self,
        id: u64,
    ) -> Result<fidl_fuchsia_net_interfaces_ext::admin::Control, Error> {
        let root_interfaces = self.get_root_interfaces().await?;
        let (control, server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .context("create admin control endpoints")?;
        let () = root_interfaces.get_admin(id, server_end).context("send get admin request")?;
        Ok(control)
    }

    pub async fn enable_interface(&self, id: u64) -> Result<(), Error> {
        let control = self.get_control(id).await?;
        let _did_enable: bool = control
            .enable()
            .await
            .map_err(anyhow::Error::new)
            .and_then(|res| {
                res.map_err(|e: fidl_fuchsia_net_interfaces_admin::ControlEnableError| {
                    anyhow::anyhow!("{:?}", e)
                })
            })
            .with_context(|| format!("failed to enable interface {}", id))?;
        Ok(())
    }

    pub async fn disable_interface(&self, id: u64) -> Result<(), Error> {
        let control = self.get_control(id).await?;
        let _did_disable: bool = control
            .disable()
            .await
            .map_err(anyhow::Error::new)
            .and_then(|res| {
                res.map_err(|e: fidl_fuchsia_net_interfaces_admin::ControlDisableError| {
                    anyhow::anyhow!("{:?}", e)
                })
            })
            .with_context(|| format!("failed to disable interface {}", id))?;
        Ok(())
    }

    pub async fn list_interfaces(&self) -> Result<Vec<Properties>, Error> {
        let interfaces_state = self.get_interfaces_state().await?;
        let root_interfaces = self.get_root_interfaces().await?;
        // Only `OnlyAssigned` is implemented; additional work is required to
        // support unassigned addresses.
        let stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state(
            interfaces_state,
            Default::default(),
        )?;
        let response = fidl_fuchsia_net_interfaces_ext::existing(
            stream,
            std::collections::HashMap::<u64, _>::new(),
        )
        .await?;
        let response = response.into_values().map(
            |fidl_fuchsia_net_interfaces_ext::PropertiesAndState { properties, state: () }| async {
                match root_interfaces.get_mac(properties.id.get()).await? {
                    Ok(mac) => {
                        let mac = mac.map(|boxed_mac| *boxed_mac);
                        let view: Properties = (properties, mac).into();
                        Ok::<_, Error>(Some(view))
                    }
                    Err(fidl_fuchsia_net_root::InterfacesGetMacError::NotFound) => {
                        // Interface with given id not found; this occurs when state
                        // is reported for an interface that has since been removed.
                        Ok::<_, Error>(None)
                    }
                }
            },
        );
        let mut response: Vec<Properties> =
            futures::future::try_join_all(response).await?.into_iter().filter_map(|r| r).collect();
        let () = response.sort_by_key(|&Properties { id, .. }| id);
        Ok(response)
    }

    async fn get_addresses<T, F: Copy + FnMut(fidl_fuchsia_net::Subnet) -> Option<T>>(
        &self,
        f: F,
    ) -> Result<Vec<T>, Error> {
        let mut output = Vec::new();

        let interfaces_state = self.get_interfaces_state().await?;
        let (watcher, server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces::WatcherMarker>();
        let () = interfaces_state
            .get_watcher(&fidl_fuchsia_net_interfaces::WatcherOptions::default(), server)?;

        loop {
            match watcher.watch().await? {
                fidl_fuchsia_net_interfaces::Event::Existing(
                    fidl_fuchsia_net_interfaces::Properties { addresses, .. },
                ) => {
                    let addresses = addresses.unwrap();
                    let () = output.extend(
                        addresses
                            .into_iter()
                            .map(
                                |fidl_fuchsia_net_interfaces::Address {
                                     addr,
                                     valid_until: _,
                                     ..
                                 }| addr.unwrap(),
                            )
                            .filter_map(f),
                    );
                }
                fidl_fuchsia_net_interfaces::Event::Idle(fidl_fuchsia_net_interfaces::Empty {}) => {
                    break;
                }
                event => unreachable!("{:?}", event),
            }
        }

        Ok(output)
    }

    pub fn get_ipv6_addresses(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<std::net::Ipv6Addr>, Error>> + '_ {
        self.get_addresses(|addr| {
            let fidl_fuchsia_net_ext::Subnet { addr, prefix_len: _ } = addr.into();
            let fidl_fuchsia_net_ext::IpAddress(addr) = addr;
            match addr {
                std::net::IpAddr::V4(_) => None,
                std::net::IpAddr::V6(addr) => Some(addr),
            }
        })
    }

    pub fn get_link_local_ipv6_addresses(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<std::net::Ipv6Addr>, Error>> + '_ {
        use futures::TryFutureExt as _;

        self.get_ipv6_addresses().map_ok(|addresses| {
            addresses.into_iter().filter(|address| address.octets()[..2] == [0xfe, 0x80]).collect()
        })
    }

    /// Gets the current netstack version settings.
    ///
    /// See [`fnet_stack_migration::StateProxy::get_netstack_version`] for more
    /// details.
    pub async fn get_netstack_version(&self) -> Result<InEffectNetstackVersion, Error> {
        let netstack_migration_state = self.get_netstack_migration_state().await?;
        Ok(netstack_migration_state.get_netstack_version().await?.into())
    }

    /// Sets the user specified netstack version. takes effect on the next boot.
    ///
    /// See [`fnet_stack_migration::ControlProxy::set_user_netstack_version`]
    /// for more details.
    pub async fn set_user_netstack_version(&self, version: NetstackVersion) -> Result<(), Error> {
        let netstack_migration_control = self.get_netstack_migration_control().await?;
        Ok(netstack_migration_control.set_user_netstack_version(Some(&version.into())).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::StreamExt as _;
    use test_case::test_case;
    use {
        fidl_fuchsia_net as fnet, fidl_fuchsia_net_interfaces as finterfaces,
        fuchsia_async as fasync,
    };

    struct MockStateTester {
        expected_state: Vec<Box<dyn FnOnce(finterfaces::WatcherRequest) + Send + 'static>>,
    }

    impl MockStateTester {
        fn new() -> Self {
            Self { expected_state: vec![] }
        }

        pub fn create_facade_and_serve_state(
            self,
        ) -> (NetstackFacade, impl std::future::Future<Output = ()>) {
            let (interfaces_state, stream_future) = self.build_state_and_watcher();
            (
                NetstackFacade { interfaces_state: interfaces_state.into(), ..Default::default() },
                stream_future,
            )
        }

        fn push_state(
            mut self,
            request: impl FnOnce(finterfaces::WatcherRequest) + Send + 'static,
        ) -> Self {
            self.expected_state.push(Box::new(request));
            self
        }

        fn build_state_and_watcher(
            self,
        ) -> (finterfaces::StateProxy, impl std::future::Future<Output = ()>) {
            let (proxy, mut stream) =
                fidl::endpoints::create_proxy_and_stream::<finterfaces::StateMarker>();
            let stream_fut = async move {
                match stream.next().await {
                    Some(Ok(finterfaces::StateRequest::GetWatcher { watcher, .. })) => {
                        let mut into_stream = watcher.into_stream();
                        for expected in self.expected_state {
                            let () = expected(into_stream.next().await.unwrap().unwrap());
                        }
                        let finterfaces::WatcherRequest::Watch { responder } =
                            into_stream.next().await.unwrap().unwrap();
                        let () = responder
                            .send(&finterfaces::Event::Idle(finterfaces::Empty {}))
                            .unwrap();
                    }
                    err => panic!("Error in request handler: {:?}", err),
                }
            };
            (proxy, stream_fut)
        }

        fn expect_get_ipv6_addresses(self, result: Vec<fnet::Subnet>) -> Self {
            let addresses = result
                .into_iter()
                .map(|addr| finterfaces::Address { addr: Some(addr), ..Default::default() })
                .collect();
            self.push_state(move |req| match req {
                finterfaces::WatcherRequest::Watch { responder } => responder
                    .send(&finterfaces::Event::Existing(finterfaces::Properties {
                        addresses: Some(addresses),
                        ..Default::default()
                    }))
                    .unwrap(),
            })
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_get_ipv6_addresses() {
        let ipv6_octets = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

        let ipv6_address = fnet::Subnet {
            addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: ipv6_octets }),
            // NB: prefix length is ignored, use invalid value to prove it.
            prefix_len: 137,
        };
        let ipv4_address = fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: [0, 1, 2, 3] }),
            // NB: prefix length is ignored, use invalid value to prove it.
            prefix_len: 139,
        };
        let all_addresses = [ipv6_address.clone(), ipv4_address.clone()];
        let (facade, stream_fut) = MockStateTester::new()
            .expect_get_ipv6_addresses(all_addresses.to_vec())
            .create_facade_and_serve_state();
        let facade_fut = async move {
            let result_address: Vec<_> = facade.get_ipv6_addresses().await.unwrap();
            assert_eq!(result_address, [std::net::Ipv6Addr::from(ipv6_octets)]);
        };
        futures::future::join(facade_fut, stream_fut).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_get_link_local_ipv6_addresses() {
        let ipv6_address = fnet::Subnet {
            addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                addr: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            }),
            // NB: prefix length is ignored, use invalid value to prove it.
            prefix_len: 137,
        };
        let link_local_ipv6_octets = [0xfe, 0x80, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let link_local_ipv6_address = fnet::Subnet {
            addr: fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: link_local_ipv6_octets }),
            // NB: prefix length is ignored, use invalid value to prove it.
            prefix_len: 139,
        };
        let ipv4_address = fnet::Subnet {
            addr: fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: [0, 1, 2, 3] }),
            // NB: prefix length is ignored, use invalid value to prove it.
            prefix_len: 141,
        };
        let all_addresses =
            [ipv6_address.clone(), link_local_ipv6_address.clone(), ipv4_address.clone()];
        let (facade, stream_fut) = MockStateTester::new()
            .expect_get_ipv6_addresses(all_addresses.to_vec())
            .create_facade_and_serve_state();
        let facade_fut = async move {
            let result_address: Vec<_> = facade.get_link_local_ipv6_addresses().await.unwrap();
            assert_eq!(result_address, [std::net::Ipv6Addr::from(link_local_ipv6_octets)]);
        };
        futures::future::join(facade_fut, stream_fut).await;
    }

    #[test_case(Value::String("ns2".to_string()), Some(NetstackVersion::Netstack2); "ns2")]
    #[test_case(Value::String("ns3".to_string()), Some(NetstackVersion::Netstack3); "ns3")]
    #[test_case(Value::String("netstack2".to_string()), Some(NetstackVersion::Netstack2);
        "netstack2")]
    #[test_case(Value::String("netstack3".to_string()), Some(NetstackVersion::Netstack3);
        "netstack3")]
    #[test_case(Value::String("invalid".to_string()), None; "invalid_string")]
    #[test_case(Value::Bool(false), None; "invalid_value")]
    fn test_convert_netstack_version_from_json_value(
        json_value: Value,
        expected_version: Option<NetstackVersion>,
    ) {
        let version: Result<NetstackVersion, Error> = json_value.try_into();
        match expected_version {
            Some(expected_version) => assert_eq!(version.expect("parse version"), expected_version),
            None => assert_matches!(version, Err(_)),
        }
    }

    #[test_case(fnet_stack_migration::NetstackVersion::Netstack2, NetstackVersion::Netstack2;
        "netstack2")]
    #[test_case(fnet_stack_migration::NetstackVersion::Netstack3, NetstackVersion::Netstack3;
        "netstack3")]
    fn test_convert_netstack_version_from_fidl(
        fidl_version: fnet_stack_migration::NetstackVersion,
        expected_version: NetstackVersion,
    ) {
        assert_eq!(NetstackVersion::from(fidl_version), expected_version);
        assert_eq!(
            NetstackVersion::from(fnet_stack_migration::VersionSetting { version: fidl_version }),
            expected_version
        );
    }
}
