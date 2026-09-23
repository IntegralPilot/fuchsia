// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::collections::HashMap;
use std::pin::pin;

use component_events::events::{EventStream, ExitStatus, Stopped, StoppedPayload};
use component_events::matcher::EventMatcher;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fcomponent_decl;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_filter_ext::{
    self as fnet_filter_ext, Action, ControllerId, Domain, InstalledIpRoutine, InstalledNatRoutine,
    IpHook, MarkAction, Matchers, Namespace, NamespaceId, NatHook, RejectType, Resource,
    ResourceId, Routine, RoutineId, RoutineType, Rule, RuleId,
};
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_matchers_ext::TransportProtocol;
use fidl_fuchsia_process as fprocess;
use fuchsia_component_test::{RealmBuilder, RealmBuilderParams, RealmInstance};
use fuchsia_runtime::{HandleInfo, HandleType};
use log::info;
use test_case::test_case;

const IPTABLES_RESTORE: &'static str = "iptables_restore";
const IP6TABLES_RESTORE: &'static str = "ip6tables_restore";

const FILTER_TABLE_WITH_CUSTOM_CHAIN: &[&str] = &[
    "*filter",
    ":INPUT ACCEPT [0:0]",
    ":FORWARD ACCEPT [0:0]",
    ":OUTPUT ACCEPT [0:0]",
    ":test -",
    "COMMIT",
];

const NAT_TABLE_WITH_CUSTOM_CHAIN: &[&str] = &[
    "*nat",
    ":PREROUTING ACCEPT [0:0]",
    ":INPUT ACCEPT [0:0]",
    ":OUTPUT ACCEPT [0:0]",
    ":POSTROUTING ACCEPT [0:0]",
    ":test -",
    "COMMIT",
];

const MANGLE_TABLE_WITH_CUSTOM_CHAIN: &[&str] = &[
    "*mangle",
    ":PREROUTING ACCEPT [0:0]",
    ":INPUT ACCEPT [0:0]",
    ":FORWARD ACCEPT [0:0]",
    ":OUTPUT ACCEPT [0:0]",
    ":POSTROUTING ACCEPT [0:0]",
    ":test -",
    "COMMIT",
];

const MANGLE_TABLE_WITH_MARK_TARGET: &[&str] =
    &["*mangle", "-A INPUT -i lo -j MARK --set-mark 0x1/0x3", "COMMIT"];

const MANGLE_TABLE_WITH_NOOP_TARGET: &[&str] = &["*mangle", "-A INPUT -i lo", "COMMIT"];
const EBPF_LOADER: &'static str = "ebpf_loader";
const EBPF_PROGRAM_PIN_PATH: &'static str = "/sys/fs/bpf/test_program";
const FILTER_TABLE_WITH_BPF_MATCHER: &[&str] =
    &["*filter", "-A INPUT -m bpf --object-pinned /sys/fs/bpf/test_program", "COMMIT"];

struct TestRealm {
    realm: RealmInstance,
    realm_proxy: fcomponent::RealmProxy,
}

impl TestRealm {
    /// Starts test realm.
    async fn new(name: &str) -> Self {
        let builder = RealmBuilder::with_params(
            RealmBuilderParams::new().realm_name(name).from_relative_url("#meta/realm.cm"),
        )
        .await
        .expect("create realm builder");

        let realm = builder.build().await.unwrap();
        let realm_proxy: fcomponent::RealmProxy =
            realm.root.connect_to_protocol_at_exposed_dir().unwrap();

        TestRealm { realm, realm_proxy }
    }

