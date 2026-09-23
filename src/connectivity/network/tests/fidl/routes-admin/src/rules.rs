// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;
use std::pin::pin;

use assert_matches::assert_matches;
use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_routes_ext::FidlRouteIpExt;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fidl_fuchsia_net_routes_ext::rules::{FidlRuleAdminIpExt, FidlRuleIpExt};
use fidl_fuchsia_posix_socket as fposix_socket;
use fnet_routes_ext::rules::{InstalledRule, RuleAction, RuleIndex, RuleMatcher, RuleSetPriority};
use futures::{AsyncReadExt as _, AsyncWriteExt as _, StreamExt as _};
use net_declare::fidl_subnet;
use net_types::ip::{GenericOverIp, Ip, IpInvariant, IpVersion};
use netemul::{RealmTcpListener as _, RealmTcpStream as _};
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use routes_common::{TestSetup, add_default_route_for_mark};

fn rule_set_err_stream<I: FidlRuleAdminIpExt>(
    rule_set: <I::RuleSetMarker as ProtocolMarker>::Proxy,
) -> futures::stream::BoxStream<'static, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct In<I: FidlRuleAdminIpExt>(<I::RuleSetMarker as ProtocolMarker>::Proxy);

    let IpInvariant(err_stream) = net_types::map_ip_twice!(I, In(rule_set), |In(rule_set)| {
        IpInvariant(
            rule_set
                .take_event_stream()
                .map(|result| match result {
                    Err(err) => err,
                })
                .boxed(),
        )
    });
    err_stream
}

