// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::pin::pin;

use assert_matches::assert_matches;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_tun as fnet_tun;
use futures::{SinkExt as _, StreamExt as _};
use net_declare::fidl_mac;
use net_types::Witness as _;
use net_types::ip::Ip as _;

const PERF_TEST_MODE_ITERATIONS: usize = 10;
const PERF_TEST_MODE_INTERFACES: usize = 10;

const UNIT_TEST_MODE_ITERATIONS: usize = 2;
const UNIT_TEST_MODE_INTERFACES: usize = 1;

pub struct Interfaces;

impl crate::Workload for Interfaces {
    const NAME: &'static str = "Interfaces";

    async fn create(_netstack: &netemul::TestRealm<'_>) -> Self {
        Self
    }

    async fn run(&self, netstack: &netemul::TestRealm<'_>, perftest_mode: bool) {
        let interfaces_state = netstack
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .expect("connect to protocol");

        // Install several interfaces backed by network-tun.
        let interfaces = {
            let stream = fnet_interfaces_ext::event_stream_from_state::<
                fnet_interfaces_ext::DefaultInterest,
            >(&interfaces_state, Default::default())
            .expect("get interface event stream");
            let mut stream = pin!(stream);
            let mut if_state = fnet_interfaces_ext::existing(
                stream.by_ref(),
                HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
            )
            .await
            .expect("collect existing interfaces");

            let num_interfaces =
                if perftest_mode { PERF_TEST_MODE_INTERFACES } else { UNIT_TEST_MODE_INTERFACES };
            let (tx, rx) = futures::channel::mpsc::channel(num_interfaces);
            futures::stream::repeat(())
                .take(num_interfaces)
                .for_each_concurrent(None, |()| {
                    let interfaces_state = &interfaces_state;
                    let mut tx = tx.clone();
                    async move {
                        let interface = install_interface(netstack, interfaces_state).await;
                        tx.send(interface).await.expect("receiver should not be dropped");
                    }
                })
                .await;
            drop(tx);
            fnet_interfaces_ext::wait_interface(stream.by_ref(), &mut if_state, |interfaces| {
                (interfaces.len() == num_interfaces + 1).then_some(())
            })
            .await
            .expect("observe interface creation");
            rx
        };

        interfaces
            .for_each_concurrent(None, |interface| {
                stress_interface(interface, &interfaces_state, perftest_mode)
            })
            .await;

        // Wait for the interfaces we installed to be removed.
        let stream = fnet_interfaces_ext::event_stream_from_state::<
            fnet_interfaces_ext::DefaultInterest,
        >(&interfaces_state, Default::default())
        .expect("get interface event stream");
        let mut stream = pin!(stream);
        let mut interfaces = fnet_interfaces_ext::existing(
            stream.by_ref(),
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
        )
        .await
        .expect("collect existing interfaces");
        if interfaces.len() != 1 {
            fnet_interfaces_ext::wait_interface(stream.by_ref(), &mut interfaces, |interfaces| {
                (interfaces.len() == 1).then_some(())
            })
            .await
            .expect("observe interface removal");
        }
    }
}

struct Interface {
    id: u64,
    addr: net_types::ip::Ipv6Addr,
    control: fnet_interfaces_ext::admin::Control,
    _device_control: fnet_interfaces_admin::DeviceControlProxy,
    tun_device: fnet_tun::DeviceProxy,
    _tun_port: fnet_tun::PortProxy,
}

const TUN_DEVICE_PORT_ID: u8 = 0;
const MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:00:00:00:00:ff");

