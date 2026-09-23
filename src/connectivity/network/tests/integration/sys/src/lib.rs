// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_component::server::{ServiceFs, ServiceFsDir, ServiceObj};
use futures::StreamExt as _;
use netemul::{TestRealm, TestSandbox};
use netstack_testing_common::ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, Netstack2, Netstack3, NetstackVersion, TestSandboxExt as _,
    constants,
};
use netstack_testing_macros::netstack_test;
use std::borrow::Cow;

const MOCK_SERVICES_NAME: &str = "mock";

fn create_netstack_with_mock_endpoint<'s, RS, N>(
    sandbox: &'s TestSandbox,
    name: &'s str,
) -> (TestRealm<'s>, ServiceFs<ServiceObj<'s, RS>>)
where
    RS: RequestStream + 'static,
    RS::Protocol: DiscoverableProtocolMarker,
    N: Netstack,
{
    let mut netstack: fnetemul::ChildDef =
        netstack_testing_common::realms::KnownServiceProvider::Netstack(
            match N::VERSION {
                // The prod ns2 has a route for
                // fuchsia.scheduler.deprecated.ProfileProvider which is needed for tests
                // in this suite.
                NetstackVersion::Netstack2 { tracing: false, fast_udp: false } => NetstackVersion::ProdNetstack2,
                v @ NetstackVersion::Netstack3 => v,
                v @ (NetstackVersion::Netstack2 { tracing: _, fast_udp: _ }
                | NetstackVersion::ProdNetstack2
                | NetstackVersion::ProdNetstack3
                ) => {
                    panic!("netstack_test should only be parameterized with Netstack2 or Netstack3: got {:?}", v);
                }
            }
        )
            .into();
    {
        let fnetemul::ChildUses::Capabilities(capabilities) =
            netstack.uses.as_mut().expect("empty uses");

        // Remove any existing ChildDep capability with the same name as the endpoint
        // we wish to add.
        let new_capability_name = RS::Protocol::PROTOCOL_NAME.to_string();
        capabilities.retain(|cap| {
            match cap {
                fnetemul::Capability::ChildDep(child_dep) => {
                    match &child_dep.capability {
                        Some(fnetemul::ExposedCapability::Protocol(name)) => {
                            name != &new_capability_name
                        }
                        // Keep deps without a capability or ExposedCapability
                        // that is not a Protocol.
                        _ => true,
                    }
                }
                // Keep all other capability variants.
                _ => true,
            }
        });

        // Add the new capability.
        capabilities.push(fnetemul::Capability::ChildDep(fnetemul::ChildDep {
            name: Some(MOCK_SERVICES_NAME.to_string()),
            capability: Some(fnetemul::ExposedCapability::Protocol(new_capability_name)),
            ..Default::default()
        }));
    }

    let (mock_dir, server_end) = fidl::endpoints::create_endpoints();
    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service_at(RS::Protocol::PROTOCOL_NAME, |rs: RS| rs);
    let _: &mut ServiceFs<_> = fs.serve_connection(server_end).expect("serve connection");

    let realm = sandbox
        .create_realm(
            name,
            [
                netstack,
                (&netstack_testing_common::realms::KnownServiceProvider::SecureStash).into(),
                fnetemul::ChildDef {
                    source: Some(fnetemul::ChildSource::Mock(mock_dir)),
                    name: Some(MOCK_SERVICES_NAME.to_string()),
                    ..Default::default()
                },
            ],
        )
        .expect("failed to create realm");

    // Connect to any service to get netstack launched.
    let _: fidl_fuchsia_net_interfaces::StateProxy = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    (realm, fs)
}