#[netstack_test]
#[variant(I, Ip)]
async fn add_remove_rules<I: FidlRuleAdminIpExt + FidlRouteAdminIpExt + FidlRouteIpExt>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let TestSetup {
        realm,
        network: _network,
        interface: _,
        route_table,
        global_route_table: _,
        state: _,
    } = TestSetup::<I>::new(&sandbox, name).await;
    let rule_table =
        realm.connect_to_protocol::<I::RuleTableMarker>().expect("connect to rule table");
    let priority = fnet_routes_ext::rules::RuleSetPriority::from(0);
    let rule_set =
        fnet_routes_ext::rules::new_rule_set::<I>(&rule_table, priority).expect("fidl error");

    const RULE_INDEX_0: RuleIndex = RuleIndex::new(0);
    const RULE_INDEX_1: RuleIndex = RuleIndex::new(1);

    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RULE_INDEX_0,
        RuleMatcher::default(),
        RuleAction::Unreachable,
    )
    .await
    .expect("fidl error")
    .expect("failed to add a new rule");

    assert_matches!(
        fnet_routes_ext::rules::add_rule::<I>(
            &rule_set,
            RULE_INDEX_0,
            RuleMatcher::default(),
            RuleAction::Unreachable
        )
        .await,
        Ok(Err(fnet_routes_admin::RuleSetError::RuleAlreadyExists)),
        "cannot add a rule with an existing index"
    );

    assert_eq!(
        fnet_routes_ext::rules::add_rule::<I>(
            &rule_set,
            RuleIndex::from(1),
            RuleMatcher {
                locally_generated: Some(false),
                bound_device: Some(fnet_matchers_ext::BoundInterface::Bound(
                    fnet_matchers_ext::Interface::Name("lo".into())
                )),
                ..Default::default()
            },
            RuleAction::Unreachable,
        )
        .await
        .expect("fidl error"),
        Err(fnet_routes_admin::RuleSetError::InvalidMatcher),
        "cannot add a rule with an invalid matcher"
    );

    // Adding a rule with a different index should succeed.
    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RULE_INDEX_1,
        RuleMatcher::default(),
        RuleAction::Unreachable,
    )
    .await
    .expect("fidl error")
    .expect("failed to add rule with the index back after the old rule is removed");

    fnet_routes_ext::rules::remove_rule::<I>(&rule_set, RuleIndex::from(0))
        .await
        .expect("fidl error")
        .expect("failed to remove a rule");

    assert_matches!(
        fnet_routes_ext::rules::remove_rule::<I>(
            &rule_set,
            fnet_routes_ext::rules::RuleIndex::from(0),
        )
        .await,
        Ok(Err(fnet_routes_admin::RuleSetError::RuleDoesNotExist)),
        "cannot remove a rule with a non-existing index"
    );

    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RULE_INDEX_0,
        RuleMatcher::default(),
        RuleAction::Unreachable,
    )
    .await
    .expect("fidl error")
    .expect("failed to add rule with the index back after the old rule is removed");

    // Cannot add the rule set at the same priority.
    let new_rule_set =
        fnet_routes_ext::rules::new_rule_set::<I>(&rule_table, priority).expect("fidl error");
    let mut err_stream = rule_set_err_stream::<I>(new_rule_set);
    assert_matches!(
        err_stream.next().await,
        Some(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx::Status::ALREADY_EXISTS
    );
    assert_matches!(err_stream.next().await, None);

    fnet_routes_ext::rules::close_rule_set::<I>(rule_set).await.expect("fidl error");

    // Create a new rule set and we should be able to add a new rule.
    let new_rule_set =
        fnet_routes_ext::rules::new_rule_set::<I>(&rule_table, priority).expect("fidl error");

    let fnet_routes_admin::GrantForRouteTableAuthorization { table_id, token } =
        fnet_routes_ext::admin::get_authorization_for_route_table::<I>(&route_table)
            .await
            .expect("fidl error");

    assert_matches!(
        fnet_routes_ext::rules::add_rule::<I>(
            &new_rule_set,
            RULE_INDEX_0,
            RuleMatcher::default(),
            RuleAction::Lookup(fnet_routes_ext::TableId::new(table_id)),
        )
        .await,
        Ok(Err(fnet_routes_admin::RuleSetError::Unauthenticated)),
        "the rule set is not authenticated to the table"
    );

    fnet_routes_ext::rules::authenticate_for_route_table::<I>(&new_rule_set, table_id, token)
        .await
        .expect("fidl error")
        .expect("failed to authenticate");

    fnet_routes_ext::rules::add_rule::<I>(
        &new_rule_set,
        RULE_INDEX_0,
        RuleMatcher::default(),
        RuleAction::Lookup(fnet_routes_ext::TableId::new(table_id)),
    )
    .await
    .expect("fidl error")
    .expect("failed to add a new rule");
}

#[netstack_test]
#[variant(I, Ip)]
async fn bad_route_table_authentication<
    I: FidlRuleAdminIpExt + FidlRouteAdminIpExt + FidlRouteIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let TestSetup {
        realm,
        network: _network,
        interface: _,
        route_table,
        global_route_table: _,
        state: _,
    } = TestSetup::<I>::new(&sandbox, name).await;
    let rule_table =
        realm.connect_to_protocol::<I::RuleTableMarker>().expect("connect to rule table");
    let rule_set = fnet_routes_ext::rules::new_rule_set::<I>(
        &rule_table,
        fnet_routes_ext::rules::RuleSetPriority::from(0),
    )
    .expect("fidl error");

    let fnet_routes_admin::GrantForRouteTableAuthorization { table_id, token } =
        fnet_routes_ext::admin::get_authorization_for_route_table::<I>(&route_table)
            .await
            .expect("fidl error");

    // Invalid table id because of version mismatch.
    assert_matches!(
        fnet_routes_ext::rules::authenticate_for_route_table::<I>(
            &rule_set,
            table_id + 1,
            token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("failed to duplicate token")
        )
        .await,
        Ok(Err(fnet_routes_admin::AuthenticateForRouteTableError::InvalidAuthentication))
    );

    // Non-existent table that matches the IP version.
    assert_matches!(
        fnet_routes_ext::rules::authenticate_for_route_table::<I>(&rule_set, table_id + 2, token)
            .await,
        Ok(Err(fnet_routes_admin::AuthenticateForRouteTableError::InvalidAuthentication))
    );

    // Wrong token.
    assert_matches!(
        fnet_routes_ext::rules::authenticate_for_route_table::<I>(
            &rule_set,
            table_id,
            zx::Event::create(),
        )
        .await,
        Ok(Err(fnet_routes_admin::AuthenticateForRouteTableError::InvalidAuthentication))
    );
}