async fn install_interface(
    netstack: &netemul::TestRealm<'_>,
    interfaces_state: &fnet_interfaces::StateProxy,
) -> Interface {
    let (tun_device, netdevice) = netstack_testing_common::devices::create_tun_device();
    let (tun_port, dev_port) = netstack_testing_common::devices::create_eth_tun_port(
        &tun_device,
        TUN_DEVICE_PORT_ID,
        MAC_ADDRESS,
    )
    .await;

    let device_control = netstack_testing_common::devices::install_device(&netstack, netdevice);
    let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");
    let (control, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();
    device_control
        .create_interface(&port_id, server_end, fnet_interfaces_admin::Options::default())
        .expect("create interface");
    let control = fnet_interfaces_ext::admin::Control::new(control);
    assert!(control.enable().await.expect("call enable").expect("enable interface"));
    tun_port.set_online(true).await.expect("can set online");

    let id = control.get_id().await.expect("get id");
    let addr = netstack_testing_common::interfaces::wait_for_v6_ll(interfaces_state, id)
        .await
        .expect("waiting for link local address");

    Interface {
        id,
        addr,
        control,
        _device_control: device_control,
        tun_device,
        _tun_port: tun_port,
    }
}

async fn stress_interface(
    interface: Interface,
    interfaces_state: &fnet_interfaces::StateProxy,
    perftest_mode: bool,
) {
    let Interface { id, addr, control, tun_device, .. } = interface;
    let stream =
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("get interface event stream");
    let mut stream = pin!(stream);
    let mut state = fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(id);

    // Repeatedly toggle interface up/down and send traffic through it
    // simulating incoming neighbor solicitations.
    let iterations =
        if perftest_mode { PERF_TEST_MODE_ITERATIONS } else { UNIT_TEST_MODE_ITERATIONS };
    for i in 0..iterations {
        fuchsia_async::Timer::new(zx::MonotonicDuration::from_millis(50)).await;

        assert!(control.disable().await.expect("call disable").expect("disable interface"));
        fnet_interfaces_ext::wait_interface_with_id(stream.by_ref(), &mut state, |iface| {
            (!iface.properties.online).then_some(())
        })
        .await
        .expect("wait for interface offline");

        fuchsia_async::Timer::new(zx::MonotonicDuration::from_millis(50)).await;

        assert!(control.enable().await.expect("call enable").expect("enable interface"));
        fnet_interfaces_ext::wait_interface_with_id(stream.by_ref(), &mut state, |iface| {
            iface.properties.online.then_some(())
        })
        .await
        .expect("wait for interface online");

        // Pick a source address that is not the same as the interface's address by
        // flipping the last byte.
        let src_ip = {
            let mut bytes = addr.ipv6_bytes();
            let last = bytes.last_mut().unwrap();
            *last = !*last;
            net_types::ip::Ipv6Addr::from_bytes(bytes)
        };
        let src_mac = net_types::ethernet::Mac::new([0, 0, 0, 0, 0, u8::try_from(i).unwrap()]);

        // Simulate an incoming Neighbor Solicitation.
        let frame = serialize_neighbor_solictation(src_ip, src_mac);
        loop {
            match tun_device
                .write_frame(&fnet_tun::Frame {
                    port: Some(TUN_DEVICE_PORT_ID),
                    frame_type: Some(fhardware_network::FrameType::Ethernet),
                    data: Some(frame.clone()),
                    ..Default::default()
                })
                .await
                .expect("call write_frame")
                .map_err(zx::Status::err_from_raw)
            {
                Ok(()) => break,
                Err(zx::Status::SHOULD_WAIT) => continue,
                Err(e) => panic!("failed to write incoming frame to tun device: {:?}", e),
            }
        }
    }

    control.remove().await.expect("interface should be removable").expect("remove interface");
    assert_matches!(
        control.wait_termination().await,
        fnet_interfaces_ext::admin::TerminalError::Terminal(
            fnet_interfaces_admin::InterfaceRemovedReason::User
        )
    );
}

fn serialize_neighbor_solictation(
    src_ip: net_types::ip::Ipv6Addr,
    src_mac: net_types::ethernet::Mac,
) -> Vec<u8> {
    use packet::NestableSerializer as _;
    use packet::serialize::{InnerPacketBuilder as _, NoOpSerializationContext, Serializer as _};
    use packet_formats::ethernet::{ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrameBuilder};
    use packet_formats::icmp::ndp::NeighborSolicitation;
    use packet_formats::icmp::{IcmpPacketBuilder, IcmpZeroCode};
    use packet_formats::ip::Ipv6Proto;
    use packet_formats::ipv6::Ipv6PacketBuilder;

    let snmc = src_ip.to_solicited_node_address();
    [].into_serializer()
        .wrap_in(IcmpPacketBuilder::<_, _>::new(
            net_types::ip::Ipv6::UNSPECIFIED_ADDRESS,
            snmc.get(),
            IcmpZeroCode,
            NeighborSolicitation::new(src_ip),
        ))
        .wrap_in(Ipv6PacketBuilder::new(
            net_types::ip::Ipv6::UNSPECIFIED_ADDRESS,
            snmc.get(),
            netstack_testing_common::ndp::MESSAGE_TTL,
            Ipv6Proto::Icmpv6,
        ))
        .wrap_in(EthernetFrameBuilder::new(
            src_mac,
            net_types::ethernet::Mac::from(&snmc),
            EtherType::Ipv6,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NoOpSerializationContext)
        .expect("serialize NDP packet in Ethernet frame")
        .unwrap_b()
        .into_inner()
}
