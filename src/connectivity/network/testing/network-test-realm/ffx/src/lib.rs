// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42156465): Replace with GN config once available in an ffx_plugin.
#![warn(unused_results)]

use anyhow::Context as _;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_developer_remotecontrol as fremotecontrol;
use fdomain_fuchsia_net as fnet;
use fdomain_fuchsia_net_dhcpv6 as fnet_dhcpv6;
use fdomain_fuchsia_net_test_realm as fntr;
use fdomain_fuchsia_sys2 as fsys;
use ffx_net_test_realm_args as ntr_args;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use fidl_fuchsia_net_dhcpv6_ext as fnet_dhcpv6_ext;
use fidl_fuchsia_net_ext as fnet_ext;
use log::error;
use target_holders::RemoteControlProxyHolder;

async fn connect_to_protocol<S: fdomain_client::fidl::DiscoverableProtocolMarker>(
    remote_control: &fremotecontrol::RemoteControlProxy,
    moniker: &str,
) -> anyhow::Result<S::Proxy> {
    let (proxy, server_end) = remote_control.domain().create_proxy::<S>();
    remote_control
        .connect_capability(
            moniker,
            fsys::OpenDirType::ExposedDir,
            S::PROTOCOL_NAME,
            server_end.into_channel(),
        )
        .await?
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to connect to {} at {}: {:?}. Did you forget to run ffx component start?",
                S::PROTOCOL_NAME,
                moniker,
                e
            )
        })?;
    Ok(proxy)
}
#[derive(FfxTool)]
#[check(AvailabilityFlag("net.test.realm"))]
pub struct NetTestRealmTool {
    remote_control: RemoteControlProxyHolder,
    #[command]
    cmd: ntr_args::Command,
}

fho::embedded_plugin!(NetTestRealmTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for NetTestRealmTool {
    type Writer = MachineWriter<String>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        net_test_realm(self.remote_control, self.cmd, writer).await.map_err(Into::into)
    }
}

async fn net_test_realm(
    remote_control: RemoteControlProxyHolder,
    mut cmd: ntr_args::Command,
    writer: MachineWriter<String>,
) -> anyhow::Result<()> {
    // The tool was called with a selector, make the arg an moniker.
    // TODO: remove once all clients of this tool are passing monikers.
    if !cmd.component_moniker.starts_with("/") {
        let moniker = cmd.component_moniker.replace("\\:", ":");
        cmd.component_moniker = format!("/{moniker}");
    }
    let controller =
        connect_to_protocol::<fntr::ControllerMarker>(&remote_control, &cmd.component_moniker)
            .await?;
    handle_command(controller, cmd.subcommand, writer).await
}

