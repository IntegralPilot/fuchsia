// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Integration tests for the `net filter` subcommand.

#![cfg(test)]

use std::collections::HashMap;
use std::pin::pin;

use anyhow::Result;
use argh::FromArgs as _;
use assert_matches::assert_matches;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext::{
    self as fnet_filter_ext, Action, ControllerId, Domain, InstalledIpRoutine, IpHook, Matchers,
    Namespace, NamespaceId, Resource, Routine, RoutineId, RoutineType, Rule, RuleId,
};
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use net_declare::fidl_subnet;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use test_case::test_case;

async fn run_command(realm: &netemul::TestRealm<'_>, args: &[&'static str]) -> Result<()> {
    net_cli::do_root(
        writer::JsonWriter::new(None),
        net_cli::Command::from_args(&["net"], &[&["filter"], args].concat())
            .expect("should parse args successfully"),
        &crate::TestRealmConnector { realm },
        &crate::TestDeps,
    )
    .await
}

async fn run_create(realm: &netemul::TestRealm<'_>, args: &[&'static str]) -> Result<()> {
    run_command(realm, &[&["create"], args].concat()).await
}

async fn run_remove(realm: &netemul::TestRealm<'_>, args: &[&'static str]) -> Result<()> {
    run_command(realm, &[&["remove"], args].concat()).await
}

#[fuchsia::test]
async fn filter_create() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let netstack = sandbox
        .create_netstack_realm::<Netstack3, _>("netstack")
        .expect("creating realm should succeed");

    const CONTROLLER: &str = "test";
    const NAMESPACE: &str = "foo";
    const ROUTINE: &str = "bar";

    run_create(&netstack, &["--controller", CONTROLLER, "namespace", "--name", NAMESPACE])
        .await
        .expect("should succeed");
    run_create(
        &netstack,
        &[
            "--controller",
            CONTROLLER,
            "routine",
            "--namespace",
            NAMESPACE,
            "--name",
            ROUTINE,
            "--type",
            "ip",
            "--hook",
            "local_ingress",
            "--priority",
            "-100",
        ],
    )
    .await
    .expect("should succeed");
    run_create(
        &netstack,
        &[
            "--controller",
            CONTROLLER,
            "rule",
            "--namespace",
            NAMESPACE,
            "--routine",
            ROUTINE,
            "--index",
            "42",
            "--in-interface",
            "class:wlan-client",
            "--src-addr",
            "!subnet:192.0.2.0/24",
            "--transport-protocol",
            "tcp",
            "--dst-port",
            "22..=22",
            "drop",
        ],
    )
    .await
    .expect("should succeed");

    let state =
        netstack.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream = fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
    let mut stream = pin!(stream);
    let observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");

    let namespace_id = NamespaceId(NAMESPACE.to_owned());
    let namespace =
        Resource::Namespace(Namespace { id: namespace_id.clone(), domain: Domain::AllIp });
    let routine_id = RoutineId { namespace: namespace_id, name: ROUTINE.to_owned() };
    let routine = Resource::Routine(Routine {
        id: routine_id.clone(),
        routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
            hook: IpHook::LocalIngress,
            priority: -100,
        })),
    });
    let rule = Resource::Rule(Rule {
        id: RuleId { routine: routine_id, index: 42 },
        matchers: Matchers {
            in_interface: Some(fnet_matchers_ext::Interface::PortClass(
                fnet_interfaces_ext::PortClass::WlanClient,
            )),
            src_addr: Some(fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                    fidl_subnet!("192.0.2.0/24").try_into().unwrap(),
                ),
                invert: true,
            }),
            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                src_port: None,
                dst_port: Some(fnet_matchers_ext::Port::new(22, 22, false).unwrap()),
            }),
            ..Default::default()
        },
        action: Action::Drop,
    });
    assert_eq!(
        observed,
        HashMap::from([(
            ControllerId(CONTROLLER.to_owned()),
            HashMap::from([
                (namespace.id(), namespace),
                (routine.id(), routine),
                (rule.id(), rule)
            ])
        )])
    );
}