#[netstack_test]
#[variant(I, Ip)]
async fn table_removal_removes_rules<
    I: FidlRouteAdminIpExt + FidlRouteIpExt + FidlRuleIpExt + FidlRuleAdminIpExt,
>(
    name: &str,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let TestSetup {
        realm,
        network: _network,
        interface: _,
        route_table,
        global_route_table: _,
        state,
    } = TestSetup::<I>::new(&sandbox, name).await;
    let main_table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&route_table).await.expect("get table id");
    let route_table_provider = realm
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("connect to main route table");
    let user_route_table =
        fnet_routes_ext::admin::new_route_table::<I>(&route_table_provider, None)
            .expect("create new user table");
    let user_table_id =
        fnet_routes_ext::admin::get_table_id::<I>(&user_route_table).await.expect("get table id");
    let rule_table =
        realm.connect_to_protocol::<I::RuleTableMarker>().expect("connect to the rule table");
    let grant = fnet_routes_ext::admin::get_authorization_for_route_table::<I>(&user_route_table)
        .await
        .expect("fidl error");
    let rule_set = fnet_routes_ext::rules::new_rule_set::<I>(&rule_table, RuleSetPriority::from(0))
        .expect("new rule set");
    fnet_routes_ext::rules::authenticate_for_route_table::<I>(
        &rule_set,
        grant.table_id,
        grant.token,
    )
    .await
    .expect("fidl error")
    .expect("invalid authentication");
    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RuleIndex::from(0),
        RuleMatcher::default(),
        RuleAction::Lookup(user_table_id),
    )
    .await
    .expect("fidl error")
    .expect("failed to add rule");

    let rule_events = pin!(
        fnet_routes_ext::rules::rule_event_stream_from_state::<I>(&state).expect("get rule stream")
    );
    let rules = fnet_routes_ext::rules::collect_rules_until_idle::<I, HashSet<_>>(rule_events)
        .await
        .expect("failed to collect events");
    let default_rule = InstalledRule {
        priority: fnet_routes_ext::rules::DEFAULT_RULE_SET_PRIORITY,
        index: RuleIndex::from(0),
        matcher: Default::default(),
        action: RuleAction::Lookup(main_table_id),
    };
    // We have two rules: the one we just added and the default rule that exists from the beginning.
    assert_eq!(
        rules,
        HashSet::from_iter([
            InstalledRule {
                priority: RuleSetPriority::from(0),
                index: RuleIndex::from(0),
                matcher: Default::default(),
                action: RuleAction::Lookup(user_table_id)
            },
            default_rule.clone(),
        ])
    );

    fnet_routes_ext::admin::remove_route_table::<I>(&user_route_table)
        .await
        .expect("fidl error")
        .expect("remove table");
    let rule_events = pin!(
        fnet_routes_ext::rules::rule_event_stream_from_state::<I>(&state).expect("get rule stream")
    );
    let rules = fnet_routes_ext::rules::collect_rules_until_idle::<I, Vec<_>>(rule_events)
        .await
        .expect("failed to collect events");
    // Now only the default rule should exist.
    assert_eq!(rules, &[default_rule]);
}

