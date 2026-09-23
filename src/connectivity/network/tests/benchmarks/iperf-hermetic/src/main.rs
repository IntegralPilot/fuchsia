// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display;

use fidl::endpoints::ProtocolMarker as _;
use fidl_fuchsia_netemul as fnetemul;
use futures::FutureExt as _;
use net_types::ip::Ipv4;
use netstack_testing_common::realms::{
    KnownServiceProvider, Netstack, ProdNetstack3, TestSandboxExt as _, constants,
};
use netstack_testing_common::{
    get_component_stopped_event_stream, wait_for_component_stopped_with_stream,
};

const IPERF_URL: &str = "#meta/iperf.cm";
const NAME_PROVIDER_URL: &str = "#meta/device-name-provider.cm";
const NAME_PROVIDER_MONIKER: &str = "device-name-provider";
const PRIMARY_INTERFACE_CONFIGURATION: &str = "fuchsia.network.PrimaryInterface";
const NAMEGEN_CONFIGURATION: &str = "fuchsia.zircon.namegen";
const NODENAME_CONFIGURATION: &str = "fuchsia.zircon.nodename";
const BOOT_ITEM_INTERFACE_FOR_NODE_NAME_CONFIGURATION: &str =
    "fuchsia.device.BootItemInterfaceForNodeName";
const BOOT_ITEM_INTERFACE_FLIP_LOW_MAC_BIT_CONFIGURATION: &str =
    "fuchsia.device.BootItemInterfaceFlipLowMacBit";
const CUSTOM_ARTIFACTS_PATH: &str = "/custom_artifacts";

fn iperf_component<'a>(
    name: &str,
    program_args: impl IntoIterator<Item = &'a str>,
    eager_startup: bool,
) -> fnetemul::ChildDef {
    fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(IPERF_URL.to_string())),
        name: Some(name.to_string()),
        program_args: Some(program_args.into_iter().map(Into::into).collect::<Vec<_>>()),
        eager: Some(eager_startup),
        uses: Some(fnetemul::ChildUses::Capabilities(vec![
            fnetemul::Capability::LogSink(fnetemul::Empty),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: Some(constants::netstack::COMPONENT_NAME.to_string()),
                capability: Some(fnetemul::ExposedCapability::Protocol(
                    fidl_fuchsia_posix_socket::ProviderMarker::DEBUG_NAME.to_string(),
                )),
                ..Default::default()
            }),
            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                name: Some(NAME_PROVIDER_MONIKER.to_string()),
                capability: Some(fnetemul::ExposedCapability::Protocol(
                    fidl_fuchsia_device::NameProviderMarker::DEBUG_NAME.to_string(),
                )),
                ..Default::default()
            }),
            fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                variant: Some(fnetemul::StorageVariant::Tmp),
                path: Some("/tmp".to_string()),
                ..Default::default()
            }),
            fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                variant: Some(fnetemul::StorageVariant::CustomArtifacts),
                path: Some(CUSTOM_ARTIFACTS_PATH.to_string()),
                ..Default::default()
            }),
        ])),
        ..Default::default()
    }
}

fn device_name_provider_component() -> fnetemul::ChildDef {
    fnetemul::ChildDef {
        source: Some(fnetemul::ChildSource::Component(NAME_PROVIDER_URL.to_string())),
        name: Some(NAME_PROVIDER_MONIKER.to_string()),
        exposes: Some(vec![fidl_fuchsia_device::NameProviderMarker::DEBUG_NAME.to_string()]),
        uses: Some(fnetemul::ChildUses::Capabilities(vec![
            fnetemul::Capability::LogSink(fnetemul::Empty),
            fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
                capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(
                    PRIMARY_INTERFACE_CONFIGURATION.to_string(),
                )),
                ..Default::default()
            }),
            fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
                capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(
                    NAMEGEN_CONFIGURATION.to_string(),
                )),
                ..Default::default()
            }),
            fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
                capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(
                    NODENAME_CONFIGURATION.to_string(),
                )),
                ..Default::default()
            }),
            fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
                capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(
                    BOOT_ITEM_INTERFACE_FOR_NODE_NAME_CONFIGURATION.to_string(),
                )),
                ..Default::default()
            }),
            fidl_fuchsia_netemul::Capability::ChildDep(fidl_fuchsia_netemul::ChildDep {
                capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(
                    BOOT_ITEM_INTERFACE_FLIP_LOW_MAC_BIT_CONFIGURATION.to_string(),
                )),
                ..Default::default()
            }),
        ])),
        program_args: Some(vec!["--nodename".to_string(), "fuchsia-test-device".to_string()]),
        ..Default::default()
    }
}

