// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for interacting with devices during integration tests.

use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_tun as fnet_tun;

use assert_matches::assert_matches;

/// Create a Tun device, returning handles to the created
/// `fuchsia.net.tun/Device` and the underlying network device.
pub fn create_tun_device()
-> (fnet_tun::DeviceProxy, fidl::endpoints::ClientEnd<fhardware_network::DeviceMarker>) {
    create_tun_device_with(fnet_tun::DeviceConfig::default())
}

/// Create a Tun device with the provided config and return its handles.
pub fn create_tun_device_with(
    device_config: fnet_tun::DeviceConfig,
) -> (fnet_tun::DeviceProxy, fidl::endpoints::ClientEnd<fhardware_network::DeviceMarker>) {
    let tun_ctl = fuchsia_component::client::connect_to_protocol::<fnet_tun::ControlMarker>()
        .expect("connect to protocol");
    let (tun_dev, tun_dev_server_end) = fidl::endpoints::create_proxy::<fnet_tun::DeviceMarker>();
    tun_ctl.create_device(&device_config, tun_dev_server_end).expect("create tun device");
    let (netdevice_client_end, netdevice_server_end) =
        fidl::endpoints::create_endpoints::<fhardware_network::DeviceMarker>();
    tun_dev.get_device(netdevice_server_end).expect("get device");
    (tun_dev, netdevice_client_end)
}