    /// Runs component `name` in the realm feeding `input_lines` as stdin. Items
    /// in `input_lines` are suffixed with newline character, and sent one line
    /// at a time, and then stdin is closed. Checks that component exited with
    /// `ExitStatus::Clean` and returns the realm. Panics if any errors are
    /// encountered.
    async fn run_with_input(&self, name: &str, input_lines: &[&str]) {
        info!("Running {name}");

        let (stdin_recv, stdin_send) = zx::Socket::create_stream();

        for line in input_lines {
            info!("{name}: {line}");
            let bytes = format!("{line}\n").into_bytes();
            assert_eq!(stdin_send.write(&bytes).expect("write to stdin"), bytes.len());
        }

        self.realm_proxy
            .create_child(
                &fcomponent_decl::CollectionRef { name: "test-programs".to_string() },
                &fcomponent_decl::Child {
                    name: Some(name.to_string()),
                    url: Some(format!("#meta/{name}.cm")),
                    startup: Some(fcomponent_decl::StartupMode::Lazy),
                    ..Default::default()
                },
                fcomponent::CreateChildArgs {
                    numbered_handles: Some(vec![fprocess::HandleInfo {
                        id: HandleInfo::new(HandleType::FileDescriptor, libc::STDIN_FILENO as u16)
                            .as_raw(),
                        handle: stdin_recv.into(),
                    }]),
                    ..Default::default()
                },
            )
            .await
            .expect("fidl call to fuchsia.component.Realm/CreateChild")
            .expect("CreateChild successfully creates child component in collection");

        drop(stdin_send);

        let mut events = EventStream::open().await.unwrap();
        let event = EventMatcher::ok()
            .moniker(format!("realm_builder:{}/test-programs:{name}", self.realm.root.child_name()))
            .stop(None)
            .wait::<Stopped>(&mut events)
            .await
            .expect("wait for stopped event");
        let StoppedPayload { status, .. } = event.result().expect("extract event payload");
        assert_eq!(status, &ExitStatus::Clean);
    }

    async fn fetch_starnix_filter_state(
        &self,
    ) -> HashMap<fnet_filter_ext::ResourceId, fnet_filter_ext::Resource> {
        let state =
            self.realm.root.connect_to_protocol_at_exposed_dir().expect("connect to protocol");
        let stream =
            fnet_filter_ext::event_stream_from_state(state).expect("get filter event stream");
        let mut stream = pin!(stream);
        let mut all_resources: HashMap<_, _> =
            fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
        all_resources
            .remove(&ControllerId(String::from("starnix")))
            .expect("starnix should create controller")
    }
}

// Starnix's iptables subsystem prefixes table names with their IP version so
// that the IPv4 and IPv6 versions of a table do not conflict;
// fuchsia.net.filter requires that all namespaces owned by the same controller
// have unique names.
fn namespace_id_for_ipv4_table(table: &str) -> NamespaceId {
    NamespaceId(format!("ipv4-{table}"))
}
fn namespace_id_for_ipv6_table(table: &str) -> NamespaceId {
    NamespaceId(format!("ipv6-{table}"))
}