async fn handle_command(
    controller: fntr::ControllerProxy,
    command: ntr_args::Subcommand,
    mut writer: MachineWriter<String>,
) -> anyhow::Result<()> {
    let (result, method_name) = match command {
        ntr_args::Subcommand::AddInterface(ntr_args::AddInterface {
            mac_address,
            name,
            wait_any_ip_address,
        }) => (
            controller.add_interface(&mac_address.into(), &name, wait_any_ip_address).await,
            "add_interface",
        ),
        ntr_args::Subcommand::JoinMulticastGroup(ntr_args::JoinMulticastGroup {
            address,
            interface_id,
        }) => (
            {
                let now = std::time::Instant::now();
                writer
                    .item(&format!(
                        "starting join_multicast_group op at {now:?}: {address:?} {interface_id:?}"
                    ))
                    .map_err(|e| anyhow::anyhow!(e))?;
                let result = controller.join_multicast_group(&address.into(), interface_id).await;

                let elapsed = std::time::Instant::now() - now;
                writer
                    .item(&format!(
                        "finishing join_multicast_group op from {now:?}: \
                     {address:?} {interface_id:?}: {result:?} (after {elapsed:?})"
                    ))
                    .map_err(|e| anyhow::anyhow!(e))?;
                result
            },
            "join_multicast_group",
        ),
        ntr_args::Subcommand::LeaveMulticastGroup(ntr_args::LeaveMulticastGroup {
            address,
            interface_id,
        }) => (
            {
                let now = std::time::Instant::now();
                writer.item(&format!(
                    "starting leave_multicast_group op at {now:?}: {address:?} {interface_id:?}",
                )).map_err(|e| anyhow::anyhow!(e))?;
                let result = controller.leave_multicast_group(&address.into(), interface_id).await;

                let elapsed = std::time::Instant::now() - now;
                writer
                    .item(&format!(
                        "finishing leave_multicast_group op from {now:?}: \
                    {address:?} {interface_id:?}: {result:?} (after {elapsed:?})",
                    ))
                    .map_err(|e| anyhow::anyhow!(e))?;
                result
            },
            "leave_multicast_group",
        ),
        ntr_args::Subcommand::Ping(ntr_args::Ping {
            target,
            payload_length,
            timeout,
            interface_name,
        }) => (
            controller
                .ping(&target.into(), payload_length, interface_name.as_deref(), timeout)
                .await,
            "ping",
        ),
        ntr_args::Subcommand::PollUdp(ntr_args::PollUdp {
            target,
            payload,
            timeout,
            num_retries,
        }) => (
            async move {
                controller
                    .poll_udp(
                        &fnet_ext::SocketAddress(target).into(),
                        payload.as_bytes(),
                        timeout,
                        num_retries,
                    )
                    .await
                    .map(|ntr_result| {
                        ntr_result.and_then(|bytes| {
                            let received = std::str::from_utf8(&bytes).map_err(|e| {
                                error!("error parsing {:?} as utf8: {:?}", bytes, e);
                                fntr::Error::Internal
                            })?;
                            let _ = writer.item(&format!("{received}"));
                            Ok(())
                        })
                    })
            }
            .await,
            "poll_udp",
        ),
        ntr_args::Subcommand::StartHermeticNetworkRealm(ntr_args::StartHermeticNetworkRealm {
            netstack,
        }) => (
            controller.start_hermetic_network_realm(netstack).await,
            "start_hermetic_network_realm",
        ),
        ntr_args::Subcommand::StartStub(ntr_args::StartStub { component_url }) => {
            (controller.start_stub(&component_url).await, "start_stub")
        }
        ntr_args::Subcommand::StopHermeticNetworkRealm(ntr_args::StopHermeticNetworkRealm {}) => {
            (controller.stop_hermetic_network_realm().await, "stop_hermetic_network_realm")
        }
        ntr_args::Subcommand::StopStub(ntr_args::StopStub {}) => {
            (controller.stop_stub().await, "stop_stub")
        }
        ntr_args::Subcommand::Dhcpv6Client(ntr_args::Dhcpv6Client { subcommand }) => {
            match subcommand {
                ntr_args::Dhcpv6ClientSubcommand::Start(ntr_args::Dhcpv6ClientStart {
                    interface_id,
                    address,
                    request_non_temporary_address,
                    request_dns_servers,
                    prefix_delegation_config,
                }) => (
                    controller
                        .start_dhcpv6_client(
                            &fnet_dhcpv6_ext::NewClientParams {
                                interface_id: interface_id,
                                address: fnet::Ipv6SocketAddress {
                                    address: address.into(),
                                    port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
                                    zone_index: interface_id,
                                },
                                config: fnet_dhcpv6_ext::ClientConfig {
                                    information_config: fnet_dhcpv6_ext::InformationConfig {
                                        dns_servers: request_dns_servers,
                                    },
                                    non_temporary_address_config: fnet_dhcpv6_ext::AddressConfig {
                                        address_count: if request_non_temporary_address {
                                            1
                                        } else {
                                            0
                                        },
                                        preferred_addresses: None,
                                    },
                                    prefix_delegation_config: prefix_delegation_config.map(
                                        |pd_config| match pd_config {
                                            None => fnet_dhcpv6::PrefixDelegationConfig::Empty(
                                                fnet_dhcpv6::Empty,
                                            ),
                                            Some(fnet_ext::SubnetV6 {
                                                addr: fnet_ext::Ipv6Address(addr),
                                                prefix_len,
                                            }) if addr.is_unspecified() => {
                                                fnet_dhcpv6::PrefixDelegationConfig::PrefixLength(
                                                    prefix_len,
                                                )
                                            }
                                            Some(subnet) => {
                                                fnet_dhcpv6::PrefixDelegationConfig::Prefix(
                                                    subnet.into(),
                                                )
                                            }
                                        },
                                    ),
                                },
                                duid: (request_non_temporary_address
                                    || prefix_delegation_config.is_some())
                                .then(|| {
                                    fnet_dhcpv6::Duid::Uuid(uuid::Uuid::new_v4().into_bytes())
                                }),
                            }
                            .into(),
                        )
                        .await,
                    "start_dhcpv6_client",
                ),
                ntr_args::Dhcpv6ClientSubcommand::Stop(ntr_args::Dhcpv6ClientStop {}) => {
                    (controller.stop_dhcpv6_client().await, "stop_dhcpv6_client")
                }
            }
        }
        ntr_args::Subcommand::DhcpClient(ntr_args::DhcpClient { subcommand }) => match subcommand {
            ntr_args::DhcpClientSubcommand::Start(ntr_args::DhcpClientStart { interface_id }) => (
                controller
                    .start_out_of_stack_dhcpv4_client(
                        &fntr::ControllerStartOutOfStackDhcpv4ClientRequest {
                            interface_id: Some(interface_id),
                            ..Default::default()
                        },
                    )
                    .await,
                "start_out_of_stack_dhcpv4_client",
            ),
            ntr_args::DhcpClientSubcommand::Stop(ntr_args::DhcpClientStop { interface_id }) => (
                controller
                    .stop_out_of_stack_dhcpv4_client(
                        &fntr::ControllerStopOutOfStackDhcpv4ClientRequest {
                            interface_id: Some(interface_id),
                            ..Default::default()
                        },
                    )
                    .await,
                "stop_out_of_stack_dhcpv4_client",
            ),
        },
    };

    result
        .context(format!("{} failed", method_name))?
        .map_err(|e| anyhow::format_err!("{} error: {:?}", method_name, e))
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use fdomain_fuchsia_net as fnet;
    use futures::TryStreamExt as _;
    use net_declare::{fidl_ip, fidl_ip_v6_with_prefix, fidl_mac, std_ip_v6};
    use std::sync::Arc;

    const COMPONENT_URL: &'static str =
        "fuchsia-pkg://fuchsia.com/fake-component#meta/fake-component.cm";
    const INTERFACE_ID: u64 = 1;
    const INTERFACE_NAME: &'static str = "eth1";
    const IP_ADDRESS: fnet::IpAddress = fidl_ip!("192.168.0.1");
    const MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:03:04:05:06:07");

    /// Executes the `command` and routes the emitted request to the provided
    /// `response_handler`.
    ///
    /// The `response_handler` should validate the request and output the
    /// desired response.
    async fn net_test_realm_command_test<F: FnOnce(fntr::ControllerRequest)>(
        client: &Arc<fdomain_client::Client>,
        command: ntr_args::Subcommand,
        response_handler: F,
    ) {
        let (controller, mut requests) = client.create_proxy_and_stream::<fntr::ControllerMarker>();
        let writer = MachineWriter::<String>::new(None);
        let op = handle_command(controller, command, writer);
        let op_response = async {
            let request = requests
                .try_next()
                .await
                .expect("controller request stream error")
                .expect("controller request not found");
            response_handler(request);
            Ok(())
        };
        futures::future::try_join(op, op_response).await.expect("net_test_realm command failed");
    }

    #[fuchsia::test]
    async fn add_interface() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::AddInterface(ntr_args::AddInterface {
                mac_address: MAC_ADDRESS.into(),
                name: INTERFACE_NAME.to_string(),
                wait_any_ip_address: false,
            }),
            |request| {
                let (mac_address, name, wait_ip_address, responder) =
                    request.into_add_interface().expect("expected request of type AddInterface");
                assert_eq!(MAC_ADDRESS, mac_address);
                assert_eq!(INTERFACE_NAME.to_string(), name);
                assert_eq!(wait_ip_address, false);
                responder.send(Ok(())).expect("failed to send AddInterface response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn join_multicast_group() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::JoinMulticastGroup(ntr_args::JoinMulticastGroup {
                address: IP_ADDRESS.into(),
                interface_id: INTERFACE_ID,
            }),
            |request| {
                let (ip_address, interface_id, responder) = request
                    .into_join_multicast_group()
                    .expect("expected request of type JoinMulticastGroup");
                assert_eq!(IP_ADDRESS, ip_address);
                assert_eq!(INTERFACE_ID, interface_id);
                responder.send(Ok(())).expect("failed to send JoinMulticastGroup response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn leave_multicast_group() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::LeaveMulticastGroup(ntr_args::LeaveMulticastGroup {
                address: IP_ADDRESS.into(),
                interface_id: INTERFACE_ID,
            }),
            |request| {
                let (ip_address, interface_id, responder) = request
                    .into_leave_multicast_group()
                    .expect("expected request of type LeaveMulticastGroup");
                assert_eq!(IP_ADDRESS, ip_address);
                assert_eq!(INTERFACE_ID, interface_id);
                responder.send(Ok(())).expect("failed to send LeaveMulticastGroup response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn ping() {
        let client = fdomain_local::local_client_empty();
        let expected_payload_length = 100;
        let expected_timeout: i64 = 1000;
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::Ping(ntr_args::Ping {
                target: IP_ADDRESS.into(),
                payload_length: expected_payload_length,
                timeout: expected_timeout,
                interface_name: Some(INTERFACE_NAME.to_string()),
            }),
            |request| {
                let (target, payload_length, interface_name, timeout, responder) =
                    request.into_ping().expect("expected request of type Ping");
                assert_eq!(IP_ADDRESS, target);
                assert_eq!(expected_payload_length, payload_length);
                assert_eq!(Some(INTERFACE_NAME.to_string()), interface_name);
                assert_eq!(expected_timeout, timeout);
                responder.send(Ok(())).expect("failed to send Ping response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn poll_udp() {
        let client = fdomain_local::local_client_empty();
        let expected_port = 1234;
        let expected_target = (std::net::Ipv4Addr::LOCALHOST, expected_port).into();
        let expected_timeout = 1000;
        let expected_payload = "hello";
        let expected_num_retries = 10;
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::PollUdp(ntr_args::PollUdp {
                target: expected_target,
                payload: expected_payload.to_string(),
                timeout: expected_timeout,
                num_retries: expected_num_retries,
            }),
            |request| {
                let (target, payload, timeout, num_retries, responder) =
                    request.into_poll_udp().expect("expected request of type PollUdp");
                assert_eq!(
                    fnet_ext::SocketAddress::from(target),
                    fnet_ext::SocketAddress(expected_target)
                );
                assert_eq!(payload, expected_payload.as_bytes());
                assert_eq!(timeout, expected_timeout);
                assert_eq!(num_retries, expected_num_retries);
                responder.send(Ok(&payload)).expect("failed to send PollUdp response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn start_hermetic_network_realm() {
        let client = fdomain_local::local_client_empty();
        let expected_netstack = fntr::Netstack::V2;
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::StartHermeticNetworkRealm(ntr_args::StartHermeticNetworkRealm {
                netstack: expected_netstack,
            }),
            |request| {
                let (netstack, responder) = request
                    .into_start_hermetic_network_realm()
                    .expect("expected request of type StartHermeticNetworkRealm");
                assert_eq!(expected_netstack, netstack);
                responder.send(Ok(())).expect("failed to send StartHermeticNetworkRealm response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn start_stub() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::StartStub(ntr_args::StartStub {
                component_url: COMPONENT_URL.to_string(),
            }),
            |request| {
                let (component_url, responder) =
                    request.into_start_stub().expect("expected request of type StartStub");
                assert_eq!(COMPONENT_URL.to_string(), component_url);
                responder.send(Ok(())).expect("failed to send StartStub response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn stop_hermetic_network_realm() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::StopHermeticNetworkRealm(ntr_args::StopHermeticNetworkRealm {}),
            |request| {
                request
                    .into_stop_hermetic_network_realm()
                    .expect("expected request of type StopHermeticNetworkRealm")
                    .send(Ok(()))
                    .expect("failed to send StopHermeticNetworkRealm response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn stop_stub() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::StopStub(ntr_args::StopStub {}),
            |request| {
                request
                    .into_stop_stub()
                    .expect("expected request of type StopStub")
                    .send(Ok(()))
                    .expect("failed to send StopStub response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn dhcpv6_client_start() {
        const DHCPV6_CLIENT_BIND_ADDR: fnet_ext::Ipv6Address =
            fnet_ext::Ipv6Address(std_ip_v6!("fe80::1"));
        const PD_HINT: fnet::Ipv6AddressWithPrefix = fidl_ip_v6_with_prefix!("2001:db8::/32");
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::Dhcpv6Client(ntr_args::Dhcpv6Client {
                subcommand: ntr_args::Dhcpv6ClientSubcommand::Start(ntr_args::Dhcpv6ClientStart {
                    interface_id: INTERFACE_ID,
                    address: DHCPV6_CLIENT_BIND_ADDR,
                    request_non_temporary_address: true,
                    request_dns_servers: true,
                    prefix_delegation_config: Some(Some(PD_HINT.into())),
                }),
            }),
            |request| {
                let (params, responder) = request
                    .into_start_dhcpv6_client()
                    .expect("expected request of type StartDhcpv6Client");
                let params: fnet_dhcpv6_ext::NewClientParams =
                    params.try_into().expect("NewClientParams should pass FIDL table validation");
                assert_eq!(
                    params,
                    fnet_dhcpv6_ext::NewClientParams {
                        interface_id: INTERFACE_ID,
                        address: fnet::Ipv6SocketAddress {
                            address: DHCPV6_CLIENT_BIND_ADDR.into(),
                            port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
                            zone_index: INTERFACE_ID,
                        },
                        config: fnet_dhcpv6_ext::ClientConfig {
                            information_config: fnet_dhcpv6_ext::InformationConfig {
                                dns_servers: true,
                            },
                            non_temporary_address_config: fnet_dhcpv6_ext::AddressConfig {
                                address_count: 1,
                                preferred_addresses: None,
                            },
                            prefix_delegation_config: Some(
                                fnet_dhcpv6::PrefixDelegationConfig::Prefix(PD_HINT)
                            ),
                        },
                        // Don't check the DUID as it's a randomly generated UUID.
                        duid: params.duid.clone(),
                    },
                );
                responder.send(Ok(())).expect("failed to send StartDhcpv6Client response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn dhcpv6_client_stop() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::Dhcpv6Client(ntr_args::Dhcpv6Client {
                subcommand: ntr_args::Dhcpv6ClientSubcommand::Stop(ntr_args::Dhcpv6ClientStop {}),
            }),
            |request| {
                request
                    .into_stop_dhcpv6_client()
                    .expect("expected request of type StopDhcpv6Client")
                    .send(Ok(()))
                    .expect("failed to send StopDhcpv6Client response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn dhcp_client_start() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::DhcpClient(ntr_args::DhcpClient {
                subcommand: ntr_args::DhcpClientSubcommand::Start(ntr_args::DhcpClientStart {
                    interface_id: INTERFACE_ID,
                }),
            }),
            |request| {
                let (request, responder) = request
                    .into_start_out_of_stack_dhcpv4_client()
                    .expect("expected request of type StartOutOfStackDhcpv4Client");
                assert_matches!(
                    request,
                    fntr::ControllerStartOutOfStackDhcpv4ClientRequest {
                        interface_id: Some(interface_id),
                        ..
                    } if interface_id == INTERFACE_ID
                );
                responder
                    .send(Ok(()))
                    .expect("failed to send StartOutOfStackDhcpv4Client response");
            },
        )
        .await;
    }

    #[fuchsia::test]
    async fn dhcp_client_stop() {
        let client = fdomain_local::local_client_empty();
        net_test_realm_command_test(
            &client,
            ntr_args::Subcommand::DhcpClient(ntr_args::DhcpClient {
                subcommand: ntr_args::DhcpClientSubcommand::Stop(ntr_args::DhcpClientStop {
                    interface_id: INTERFACE_ID,
                }),
            }),
            |request| {
                let (request, responder) = request
                    .into_stop_out_of_stack_dhcpv4_client()
                    .expect("expected request of type StopOutOfStackDhcpv4Client");
                assert_matches!(
                    request,
                    fntr::ControllerStopOutOfStackDhcpv4ClientRequest {
                        interface_id: Some(interface_id),
                        ..
                    } if interface_id == INTERFACE_ID
                );
                responder.send(Ok(())).expect("failed to send StopOutOfStackDhcpv4Client response");
            },
        )
        .await;
    }
}