#[netstack_test]
#[variant(N, Netstack)]
async fn ns_sets_thread_profiles<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let (_realm, mut fs) = create_netstack_with_mock_endpoint::<
        fidl_fuchsia_scheduler_deprecated::ProfileProviderRequestStream,
        N,
    >(&sandbox, name);

    let profile_provider_request_stream = fs.next().await.expect("fs terminated unexpectedly");

    #[derive(Default, Debug)]
    struct ExpectProfiles {
        sysmon: bool,
        worker: bool,
    }

    impl ExpectProfiles {
        fn all_done(&self) -> bool {
            let Self { sysmon, worker } = self;
            *sysmon && *worker
        }

        fn update(&mut self, profile: &str) {
            let Self { sysmon, worker } = self;
            match profile {
                "fuchsia.netstack.go-worker" => {
                    *worker = true;
                }
                "fuchsia.netstack.go-sysmon" => {
                    assert!(!*sysmon, "sysmon observed more than once");
                    *sysmon = true;
                }
                other => panic!("unexpected profile {other}"),
            }
        }
    }

    let result = async_utils::fold::fold_while(
        profile_provider_request_stream,
        ExpectProfiles::default(),
        |mut expect, r| {
            let (thread, profile, responder) =
                r.expect("request failure").into_set_profile_by_role().expect("unexpected request");
            expect.update(profile.as_str());
            assert_eq!(
                thread.basic_info().expect("failed to get basic info").rights,
                zx::Rights::TRANSFER | zx::Rights::MANAGE_THREAD
            );
            responder.send(zx::sys::ZX_OK).expect("failed to respond");

            futures::future::ready(if expect.all_done() {
                async_utils::fold::FoldWhile::Done(())
            } else {
                async_utils::fold::FoldWhile::Continue(expect)
            })
        },
    )
    .await;
    result.short_circuited().expect("didn't observe all profiles installed");
}

#[netstack_test]
#[variant(N, Netstack)]
async fn ns_persist_tags_under_size_limits<N: Netstack>(name: &str) {
    persist_tags_under_size_limits(name, N::VERSION.into()).await
}

#[netstack_test]
async fn dhcp_client_persist_tags_under_size_limits(name: &str) {
    persist_tags_under_size_limits(name, PersistenceTestCase::DhcpClient).await
}

async fn persist_tags_under_size_limits(name: &str, test_case: PersistenceTestCase) {
    test_persistence(name, test_case, |inspect_payload, tag, tag_config| {
        // Convert inspect payload to a JSON string.
        let data = serde_json::to_string(&inspect_payload).expect("serialization failed");

        // Assert data to be persisted obeys size constraints specified in
        // configuration.
        assert!(data.len() > 0);
        assert!(
            data.len() <= tag_config.max_bytes,
            "{}: data = {}, max = {}",
            tag,
            data.len(),
            tag_config.max_bytes
        );
    })
    .await
}

#[netstack_test]
#[variant(N, Netstack)]
async fn ns_persist_root_inspect_nodes_for_selectors<N: Netstack>(name: &str) {
    persist_root_inspect_nodes_for_selectors(name, N::VERSION.into()).await
}

#[netstack_test]
async fn dhcp_client_persist_root_inspect_nodes_for_selectors(name: &str) {
    persist_root_inspect_nodes_for_selectors(name, PersistenceTestCase::DhcpClient).await
}

// This test validates that for any given selector in the config, the root
// inspect node specified in that selector has been persisted in an archivist
// payload.
//
// TODO(https://fxbug.dev/42076420): Note that this test does NOT validate that
// child nodes specified using wildcards (e.g. `Foo/*/*:*`) are present in the
// archivist payload, nor that all child nodes of any given selector are
// persisted. We're still relying on fireteam primaries and netstack developers
// to keep the persist file in sync with our inspect logic.
async fn persist_root_inspect_nodes_for_selectors(name: &str, test_case: PersistenceTestCase) {
    test_persistence(name, test_case, |inspect_payload, _tag, tag_config| {
        for selector in tag_config.selectors.iter() {
            // Retrieve the root Inspect node from the diagnostics selector.
            let root_node = match &selector.tree_selector {
                Some(fidl_fuchsia_diagnostics::TreeSelector::SubtreeSelector(
                    fidl_fuchsia_diagnostics::SubtreeSelector { node_path },
                )) => node_path.first().unwrap(),
                Some(fidl_fuchsia_diagnostics::TreeSelector::PropertySelector(
                    fidl_fuchsia_diagnostics::PropertySelector { node_path, target_properties: _ },
                )) => node_path.first().unwrap(),
                None => panic!("empty TreeSelector"),
                _ => panic!("unknown TreeSelector variant {:?}", selector.tree_selector),
            };

            let root_node_name = match root_node {
                fidl_fuchsia_diagnostics::StringSelector::StringPattern(s) => s,
                fidl_fuchsia_diagnostics::StringSelector::ExactMatch(s) => s,
                _ => panic!("unknown StringSelector variant {:?}", root_node),
            };

            // Strip the wildcard tree selector prefix used in Netstack2's
            // persistence selectors.
            let root_node_name = root_node_name.strip_prefix("[...]").unwrap_or(root_node_name);

            // Assert payload has the node name specified in the selector.
            assert_eq!(root_node_name, &inspect_payload.name);
        }
    })
    .await
}