fn filter_table_with_custom_chain_resources(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let priority = 0;
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    let forward_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("FORWARD") };
    let output_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("OUTPUT") };
    let custom_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("test") };
    vec![
        Resource::Namespace(namespace),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalIngress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // FORWARD built-in routine
        Resource::Routine(Routine {
            id: forward_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Forwarding,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: forward_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // OUTPUT built-in routine
        Resource::Routine(Routine {
            id: output_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalEgress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: output_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // Custom routine
        Resource::Routine(Routine {
            id: custom_routine_id.clone(),
            routine_type: RoutineType::Ip(None),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: custom_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Return,
        }),
    ]
}

fn nat_table_with_custom_chain_resources(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let dnat_priority = -100;
    let snat_priority = 100;
    let prerouting_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("PREROUTING") };
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    let output_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("OUTPUT") };
    let postrouting_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("POSTROUTING") };
    let custom_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("test") };
    vec![
        Resource::Namespace(namespace),
        // PREROUTING built-in routine
        Resource::Routine(Routine {
            id: prerouting_routine_id.clone(),
            routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                hook: NatHook::Ingress,
                priority: dnat_priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: prerouting_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                hook: NatHook::LocalIngress,
                priority: snat_priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // OUTPUT built-in routine
        Resource::Routine(Routine {
            id: output_routine_id.clone(),
            routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                hook: NatHook::LocalEgress,
                priority: dnat_priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: output_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // POSTROUTING built-in routine
        Resource::Routine(Routine {
            id: postrouting_routine_id.clone(),
            routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                hook: NatHook::Egress,
                priority: snat_priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: postrouting_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // Custom routine
        Resource::Routine(Routine {
            id: custom_routine_id.clone(),
            routine_type: RoutineType::Nat(None),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: custom_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Return,
        }),
    ]
}

fn mangle_table_with_custom_chain_resources(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let priority = -150;
    let prerouting_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("PREROUTING") };
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    let forward_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("FORWARD") };
    let output_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("OUTPUT") };
    let postrouting_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("POSTROUTING") };
    let custom_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("test") };
    vec![
        Resource::Namespace(namespace),
        // PREROUTING built-in routine
        Resource::Routine(Routine {
            id: prerouting_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Ingress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: prerouting_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalIngress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // FORWARD built-in routine
        Resource::Routine(Routine {
            id: forward_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Forwarding,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: forward_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // OUTPUT built-in routine
        Resource::Routine(Routine {
            id: output_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalEgress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: output_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // POSTROUTING built-in routine
        Resource::Routine(Routine {
            id: postrouting_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::Egress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: postrouting_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }),
        // Custom routine
        Resource::Routine(Routine {
            id: custom_routine_id.clone(),
            routine_type: RoutineType::Ip(None),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: custom_routine_id, index: 0 },
            matchers: Matchers::default(),
            action: Action::Return,
        }),
    ]
}

fn mangle_table_with_input_marking(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let priority = -150;
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    vec![
        Resource::Namespace(namespace),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalIngress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id, index: 0 },
            matchers: Matchers {
                in_interface: Some(fnet_matchers_ext::Interface::Name("lo".into())),
                ..Default::default()
            },
            action: Action::Mark {
                domain: fnet::MARK_DOMAIN_SO_MARK,
                action: MarkAction::SetMark { clearing_mask: 0x3, mark: 0x1 },
            },
        }),
    ]
}

fn mangle_table_with_noop_target(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let priority = -150;
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    vec![
        Resource::Namespace(namespace),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalIngress,
                priority,
            })),
        }),
        Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id, index: 0 },
            matchers: Matchers {
                in_interface: Some(fnet_matchers_ext::Interface::Name("lo".into())),
                ..Default::default()
            },
            action: Action::None,
        }),
    ]
}

const FILTER_TABLE_WITH_REJECT_IPV4: &[&str] = &[
    "*filter",
    "-A INPUT -j REJECT --reject-with icmp-net-unreachable",
    "-A INPUT -j REJECT --reject-with icmp-host-unreachable",
    "-A INPUT -j REJECT --reject-with icmp-proto-unreachable",
    "-A INPUT -j REJECT --reject-with icmp-port-unreachable",
    "-A INPUT -j REJECT --reject-with icmp-net-prohibited",
    "-A INPUT -j REJECT --reject-with icmp-host-prohibited",
    "-A INPUT -j REJECT --reject-with icmp-admin-prohibited",
    "-A INPUT -p tcp -j REJECT --reject-with tcp-reset",
    "COMMIT",
];

fn filter_table_with_reject_ipv4_resources(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    let mut index = 0;
    let mut new_reject_rule = |type_| {
        let mut matchers = Matchers::default();
        if type_ == RejectType::TcpReset {
            matchers.transport_protocol =
                Some(TransportProtocol::Tcp { src_port: None, dst_port: None });
        }
        let r = Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id.clone(), index },
            matchers,
            action: Action::Reject(type_),
        });
        index += 1;
        r
    };
    vec![
        Resource::Namespace(namespace),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalIngress,
                priority: 0,
            })),
        }),
        new_reject_rule(RejectType::NetUnreachable),
        new_reject_rule(RejectType::HostUnreachable),
        new_reject_rule(RejectType::ProtoUnreachable),
        new_reject_rule(RejectType::PortUnreachable),
        new_reject_rule(RejectType::RoutePolicyFail),
        new_reject_rule(RejectType::RejectRoute),
        new_reject_rule(RejectType::AdminProhibited),
        new_reject_rule(RejectType::TcpReset),
    ]
}

