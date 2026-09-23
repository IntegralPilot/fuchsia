// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

const NETSTACK_PROXY_URL: &'static str = "#meta/netstack-proxy.cm";
const NETSTACK_PROXY_NAME: &'static str = "netstack";

const MOCK_SERVICES_NAME: &'static str = "mock";

use fidl::prelude::*;
use fidl_fuchsia_net_stackmigrationdeprecated as fnet_migration;
use futures::{FutureExt as _, StreamExt as _};
use std::pin::pin;
use test_case::test_case;

async fn run_with_proxy_realm<
    'a,
    F: FnOnce(netemul::TestRealm<'a>) -> Fut,
    Fut: futures::Future<Output = ()> + 'a,
>(
    sandbox: &'a netemul::TestSandbox,
    netstack_version: fnet_migration::NetstackVersion,
    test: F,
) {
    let (mock_dir, server_end) = fidl::endpoints::create_endpoints();

    let proto_caps = [
        fnet_migration::StateMarker::PROTOCOL_NAME,
        fidl_fuchsia_process::LauncherMarker::PROTOCOL_NAME,
    ]
    .into_iter()
    .map(|proto| {
        fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
            name: Some(MOCK_SERVICES_NAME.to_string()),
            capability: Some(fidl_fuchsia_netemul::ExposedCapability::Protocol(proto.to_string())),
            ..Default::default()
        })
    });

    let config_caps = ["fuchsia.power.SuspendEnabled"].into_iter().map(|c| {
        fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
            // Route from void.
            name: None,
            capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(c.to_string())),
            ..Default::default()
        })
    });

    let version = match netstack_version {
        fnet_migration::NetstackVersion::Netstack2 => "ns2",
        fnet_migration::NetstackVersion::Netstack3 => "ns3",
    };

    let realm = sandbox
        .create_realm(
            format!("netstack-proxy_{version}"),
            [
                fidl_fuchsia_netemul::ChildDef {
                    source: Some(fidl_fuchsia_netemul::ChildSource::Component(
                        NETSTACK_PROXY_URL.to_string(),
                    )),
                    name: Some(NETSTACK_PROXY_NAME.to_string()),
                    uses: Some(fidl_fuchsia_netemul::ChildUses::Capabilities(
                        proto_caps
                            .chain(config_caps)
                            .chain(std::iter::once(fidl_fuchsia_netemul::Capability::LogSink(
                                fidl_fuchsia_netemul::Empty {},
                            )))
                            .collect(),
                    )),
                    exposes: Some(vec![
                        fidl_fuchsia_net_dhcp::ClientProviderMarker::PROTOCOL_NAME.to_string(),
                        fidl_fuchsia_net_interfaces::StateMarker::PROTOCOL_NAME.to_string(),
                    ]),
                    ..Default::default()
                },
                fidl_fuchsia_netemul::ChildDef {
                    source: Some(fidl_fuchsia_netemul::ChildSource::Mock(mock_dir)),
                    name: Some(MOCK_SERVICES_NAME.to_string()),
                    ..Default::default()
                },
            ],
        )
        .expect("create realm");

    let mut fs = fuchsia_component::server::ServiceFs::new();
    let _: &mut fuchsia_component::server::ServiceFsDir<'_, _> = fs
        .dir("svc")
        .add_proxy_service::<fidl_fuchsia_process::LauncherMarker, _>()
        .add_fidl_service(|rs: fnet_migration::StateRequestStream| rs);
    let _: &mut fuchsia_component::server::ServiceFs<_> =
        fs.serve_connection(server_end).expect("serve connection");

    let mut fs_fut = fs.fuse().flatten_unordered(None).for_each(|req| {
        match req.expect("error receiving migration request") {
            fnet_migration::StateRequest::GetNetstackVersion { responder } => {
                responder
                    .send(&fnet_migration::InEffectVersion {
                        current_boot: netstack_version.clone(),
                        automated: None,
                        user: None,
                    })
                    .expect("failed to send netstack version response");
                futures::future::ready(())
            }
        }
    });
    let test_fut = test(realm.clone()).fuse();
    let mut test_fut = pin!(test_fut);
    futures::select! {
        () = fs_fut => panic!("filesystem future ended unexpectedly"),
        () = test_fut => ()
    };
}

#[test_case(fnet_migration::NetstackVersion::Netstack2; "ns2")]
#[test_case(fnet_migration::NetstackVersion::Netstack3; "ns3")]
#[fuchsia::test]
async fn connects_to_stack(netstack_version: fnet_migration::NetstackVersion) {
    run_with_proxy_realm(
        &netemul::TestSandbox::new().expect("create sandbox"),
        netstack_version,
        |realm| async move {
            let state = realm
                .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
                .expect("connect to protocol");
            let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
                fidl_fuchsia_net_interfaces_ext::DefaultInterest,
            >(&state, Default::default())
            .expect("failed to create watcher");
            let _ = fidl_fuchsia_net_interfaces_ext::existing::<(), _, _>(
                event_stream,
                std::collections::HashMap::<u64, _>::new(),
            )
            .await
            .expect("listing existing interfaces");

            // TODO(https://fxbug.dev/42076541): Remove these checks once both
            // stacks use DHCP client. Netstack3 must serve the DHCP client
            // through itself to comply with netstack-proxy.
            let client_provider = realm
                .connect_to_protocol::<fidl_fuchsia_net_dhcp::ClientProviderMarker>()
                .expect("connect to protocol");
            let check_presence_result = client_provider.check_presence().await;
            assert_matches::assert_matches!(
                (netstack_version, check_presence_result),
                (fnet_migration::NetstackVersion::Netstack2, Err(_))
                    | (fnet_migration::NetstackVersion::Netstack3, Ok(()))
            );
        },
    )
    .await;
}