fn persistence_tag_to_ns2_diagnostics_dir(tag: &persistence_config::Tag) -> Cow<'static, str> {
    match tag.as_str() {
        "fidl" => Cow::from("fidlStats"),
        "nics" => Cow::from("interfaces"),
        "runtime" => Cow::from("configuration"),
        other => Cow::from(format!("{other}")),
    }
}

enum PersistenceTestCase {
    Netstack2,
    Netstack3,
    DhcpClient,
}

impl From<NetstackVersion> for PersistenceTestCase {
    fn from(netstack_version: NetstackVersion) -> Self {
        match netstack_version {
            NetstackVersion::ProdNetstack2 | NetstackVersion::Netstack2 { .. } => {
                PersistenceTestCase::Netstack2
            }
            NetstackVersion::ProdNetstack3 | NetstackVersion::Netstack3 => {
                PersistenceTestCase::Netstack3
            }
        }
    }
}

impl PersistenceTestCase {
    // Returns the path to the persit configuration.
    fn config_path(&self) -> &'static str {
        match self {
            PersistenceTestCase::Netstack2 => "/pkg/data/netstack.persist",
            PersistenceTestCase::Netstack3 => "/pkg/data/netstack3.persist",
            PersistenceTestCase::DhcpClient => "/pkg/data/dhcp_client.persist",
        }
    }

    // Returns the component name of the component that serves the inspect data.
    fn component_name(&self) -> &'static str {
        match self {
            PersistenceTestCase::Netstack2 | PersistenceTestCase::Netstack3 => "netstack",
            PersistenceTestCase::DhcpClient => "dhcp-client",
        }
    }

    // Returns the service name declared in the persit configuration.
    fn service_name(&self) -> &'static str {
        match self {
            PersistenceTestCase::Netstack2 | PersistenceTestCase::Netstack3 => "netstack",
            PersistenceTestCase::DhcpClient => "dhcp-client",
        }
    }
}