const FILTER_TABLE_WITH_REJECT_IPV6: &[&str] = &[
    "*filter",
    "-A INPUT -j REJECT --reject-with icmp6-no-route",
    "-A INPUT -j REJECT --reject-with icmp6-addr-unreachable",
    "-A INPUT -j REJECT --reject-with icmp6-port-unreachable",
    "-A INPUT -j REJECT --reject-with icmp6-policy-fail",
    "-A INPUT -j REJECT --reject-with icmp6-reject-route",
    "-A INPUT -j REJECT --reject-with icmp6-adm-prohibited",
    "-A INPUT -p tcp -j REJECT --reject-with tcp-reset",
    "COMMIT",
];

fn filter_table_with_reject_ipv6_resources(namespace: Namespace) -> Vec<Resource> {
    let namespace_id = namespace.id.clone();
    let priority = 0;
    let input_routine_id =
        RoutineId { namespace: namespace_id.clone(), name: String::from("INPUT") };
    let mut index = 0;
    let mut new_reject_rule = |type_| {
        let mut matchers = Matchers::default();
        if type_ == RejectType::TcpReset {
            matchers.transport_protocol =
                Some(TransportProtocol::Tcp { src_port: None, dst_port: None });
        }
        let r = Resource::Rule(Rule {
            id: RuleId { routine: input_routine_id.clone(), index },
            matchers,
            action: Action::Reject(type_),
        });
        index += 1;
        r
    };
    vec![
        Resource::Namespace(namespace),
        // INPUT built-in routine
        Resource::Routine(Routine {
            id: input_routine_id.clone(),
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                hook: IpHook::LocalIngress,
                priority,
            })),
        }),
        new_reject_rule(RejectType::NetUnreachable),
        new_reject_rule(RejectType::HostUnreachable),
        new_reject_rule(RejectType::PortUnreachable),
        new_reject_rule(RejectType::RoutePolicyFail),
        new_reject_rule(RejectType::RejectRoute),
        new_reject_rule(RejectType::AdminProhibited),
        new_reject_rule(RejectType::TcpReset),
    ]
}

enum Ip {
    V4,
    V6,
}

impl Ip {
    fn iptables_restore(&self) -> &'static str {
        match self {
            Ip::V4 => IPTABLES_RESTORE,
            Ip::V6 => IP6TABLES_RESTORE,
        }
    }

    fn namespace(&self, table_name: &str) -> Namespace {
        match self {
            Ip::V4 => {
                Namespace { id: namespace_id_for_ipv4_table(table_name), domain: Domain::Ipv4 }
            }
            Ip::V6 => {
                Namespace { id: namespace_id_for_ipv6_table(table_name), domain: Domain::Ipv6 }
            }
        }
    }
}