/// Install the given network device into the test realm's networking stack,
/// returning the created `fuchsia.net.interfaces.admin/DeviceControl` handle.
pub fn install_device(
    realm: &netemul::TestRealm<'_>,
    device: fidl::endpoints::ClientEnd<fhardware_network::DeviceMarker>,
) -> fnet_interfaces_admin::DeviceControlProxy {
    let (admin_device_control, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::DeviceControlMarker>();
    let installer = realm
        .connect_to_protocol::<fnet_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");
    installer.install_device(device, server_end).expect("install device");
    admin_device_control
}

/// Create a port on the given Tun device, returning handles to the created
/// `fuchsia.net.tun/Port` and the underlying network port.
pub async fn create_tun_port_with(
    tun_device: &fnet_tun::DeviceProxy,
    id: u8,
    rx_frame_types: impl IntoIterator<Item = fhardware_network::FrameType>,
    tx_frame_types: impl IntoIterator<Item = fhardware_network::FrameType>,
    mac: Option<fnet::MacAddress>,
) -> (fnet_tun::PortProxy, fhardware_network::PortProxy) {
    let (port, server_end) = fidl::endpoints::create_proxy::<fnet_tun::PortMarker>();
    let rx_types = rx_frame_types.into_iter().collect();
    let tx_types = tx_frame_types
        .into_iter()
        .map(|frame_type| fhardware_network::FrameTypeSupport {
            type_: frame_type,
            features: fhardware_network::FRAME_FEATURES_RAW,
            supported_flags: fhardware_network::TxFlags::empty(),
        })
        .collect();
    tun_device
        .add_port(
            &fnet_tun::DevicePortConfig {
                base: Some(fnet_tun::BasePortConfig {
                    id: Some(id),
                    rx_types: Some(rx_types),
                    tx_types: Some(tx_types),
                    mtu: Some(netemul::DEFAULT_MTU.into()),
                    ..Default::default()
                }),
                mac,
                ..Default::default()
            },
            server_end,
        )
        .expect("add port");

    let (network_port, server_end) =
        fidl::endpoints::create_proxy::<fhardware_network::PortMarker>();
    port.get_port(server_end).expect("get port");

    (port, network_port)
}

/// Creates a port on the given Tun device that supports IPv4 and IPv6 frame
/// types.
pub async fn create_ip_tun_port(
    tun_device: &fnet_tun::DeviceProxy,
    id: u8,
) -> (fnet_tun::PortProxy, fhardware_network::PortProxy) {
    const IP_FRAME_TYPES: [fhardware_network::FrameType; 2] =
        [fhardware_network::FrameType::Ipv4, fhardware_network::FrameType::Ipv6];
    create_tun_port_with(tun_device, id, IP_FRAME_TYPES, IP_FRAME_TYPES, None).await
}

/// Add a pure IP interface to the given device/port, returning the created
/// `fuchsia.net.interfaces.admin/Control` handle.
pub async fn add_pure_ip_interface(
    network_port: &fhardware_network::PortProxy,
    admin_device_control: &fnet_interfaces_admin::DeviceControlProxy,
    interface_name: &str,
) -> fnet_interfaces_admin::ControlProxy {
    let fhardware_network::PortInfo { id, .. } = network_port.get_info().await.expect("get info");
    let port_id = id.expect("port id");

    let (admin_control, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();

    admin_device_control
        .create_interface(
            &port_id,
            server_end,
            fnet_interfaces_admin::Options {
                name: Some(interface_name.to_string()),
                ..Default::default()
            },
        )
        .expect("create interface");
    admin_control
}

/// Creates a port on the given Tun device that supports the Ethernet frame
/// type.
pub async fn create_eth_tun_port(
    tun_device: &fnet_tun::DeviceProxy,
    id: u8,
    mac: fnet::MacAddress,
) -> (fnet_tun::PortProxy, fhardware_network::PortProxy) {
    const ETH_FRAME_TYPES: [fhardware_network::FrameType; 1] =
        [fhardware_network::FrameType::Ethernet];
    create_tun_port_with(tun_device, id, ETH_FRAME_TYPES, ETH_FRAME_TYPES, Some(mac)).await
}

/// Default port ID when creating a tun device pair.
pub const TUN_DEFAULT_PORT_ID: u8 = 0;

/// Create a Tun device pair with an Ethernet port.
pub async fn create_eth_tun_pair()
-> (fnet_tun::DevicePairProxy, fhardware_network::PortProxy, fhardware_network::PortProxy) {
    create_tun_pair_with(
        fnet_tun::DevicePairConfig::default(),
        fnet_tun::DevicePairPortConfig {
            base: Some(fnet_tun::BasePortConfig {
                id: Some(TUN_DEFAULT_PORT_ID),
                mtu: Some(netemul::DEFAULT_MTU.into()),
                rx_types: Some(vec![fhardware_network::FrameType::Ethernet]),
                tx_types: Some(vec![fhardware_network::FrameTypeSupport {
                    type_: fhardware_network::FrameType::Ethernet,
                    features: 0,
                    supported_flags: fhardware_network::TxFlags::empty(),
                }]),
                ..Default::default()
            }),
            mac_left: Some(fnet::MacAddress { octets: crate::constants::eth::MAC_ADDR.bytes() }),
            mac_right: Some(fnet::MacAddress { octets: crate::constants::eth::MAC_ADDR2.bytes() }),
            ..Default::default()
        },
    )
    .await
}

/// Create a Tun device pair with the provided configurations, returning
/// handles to the created `fuchsia.net.tun/DevicePair` and both underlying
/// ports.
pub async fn create_tun_pair_with(
    dev_pair_config: fnet_tun::DevicePairConfig,
    dev_pair_port_config: fnet_tun::DevicePairPortConfig,
) -> (fnet_tun::DevicePairProxy, fhardware_network::PortProxy, fhardware_network::PortProxy) {
    let tun_ctl = fuchsia_component::client::connect_to_protocol::<fnet_tun::ControlMarker>()
        .expect("connect to protocol");
    let (tun_dev_pair, tun_dev_pair_server_end) =
        fidl::endpoints::create_proxy::<fnet_tun::DevicePairMarker>();
    tun_ctl.create_pair(&dev_pair_config, tun_dev_pair_server_end).expect("create tun device pair");

    let port_id = assert_matches!(dev_pair_port_config, fnet_tun::DevicePairPortConfig {
        base: Some(fnet_tun::BasePortConfig {
            id: Some(id),
            ..
        }),
        ..
    } => id);
    tun_dev_pair
        .add_port(&dev_pair_port_config)
        .await
        .expect("add port FIDL call")
        .map_err(zx::Status::err_from_raw)
        .expect("add port");

    let (left_port, left_port_server_end) =
        fidl::endpoints::create_proxy::<fhardware_network::PortMarker>();
    let (right_port, right_port_server_end) =
        fidl::endpoints::create_proxy::<fhardware_network::PortMarker>();
    tun_dev_pair.get_left_port(port_id, left_port_server_end).expect("get left port");
    tun_dev_pair.get_right_port(port_id, right_port_server_end).expect("get right port");

    (tun_dev_pair, left_port, right_port)
}