async fn watch_for_exit(
    event_stream: &mut component_events::events::EventStream,
    realm: &netemul::TestRealm<'_>,
    component_moniker: &str,
) -> component_events::events::ExitStatus {
    let event =
        wait_for_component_stopped_with_stream(event_stream, &realm, component_moniker, None)
            .await
            .expect("observe stopped event");
    let component_events::events::StoppedPayload { status, .. } =
        event.result().expect("extract event payload");
    *status
}

#[derive(PartialEq)]
enum Protocol {
    Tcp,
    Udp,
}

impl std::str::FromStr for Protocol {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            s => Err(anyhow::anyhow!("unknown protocol {s}")),
        }
    }
}

impl Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp => write!(f, "tcp"),
            Self::Udp => write!(f, "udp"),
        }
    }
}

trait TestIpExt {
    const SERVER_FLAG: &'static str;
    const SERVER_ADDR: &'static str;
}

impl TestIpExt for net_types::ip::Ipv4 {
    const SERVER_FLAG: &'static str = "-4";
    const SERVER_ADDR: &'static str = "127.0.0.1";
}

impl TestIpExt for net_types::ip::Ipv6 {
    const SERVER_FLAG: &'static str = "-6";
    const SERVER_ADDR: &'static str = "::1";
}

#[derive(argh::FromArgs)]
/// Iperf3 loopback benchmarks.
struct Args {
    /// transport layer protocol (UDP or TCP)
    #[argh(option)]
    protocol: Protocol,

    /// message size
    #[argh(option)]
    message_size: u16,

    /// number of flows
    #[argh(option)]
    flows: u8,
}

#[fuchsia::main]
async fn main() {
    let Args { protocol, message_size, flows } = argh::from_env();
    // TODO(https://fxbug.dev/359670074): Consider adding IPv6 versions of
    // these benchmarks.
    bench::<ProdNetstack3, Ipv4>("bench", protocol, message_size, flows, true /* bench */).await;
}

async fn bench<N: Netstack, I: TestIpExt>(
    name: &str,
    protocol: Protocol,
    message_size: u16,
    flows: u8,
    bench: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    let client_moniker = |i| format!("iperf-client-{i}");
    let server_moniker = |i| format!("iperf-server-{i}");
    let message_size = message_size.to_string();
    let realm = sandbox
        .create_netstack_realm_with::<N, _, _>(
            name,
            std::iter::once(device_name_provider_component())
                .chain(bench.then_some(KnownServiceProvider::SecureStash.into()))
                .chain(
                    (0..flows)
                        .map(|i| {
                            // NB: The port numbers are arbitrarily chosen.
                            let port = (9001 + u16::from(i)).to_string();
                            let server_output_file =
                                format!("{CUSTOM_ARTIFACTS_PATH}/iperf_server_{i}.json");
                            let client_output_file =
                                format!("{CUSTOM_ARTIFACTS_PATH}/iperf_client_{i}.json");

                            let mut server_args =
                                vec!["-s", I::SERVER_FLAG, "--port", &port, "--json"];
                            let mut client_args: Vec<_> = [
                                "-c",
                                I::SERVER_ADDR,
                                "--port",
                                &port,
                                "--length",
                                &message_size,
                                "--json",
                                "--bitrate",
                                "0",
                                "--get-server-output",
                                if bench { "-t5" } else { "-n1" },
                            ]
                            .into_iter()
                            .chain((protocol == Protocol::Udp).then_some("-u"))
                            .collect();

                            if bench {
                                server_args.extend(&["--logfile", &server_output_file]);
                                client_args.extend(&["--logfile", &client_output_file]);
                            }

                            [
                                iperf_component(
                                    &server_moniker(i),
                                    server_args,
                                    /* eager */ true,
                                ),
                                iperf_component(
                                    &client_moniker(i),
                                    client_args,
                                    /* eager */ false,
                                ),
                            ]
                            .into_iter()
                        })
                        .flatten(),
                ),
        )
        .expect("create realm");

    // Ignore 'crashes' from iperf, since it reports things in its exit code
    // that can be seen as crashes in CF world and netemul.
    realm.ignore_checked_shutdown_monikers(
        (0..flows).map(|i| [client_moniker(i), server_moniker(i)].into_iter()).flatten(),
    );

    // Start the iPerf client until a successful run is observed.
    let realm_ref = &realm;
    let mut servers = futures::future::select_all((0..flows).map(|i| {
        Box::pin(async move {
            let mut stream = get_component_stopped_event_stream().await.expect("get event stream");
            watch_for_exit(&mut stream, realm_ref, &server_moniker(i)).await
        })
    }))
    .fuse();
    let mut clients = futures::future::join_all((0..flows).map(|i| async move {
        loop {
            // Create an event stream watcher before starting the client so we're sure to
            // observe its stopped event.
            let mut stream = get_component_stopped_event_stream().await.expect("get event stream");

            realm_ref.start_child_component(&client_moniker(i)).await.expect("start client");

            let status = watch_for_exit(&mut stream, realm_ref, &client_moniker(i)).await;
            if status == component_events::events::ExitStatus::Clean {
                return;
            }
        }
    }))
    .fuse();

    futures::select! {
        _ = clients => {},
        (status, index, _remaining_futs) = servers => {
            panic!(
                "servers should not stop but server with index {} exited with status {:?}",
                index, status,
            );
        },
    }
}