#[test_case(
    Ip::V4,
    "filter",
    FILTER_TABLE_WITH_CUSTOM_CHAIN,
    filter_table_with_custom_chain_resources,
    "filter_ipv4";
    "filter chain ipv4"
)]
#[test_case(
    Ip::V6,
    "filter",
    FILTER_TABLE_WITH_CUSTOM_CHAIN,
    filter_table_with_custom_chain_resources,
    "filter_ipv6";
    "filter chain ipv6"
)]
#[test_case(
    Ip::V4,
    "nat",
    NAT_TABLE_WITH_CUSTOM_CHAIN,
    nat_table_with_custom_chain_resources,
    "nat_ipv4";
    "nat chain ipv4"
)]
#[test_case(
    Ip::V6,
    "nat",
    NAT_TABLE_WITH_CUSTOM_CHAIN,
    nat_table_with_custom_chain_resources,
    "nat_ipv6";
    "nat chain ipv6"
)]
#[test_case(
    Ip::V4,
    "mangle",
    MANGLE_TABLE_WITH_CUSTOM_CHAIN,
    mangle_table_with_custom_chain_resources,
    "mangle_ipv4";
    "mangle chain ipv4"
)]
#[test_case(
    Ip::V6,
    "mangle",
    MANGLE_TABLE_WITH_CUSTOM_CHAIN,
    mangle_table_with_custom_chain_resources,
    "mangle_ipv6";
    "mangle chain ipv6"
)]
#[test_case(
    Ip::V4,
    "mangle",
    MANGLE_TABLE_WITH_MARK_TARGET,
    mangle_table_with_input_marking,
    "mangle_mark_ipv4";
    "mangle chain mark ipv4"
)]
#[test_case(
    Ip::V6,
    "mangle",
    MANGLE_TABLE_WITH_MARK_TARGET,
    mangle_table_with_input_marking,
    "mangle_mark_ipv6";
    "mangle chain mark ipv6"
)]
#[test_case(
    Ip::V4,
    "mangle",
    MANGLE_TABLE_WITH_NOOP_TARGET,
    mangle_table_with_noop_target,
    "mangle_noop_ipv4";
    "mangle chain noop ipv4"
)]
#[test_case(
    Ip::V6,
    "mangle",
    MANGLE_TABLE_WITH_NOOP_TARGET,
    mangle_table_with_noop_target,
    "mangle_noop_ipv6";
    "mangle chain noop ipv6"
)]
#[test_case(
    Ip::V4,
    "filter",
    FILTER_TABLE_WITH_REJECT_IPV4,
    filter_table_with_reject_ipv4_resources,
    "filter_reject_ipv4";
    "filter chain reject ipv4"
)]
#[test_case(
    Ip::V6,
    "filter",
    FILTER_TABLE_WITH_REJECT_IPV6,
    filter_table_with_reject_ipv6_resources,
    "filter_reject_ipv6";
    "filter chain reject ipv6"
)]
#[fuchsia::test]
async fn create_chain(
    protocol: Ip,
    table_name: &str,
    table_spec: &[&'static str],
    expected_resources_fn: fn(Namespace) -> Vec<Resource>,
    case_name: &str,
) {
    let realm_name = format!("create_chain_{}", case_name);
    let realm = TestRealm::new(&realm_name).await;
    realm.run_with_input(protocol.iptables_restore(), table_spec).await;
    let starnix = realm.fetch_starnix_filter_state().await;

    for expected_resource in expected_resources_fn(protocol.namespace(table_name)) {
        let observed_resource = match starnix.get(&expected_resource.id()) {
            Some(resource) => resource,
            None => panic!("expected resource {expected_resource:?}; did not find in {starnix:?}"),
        };
        assert_eq!(observed_resource, &expected_resource);
    }
}

#[test_case(Ip::V6, FILTER_TABLE_WITH_BPF_MATCHER, "ipv6"; "filter chain ipv6")]
#[test_case(Ip::V4, FILTER_TABLE_WITH_BPF_MATCHER, "ipv4"; "filter chain ipv4")]
#[fuchsia::test]
async fn create_chain_with_bpf_matcher(protocol: Ip, table_spec: &[&'static str], case_name: &str) {
    let realm_name = format!("create_chain_with_bpf_matcher_{}", case_name);
    let realm = TestRealm::new(&realm_name).await;
    realm.run_with_input(EBPF_LOADER, &[EBPF_PROGRAM_PIN_PATH]).await;
    realm.run_with_input(protocol.iptables_restore(), table_spec).await;
    let starnix = realm.fetch_starnix_filter_state().await;

    let routine_id =
        RoutineId { namespace: protocol.namespace("filter").id, name: String::from("INPUT") };
    let rule_id = RuleId { routine: routine_id, index: 0 };
    let rule = starnix
        .get(&ResourceId::Rule(rule_id))
        .expect("expected routine {routine_id:#?}; did not find in {starnix:#?}");
    assert!(matches!(
        rule,
        Resource::Rule(Rule {
            id: _,
            matchers: Matchers { ebpf_program: Some(_), .. },
            action: Action::None
        })
    ));
}