async fn test_persistence<F>(name: &str, test_case: PersistenceTestCase, validate_payload: F)
where
    F: Fn(
        diagnostics_reader::DiagnosticsHierarchy,
        &persistence_config::Tag,
        &persistence_config::TagConfig,
    ) -> (),
{
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = match test_case {
        PersistenceTestCase::Netstack2 => sandbox.create_netstack_realm::<Netstack2, _>(name),
        PersistenceTestCase::Netstack3 => sandbox.create_netstack_realm::<Netstack3, _>(name),
        PersistenceTestCase::DhcpClient => sandbox.create_netstack_realm_with::<Netstack3, _, _>(
            name,
            &[KnownServiceProvider::DhcpClient],
        ),
    }
    .expect("create realm");

    let config = persistence_config::load_configuration_files_from(test_case.config_path())
        .expect("load configuration files failed");

    // Create a socket to ensure socket Inspect data is available.
    let _socket = match test_case {
        PersistenceTestCase::Netstack2 | PersistenceTestCase::Netstack3 => Some(
            realm
                .datagram_socket(
                    fposix_socket::Domain::Ipv4,
                    fposix_socket::DatagramSocketProtocol::Udp,
                )
                .await
                .expect("datagram socket creation failed"),
        ),
        PersistenceTestCase::DhcpClient => None,
    };

    // Connect to the DHCP protocol to ensure the DHCP client starts and makes
    // Inspect data available.
    let _dhcp_client = match test_case {
        PersistenceTestCase::Netstack2 | PersistenceTestCase::Netstack3 => None,
        PersistenceTestCase::DhcpClient => Some(
            realm
                .connect_to_protocol::<fidl_fuchsia_net_dhcp::ClientProviderMarker>()
                .expect("failed to connect to DHCP client"),
        ),
    };

    // The realm moniker is needed to construct the component part of an Inspect
    // selector.
    let moniker = realm.get_moniker().await.expect("get moniker failed");
    let realm_moniker = match test_case {
        PersistenceTestCase::Netstack2 => {
            // Because Netstack2 uses the deprecated diagnostics API, it needs
            // to use a sanitized moniker. The `ArchiveReader` used to gather
            // Netstack3/DhcpClient data will sanitize the moniker internally.
            selectors::sanitize_moniker_for_selectors(&moniker)
        }
        PersistenceTestCase::Netstack3 | PersistenceTestCase::DhcpClient => moniker,
    };

    const SANDBOX_MONIKER: &str = "sandbox";

    let tags = config.get(test_case.service_name()).expect("service not present");
    for (tag, tag_config) in tags {
        // Modify selectors to use test realm moniker.
        let selectors = tag_config
            .selectors
            .iter()
            // Raw selector strings have the schema
            // <type>:<component>:<subtree>:<property>. Extract the subtree portion
            // of the selector, and combine it with a test realm specific component
            // selector.
            .map(|selector| {
                fidl_fuchsia_diagnostics::Selector {
                    component_selector: Some(fidl_fuchsia_diagnostics::ComponentSelector {
                        moniker_segments: Some(vec![
                            fidl_fuchsia_diagnostics::StringSelector::ExactMatch(
                                SANDBOX_MONIKER.to_string(),
                            ),
                            fidl_fuchsia_diagnostics::StringSelector::ExactMatch(
                                realm_moniker.to_string(),
                            ),
                            fidl_fuchsia_diagnostics::StringSelector::ExactMatch(
                                test_case.component_name().to_string(),
                            ),
                        ]),
                        ..Default::default()
                    }),
                    ..selector.clone().into()
                }
                .into()
            });

        let inspect_payload = match test_case {
            PersistenceTestCase::Netstack2 => {
                let diagnostics_dir =
                    realm.open_diagnostics_directory(test_case.component_name()).unwrap();
                let subdir = persistence_tag_to_ns2_diagnostics_dir(tag);
                netstack_testing_common::get_deprecated_netstack2_inspect_data(
                    &diagnostics_dir,
                    &subdir,
                    selectors,
                )
                .await
            }
            PersistenceTestCase::Netstack3 | PersistenceTestCase::DhcpClient => {
                // Retrieve the inspect payload from the archivist.
                let mut archive_reader = diagnostics_reader::ArchiveReader::inspect();
                let archive_reader = archive_reader.add_selectors(selectors);
                let payload = archive_reader
                    .with_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT)
                    .snapshot()
                    .await
                    .expect("snapshot failed")
                    .into_iter()
                    .filter_map(|v| v.payload)
                    .next();
                match payload {
                    Some(p) => p,
                    None => panic!("No payload in snapshot for tag={tag}."),
                }
            }
        };

        // Assert on payload.
        validate_payload(inspect_payload, tag, tag_config);
    }
}

#[netstack_test]
#[variant(N, Netstack)]
async fn serves_ota_health_check<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create netstack realm");
    let health_check = realm
        .connect_to_protocol::<fidl_fuchsia_update_verify::ComponentOtaHealthCheckMarker>()
        .expect("connect to protocol");

    let response = health_check.get_health_status().await.expect("call succeeded");
    assert_eq!(response, fidl_fuchsia_update_verify::HealthStatus::Healthy);
}

#[netstack_test]
#[variant(N, Netstack)]
async fn emits_logs<N: Netstack>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<N, _>(name).expect("create netstack realm");
    // Start the netstack.
    let _ = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let netstack_moniker =
        netstack_testing_common::get_component_moniker(&realm, constants::netstack::COMPONENT_NAME)
            .await
            .expect("get netstack moniker");
    let mut stream = diagnostics_reader::ArchiveReader::logs()
        .select_all_for_component(netstack_moniker.as_str())
        .snapshot_then_subscribe()
        .expect("subscribe to netstack logs");
    let payload = stream
        .next()
        .await
        .expect("netstack should emit logs on startup")
        .expect("extract syslogs from archivist payload");
    assert!(payload.msg().is_some(), "syslog should contain message");
}