#[cfg(test)]
mod test {
    use futures::StreamExt as _;
    use netstack_testing_common::get_component_moniker;
    use netstack_testing_common::realms::Netstack3;
    use netstack_testing_macros::netstack_test;
    use test_case::test_case;

    use super::*;

    async fn wait_for_log(stream: diagnostics_reader::Subscription, log: &str) {
        stream
            .filter_map(|data| {
                futures::future::ready(
                    data.expect("stream error")
                        .msg()
                        .map(|msg| msg.contains(log))
                        .unwrap_or(false)
                        .then_some(()),
                )
            })
            .next()
            .await
            .expect("observe expected log");
    }

    #[netstack_test]
    async fn version(name: &str) {
        let sandbox = netemul::TestSandbox::new().expect("create sandbox");

        // Create an event stream watcher before starting iPerf so we're sure to observe
        // its stopped event.
        let mut component_stopped_event_stream =
            get_component_stopped_event_stream().await.expect("get event stream");

        const IPERF_MONIKER: &str = "iperf";
        let realm = sandbox
            .create_netstack_realm_with::<Netstack3, _, _>(
                name,
                [
                    device_name_provider_component(),
                    iperf_component(IPERF_MONIKER, ["-v"], /* eager */ true),
                ],
            )
            .expect("create realm");

        let iperf_moniker =
            get_component_moniker(&realm, IPERF_MONIKER).await.expect("get iperf moniker");
        let stream = diagnostics_reader::ArchiveReader::logs()
            .select_all_for_component(iperf_moniker.as_str())
            .snapshot_then_subscribe()
            .expect("subscribe to logs");

        let watch_exit_fut = async move {
            let status =
                watch_for_exit(&mut component_stopped_event_stream, &realm, IPERF_MONIKER).await;
            assert_eq!(status, component_events::events::ExitStatus::Clean);
        };

        let watch_log_fut = async move {
            const WAIT_FOR_LOG: &str = "iperf 3.7-FUCHSIA";
            wait_for_log(stream, WAIT_FOR_LOG).await;
        };

        let ((), ()) = futures::future::join(watch_exit_fut, watch_log_fut).await;
    }

    #[netstack_test]
    #[variant(I, Ip)]
    #[test_case(Protocol::Tcp; "tcp")]
    #[test_case(Protocol::Udp; "udp")]
    async fn loopback<I: TestIpExt>(name: &str, protocol: Protocol) {
        let name = &format!("{name}-{protocol}");
        bench::<Netstack3, I>(
            name, protocol, 1400,  /* message_size */
            1,     /* flows */
            false, /* bench */
        )
        .await;
    }
}
