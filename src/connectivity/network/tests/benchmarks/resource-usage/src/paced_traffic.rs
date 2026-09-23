// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_neighbor as fnet_neighbor;
use fidl_fuchsia_net_tun as fnet_tun;
use fuchsia_async::net::UdpSocket;
use futures::FutureExt as _;
use net_declare::fidl_mac;
use netemul::RealmUdpSocket as _;

const TUN_DEVICE_PORT_ID: u8 = 0;
const MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:00:00:00:00:ff");
const DST_MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");
const UNSPECIFIED: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0);
const MSG_SIZE: usize = 1200;
const DATA: [u8; MSG_SIZE] = [0xff; MSG_SIZE];
const NUM_PACKETS: usize = 4;
const INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// This benchmark generates [`NUM_PACKETS`] packets on a TUN device with
/// [`INTERVAL`] and measures the memory usage for this low traffic workload.
pub struct PacedTraffic {
    pub tun_device: fnet_tun::DeviceProxy,
    pub _tun_port: fnet_tun::PortProxy,
    pub _dev_port: fidl_fuchsia_hardware_network::PortProxy,
    pub _device_control: fnet_interfaces_admin::DeviceControlProxy,
    pub _control: fnet_interfaces_ext::admin::Control,
    pub client: UdpSocket,
    pub dst_addr: std::net::SocketAddr,
}

impl crate::Workload for PacedTraffic {
    const NAME: &'static str = "PacedTraffic";

    async fn create(netstack: &netemul::TestRealm<'_>) -> Self {
        let (tun_device, netdevice) =
            netstack_testing_common::devices::create_tun_device_with(fnet_tun::DeviceConfig {
                blocking: Some(true),
                ..Default::default()
            });
        let (tun_port, dev_port) = netstack_testing_common::devices::create_eth_tun_port(
            &tun_device,
            TUN_DEVICE_PORT_ID,
            MAC_ADDRESS,
        )
        .await;

        let device_control = netstack_testing_common::devices::install_device(netstack, netdevice);
        let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
        let (control, server_end) =
            fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
        device_control
            .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
            .expect("create interface");
        let control = fnet_interfaces_ext::admin::Control::new(control);
        assert!(control.enable().await.expect("call enable").expect("enable interface"));
        tun_port.set_online(true).await.expect("can set online");

        let interfaces_state = netstack
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .expect("connect to protocol");
        let id = control.get_id().await.expect("get id");
        let _addr = netstack_testing_common::interfaces::wait_for_v6_ll(&interfaces_state, id)
            .await
            .expect("waiting for link local address");

        // Create UDP socket to send unicast packets.
        let client = UdpSocket::bind_in_realm(netstack, UNSPECIFIED).await.expect("bind");
        let dst_ip_std = std::net::Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 2);
        let dst_ip_fidl = fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: dst_ip_std.octets() });

        let neighbor_controller = netstack
            .connect_to_protocol::<fnet_neighbor::ControllerMarker>()
            .expect("connect to protocol");
        neighbor_controller
            .add_entry(id, &dst_ip_fidl, &DST_MAC_ADDRESS)
            .await
            .expect("add_entry FIDL error")
            .expect("add_entry failed");

        let dst_addr = std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
            dst_ip_std,
            12345,
            0,
            u32::try_from(id).unwrap(),
        ));

        // Tun devices are virtual, so the VMOs are mapped instead of pinned. Try
        // to ramp up the committed pages.
        for _ in 0..fnet_tun::FIFO_DEPTH {
            assert_eq!(
                client.send_to(&DATA, dst_addr).await.expect("failed to send UDP"),
                DATA.len()
            );
        }

        // Drain any remaining frames from the tun device.
        drain_tun_device(&tun_device).await;

        PacedTraffic {
            tun_device,
            _tun_port: tun_port,
            _dev_port: dev_port,
            _device_control: device_control,
            _control: control,
            client,
            dst_addr,
        }
    }

    async fn run(&self, _netstack: &netemul::TestRealm<'_>, _perftest_mode: bool) {
        // Send packets with interval
        for _ in 0..NUM_PACKETS {
            assert_eq!(
                self.client.send_to(&DATA, self.dst_addr).await.expect("failed to send UDP"),
                DATA.len()
            );
            fuchsia_async::Timer::new(INTERVAL).await;
        }

        drain_tun_device(&self.tun_device).await;
    }
}

async fn drain_tun_device(tun_device: &fnet_tun::DeviceProxy) {
    loop {
        let read_fut = tun_device.read_frame().fuse();
        let timeout_fut = fuchsia_async::Timer::new(zx::MonotonicDuration::from_millis(100)).fuse();
        futures::pin_mut!(read_fut, timeout_fut);
        futures::select! {
            frame = read_fut => {
                let _ = frame.expect("read frame").expect("read frame status");
            }
            _ = timeout_fut => {
                break;
            }
        }
    }
}