#[fuchsia::test]
async fn filter_create_remove_idempotent() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let netstack = sandbox
        .create_netstack_realm::<Netstack3, _>("netstack")
        .expect("creating realm should succeed");

    const CONTROLLER_AND_NAMESPACE: &[&str] =
        &["--controller", "test", "namespace", "--name", "foo"];

    // Create a resource.
    run_create(&netstack, CONTROLLER_AND_NAMESPACE).await.expect("create namespace");

    // Cannot create the same resource again.
    assert_matches!(run_create(&netstack, CONTROLLER_AND_NAMESPACE).await, Err(_));

    // Can create the same resource again if `idempotent` is specified.
    run_create(&netstack, &[&["--idempotent"], CONTROLLER_AND_NAMESPACE].concat())
        .await
        .expect("create same namespace");

    // Remove the resource.
    run_remove(&netstack, CONTROLLER_AND_NAMESPACE).await.expect("remove namespace");

    // Cannot remove the same resource again.
    assert_matches!(run_remove(&netstack, CONTROLLER_AND_NAMESPACE).await, Err(_));

    // Can remove the same resource again if `idempotent` is specified.
    run_remove(&netstack, &[&["--idempotent"], CONTROLLER_AND_NAMESPACE].concat())
        .await
        .expect("remove same namespace");
}

#[test_case(&["--type", "ip", "--priority", "10"]; "IP routine with priority but no hook")]
#[test_case(&["--type", "nat", "--priority", "10"]; "NAT routine with priority but no hook")]
#[test_case(&["--type", "nat", "--hook", "forwarding"]; "NAT routine in FORWARDING hook")]
#[fuchsia::test]
async fn filter_create_routine_error(args: &[&'static str]) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let netstack = sandbox
        .create_netstack_realm::<Netstack3, _>("netstack")
        .expect("creating realm should succeed");

    const CONTROLLER: &str = "test";
    const NAMESPACE: &str = "foo";
    const ROUTINE: &str = "bar";

    run_create(&netstack, &["--controller", CONTROLLER, "namespace", "--name", NAMESPACE])
        .await
        .expect("create namespace");

    let routine = [
        &["--controller", CONTROLLER, "routine", "--namespace", NAMESPACE, "--name", ROUTINE],
        args,
    ]
    .concat();

    assert_matches!(run_create(&netstack, &routine).await, Err(_));
}

#[test_case(&["--src-port", "9999..=9999", "accept"]; "src port without transport protocol")]
#[test_case(&["--dst-port", "9999..=9999", "accept"]; "dst port without transport protocol")]
#[test_case(
    &["--src-port", "9999..=9999", "--transport-protocol", "icmp", "accept"];
    "src port with ICMP matcher"
)]
#[test_case(
    &["--src-port", "9999..=9999", "--transport-protocol", "icmpv6", "accept"];
    "src port with ICMPv6 matcher"
)]
#[test_case(&["tproxy"]; "transparent proxy with neither address nor port")]
#[test_case(&["tproxy", "--addr", "not-an-ip"]; "transparent proxy with invalid IP address")]
#[fuchsia::test]
async fn filter_create_rule_error(args: &[&'static str]) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let netstack = sandbox
        .create_netstack_realm::<Netstack3, _>("netstack")
        .expect("creating realm should succeed");

    const CONTROLLER: &str = "test";
    const NAMESPACE: &str = "foo";
    const ROUTINE: &str = "bar";

    run_create(&netstack, &["--controller", CONTROLLER, "namespace", "--name", NAMESPACE])
        .await
        .expect("create namespace");
    run_create(
        &netstack,
        &[
            "--controller",
            CONTROLLER,
            "routine",
            "--namespace",
            NAMESPACE,
            "--name",
            ROUTINE,
            "--type",
            "ip",
        ],
    )
    .await
    .expect("create routine");

    let rule = [
        &[
            "--controller",
            CONTROLLER,
            "rule",
            "--namespace",
            NAMESPACE,
            "--routine",
            ROUTINE,
            "--index",
            "42",
        ],
        args,
    ]
    .concat();

    assert_matches!(run_create(&netstack, &rule).await, Err(_));
}
