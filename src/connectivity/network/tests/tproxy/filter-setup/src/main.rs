// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This program configures TPROXY filtering rules to allow testing transparent
//! proxy behavior with UDP sockets. It sets up the following rules:
//!
//!  * UDP packets with a destination port in [10000, 19999] are proxied to a
//!    socket bound on local port 8001.
//!  * UDP packets with a destination port in [20000, 29999] are proxied to a
//!    socket bound on 127.0.0.1.
//!  * UDP packets with a destination port in [30000, 39999] are proxied to a
//!    socket bound on 127.0.0.1 port 8002.
//!  * UDP packets with a destination port in [40000, 49999] are proxied to a
//!    socket bound on ::1.
//!  * UDP packets with a destination port in [50000, 59999] are proxied to a
//!    socket bound on ::1 port 8002.

use std::num::NonZeroU16;

use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext::{
    Action, Change, Controller, ControllerId, Domain, InstalledIpRoutine, IpHook, Matchers,
    Namespace, NamespaceId, Resource, Routine, RoutineId, RoutineType, Rule, RuleId,
    TransparentProxy,
};
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use net_declare::fidl_ip;

const BUS_NAME: &str = "test-bus";
const SETUP_COMPLETE: &str = "setup-complete";
const IPV4_LOCALHOST: fidl_fuchsia_net::IpAddress = fidl_ip!("127.0.0.1");
const IPV6_LOCALHOST: fidl_fuchsia_net::IpAddress = fidl_ip!("::1");
const TPROXY_PORT_1: NonZeroU16 = NonZeroU16::new(8001).unwrap();
const TPROXY_PORT_2: NonZeroU16 = NonZeroU16::new(8002).unwrap();

#[fuchsia::main]
async fn main() {
    let control = fuchsia_component::client::connect_to_protocol::<fnet_filter::ControlMarker>()
        .expect("connect to protocol");
    let mut controller = Controller::new(&control, &ControllerId(String::from("tproxy")))
        .await
        .expect("create controller");
    let namespace_id = NamespaceId(String::from("namespace"));
    let routine_id = RoutineId { namespace: namespace_id.clone(), name: String::from("routine") };
    let resources = [
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp }),
        Resource::Routine(Routine {
            id: routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority: 0,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 0 },
            matchers: Matchers {
                transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                    src_port: None,
                    dst_port: Some(fnet_matchers_ext::Port::new(10000, 19999, false).unwrap()),
                }),
                ..Default::default()
            },
            action: Action::TransparentProxy(TransparentProxy::LocalPort(TPROXY_PORT_1)),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 1 },
            matchers: Matchers {
                transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                    src_port: None,
                    dst_port: Some(fnet_matchers_ext::Port::new(20000, 29999, false).unwrap()),
                }),
                ..Default::default()
            },
            action: Action::TransparentProxy(TransparentProxy::LocalAddr(IPV4_LOCALHOST)),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 2 },
            matchers: Matchers {
                transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                    src_port: None,
                    dst_port: Some(fnet_matchers_ext::Port::new(30000, 39999, false).unwrap()),
                }),
                ..Default::default()
            },
            action: Action::TransparentProxy(TransparentProxy::LocalAddrAndPort(
                IPV4_LOCALHOST,
                TPROXY_PORT_2,
            )),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 3 },
            matchers: Matchers {
                transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                    src_port: None,
                    dst_port: Some(fnet_matchers_ext::Port::new(40000, 49999, false).unwrap()),
                }),
                ..Default::default()
            },
            action: Action::TransparentProxy(TransparentProxy::LocalAddr(IPV6_LOCALHOST)),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: routine_id.clone(), index: 4 },
            matchers: Matchers {
                transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                    src_port: None,
                    dst_port: Some(fnet_matchers_ext::Port::new(50000, 59999, false).unwrap()),
                }),
                ..Default::default()
            },
            action: Action::TransparentProxy(TransparentProxy::LocalAddrAndPort(
                IPV6_LOCALHOST,
                TPROXY_PORT_2,
            )),
        }),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    // Let the client know that the TPROXY filter has been configured.
    let _bus = netemul_sync::Bus::subscribe(BUS_NAME, SETUP_COMPLETE).expect("subscribe to bus");

    // Stay alive so that the filter controller doesn't get removed, tearing down
    // the TPROXY configuration with it.
    //
    // TODO(https://fxbug.dev/42182623): detach the controller and exit when
    // detaching is implemented.
    futures::future::pending::<()>().await;
}