#[netstack_test]
#[variant(I, Ip)]
async fn multi_network<I: FidlRuleAdminIpExt + FidlRouteAdminIpExt + FidlRouteIpExt>(name: &str) {
    // This test sets up 2 networks and 3 hosts. There is 1 server in each network separately and 1
    // client that has 2 interfaces that connect to the 2 networks. This test also sets up routes
    // and rules on the client that will direct to different interfaces based on the socket mark.
    // Then it verifies that in this setting, we can make TCP connections from the client to each
    // server.
    struct SetupConfig {
        server_1_subnet: fnet::Subnet,
        server_2_subnet: fnet::Subnet,
        client_subnet_1: fnet::Subnet,
        client_subnet_2: fnet::Subnet,
    }

    let config = match I::VERSION {
        IpVersion::V4 => SetupConfig {
            server_1_subnet: fidl_subnet!("192.168.1.2/24"),
            server_2_subnet: fidl_subnet!("192.168.0.2/24"),
            client_subnet_1: fidl_subnet!("192.168.1.1/24"),
            client_subnet_2: fidl_subnet!("192.168.0.1/24"),
        },
        IpVersion::V6 => SetupConfig {
            server_1_subnet: fidl_subnet!("fd00:0:0:1::2/64"),
            server_2_subnet: fidl_subnet!("fd00:0:0:2::2/64"),
            client_subnet_1: fidl_subnet!("fd00:0:0:1::1/64"),
            client_subnet_2: fidl_subnet!("fd00:0:0:2::1/64"),
        },
    };

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    let server_1_ip = fidl_fuchsia_net_ext::IpAddress::from(config.server_1_subnet.addr).0;
    let server_2_ip = fidl_fuchsia_net_ext::IpAddress::from(config.server_2_subnet.addr).0;

    let net_1 = sandbox.create_network("net_1").await.expect("create network");
    let net_2 = sandbox.create_network("net_2").await.expect("create network");
    let server_1 = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_server_1", name))
        .expect("create realm");
    let server_2 = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_server_2", name))
        .expect("create realm");
    let client = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_client", name))
        .expect("create realm");

    let server_1_iface = server_1
        .join_network(&net_1, "server_1-ep")
        .await
        .expect("install interface in server_1 netstack");
    server_1_iface
        .add_address_and_subnet_route(config.server_1_subnet)
        .await
        .expect("configure address");
    server_1_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
    let server_2_iface = server_2
        .join_network(&net_2, "server_2-ep")
        .await
        .expect("install interface in server_2 netstack");
    server_2_iface
        .add_address_and_subnet_route(config.server_2_subnet)
        .await
        .expect("configure address");
    server_2_iface.apply_nud_flake_workaround().await.expect("nud flake workaround");
    let client_iface_1 = client
        .join_network(&net_1, "client-ep_1")
        .await
        .expect("install interface in client netstack");
    client_iface_1
        .add_address_and_subnet_route(config.client_subnet_1)
        .await
        .expect("configure address");
    client_iface_1.apply_nud_flake_workaround().await.expect("nud flake workaround");
    let client_iface_2 = client
        .join_network(&net_2, "client-ep_2")
        .await
        .expect("install interface in client netstack");
    client_iface_2
        .add_address_and_subnet_route(config.client_subnet_2)
        .await
        .expect("configure address");
    client_iface_2.apply_nud_flake_workaround().await.expect("nud flake workaround");

    let rule_table =
        client.connect_to_protocol::<I::RuleTableMarker>().expect("connect to rule table");
    let route_table_provider = client
        .connect_to_protocol::<I::RouteTableProviderMarker>()
        .expect("connect to route table provider");
    let rule_set = fnet_routes_ext::rules::new_rule_set::<I>(
        &rule_table,
        fnet_routes_ext::rules::RuleSetPriority::from(0),
    )
    .expect("new rule set");

    const SO_MARK_1: u32 = 0;
    const SO_MARK_2: u32 = 1;
    const SO_MARK_3: u32 = 100;
    const PORT: u16 = 8080;
    const REQUEST: &str = "hello from client";
    const RESPONSE: &str = "hello from server";

    let _rs1 = add_default_route_for_mark::<I>(
        &route_table_provider,
        &rule_set,
        &client_iface_1,
        None,
        SO_MARK_1,
        routes_common::MarkMatcher::MatchMarked(SO_MARK_1),
        routes_common::MarkMatcher::DontMatch,
    )
    .await;
    let _rs2 = add_default_route_for_mark::<I>(
        &route_table_provider,
        &rule_set,
        &client_iface_2,
        None,
        SO_MARK_2,
        routes_common::MarkMatcher::MatchMarked(SO_MARK_2),
        routes_common::MarkMatcher::DontMatch,
    )
    .await;

    // Now add the rule to make anything else unreachable.
    fnet_routes_ext::rules::add_rule::<I>(
        &rule_set,
        RuleIndex::new(100),
        Default::default(),
        fnet_routes_ext::rules::RuleAction::Unreachable,
    )
    .await
    .expect("fidl")
    .expect("add rule");

    async fn connect_in_realm_with_mark(
        realm: &netemul::TestRealm<'_>,
        sockaddr: std::net::SocketAddr,
        mark: u32,
    ) -> Result<fuchsia_async::net::TcpStream, anyhow::Error> {
        fuchsia_async::net::TcpStream::connect_in_realm_with_sock(realm, sockaddr, |sock| {
            let channel = fdio::clone_channel(&sock).expect("failed to get the underlying channel");
            let proxy = fposix_socket::StreamSocketSynchronousProxy::new(channel);
            proxy
                .set_mark(
                    fnet::MarkDomain::Mark1,
                    &fposix_socket::OptionalUint32::Value(mark),
                    zx::Instant::INFINITE,
                )
                .expect("fidl")
                .expect("set mark");
            Ok(())
        })
        .await
    }

    for (server_ip, mark, realm) in
        [(server_1_ip, SO_MARK_1, &server_1), (server_2_ip, SO_MARK_2, &server_2)]
    {
        let sockaddr = std::net::SocketAddr::from((server_ip, PORT));
        let client = async {
            let mut stream = connect_in_realm_with_mark(&client, sockaddr, mark)
                .await
                .expect("failed to connect");
            let request = REQUEST.as_bytes();
            assert_eq!(stream.write(request).await.expect("write to stream"), request.len());
            stream.flush().await.expect("flush stream");

            let mut buffer = [0; 512];
            let read = stream.read(&mut buffer).await.expect("read from stream");
            let response = String::from_utf8_lossy(&buffer[0..read]);
            assert_eq!(response, RESPONSE, "got unexpected response from server: {}", response);

            // Try a second connect with a different mark. This time it should fail.
            let err = connect_in_realm_with_mark(&client, sockaddr, SO_MARK_3)
                .await
                .expect_err("should fail");
            let io_err = err.downcast_ref::<std::io::Error>().expect("wrong error");
            // TODO(https://github.com/rust-lang/rust/issues/86442): Use error kind.
            assert_eq!(
                io_err.raw_os_error().expect("should be an OS error"),
                fidl_fuchsia_posix::Errno::Enetunreach as i32
            );
        };

        let listener = fuchsia_async::net::TcpListener::listen_in_realm(&realm, sockaddr)
            .await
            .expect("bind to address");
        let server = async {
            let (_listener, mut stream, _remote) =
                listener.accept().await.expect("accept incoming connection");
            let mut buffer = [0; 512];
            let read = stream.read(&mut buffer).await.expect("read from stream");
            let request = String::from_utf8_lossy(&buffer[0..read]);
            assert_eq!(request, REQUEST, "got unexpected request from client: {}", request);

            let response = RESPONSE.as_bytes();
            assert_eq!(stream.write(response).await.expect("write to stream"), response.len());
            stream.flush().await.expect("flush stream");
        };

        futures::join!(client, server);
    }
}
