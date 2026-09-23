// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A networking stack.
#![warn(clippy::unused_async)]
#![warn(missing_docs, unreachable_patterns, unused)]
#![recursion_limit = "512"]

mod bindings;

use std::num::NonZeroU8;

use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_process_lifecycle as fprocess_lifecycle;
use fuchsia_async as fasync;
use fuchsia_async::SendExecutorBuilder;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use futures::{Future, StreamExt as _};
use log::{error, info, warn};

use bindings::{GlobalConfig, InspectPublisher, InterfaceConfigDefaults, NetstackSeed, Service};

/// Runs Netstack3.
pub fn main() {
    let mut config = ns3_config::Config::take_from_startup_handle();
    if config.max_rolling_capture_buffer_size < fidl_fuchsia_net_debug::MIN_BUFFER_SIZE {
        warn!(
            "max rolling capture buffer size configured as {} will be clamped to the minimum {}",
            config.max_rolling_capture_buffer_size,
            fidl_fuchsia_net_debug::MIN_BUFFER_SIZE,
        );
        config.max_rolling_capture_buffer_size = fidl_fuchsia_net_debug::MIN_BUFFER_SIZE;
    };
    let ns3_config::Config {
        num_threads,
        debug_logs,
        opaque_iids,
        suspend_enabled,
        sampled_stats_enabled,
        multi_vmo,
        max_rolling_capture_buffer_size,
    } = &config;
    let num_threads = NonZeroU8::new(*num_threads).expect("invalid 0 thread count value");
    let mut executor = SendExecutorBuilder::new().num_threads(num_threads.get().into()).build();

    let mut log_options = diagnostics_log::PublishOptions::default();

    // NB: netstack3 is usually launched with a 'netstack' moniker already -
    // which implies an automatic 'netstack' tag. However, the automatic tag has
    // shown problems when extra tags are present in specific log lines (e.g.
    // https://fxbug.dev/390252317, https://fxbug.dev/390252218). Given that,
    // we always initialize with the netstack tag here.
    log_options = log_options.tags(&["netstack"]);

    if *debug_logs {
        // When forcing debug logs, disable all the dynamic features from the
        // logging framework, we want logs pegged at Severity::Debug.
        log_options = log_options
            .minimum_severity(diagnostics_log::Severity::Debug)
            .listen_for_interest_updates(false);
    }
    diagnostics_log::initialize(log_options).expect("failed to initialize log");

    fuchsia_trace_provider::trace_provider_create_with_fdio();

    info!("starting netstack3 with {config:?}");

    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs
        .dir("svc")
        // TODO(https://fxbug.dev/42076541): This is transitional. Once the
        // out-of-stack DHCP client is being used by both netstacks, it
        // should be moved out of the netstack realm and into the network
        // realm. The trip through Netstack3 allows for availability of DHCP
        // client to be dependent on Netstack version when using
        // netstack-proxy.
        .add_proxy_service::<fidl_fuchsia_net_dhcp::ClientProviderMarker, _>()
        .add_fidl_service(Service::Control)
        .add_service_connector(Service::DebugDiagnostics)
        .add_fidl_service(Service::DebugInterfaces)
        .add_fidl_service(Service::DnsServerWatcher)
        .add_fidl_service(Service::FilterControl)
        .add_fidl_service(Service::FilterState)
        .add_fidl_service(Service::HealthCheck)
        .add_fidl_service(Service::Interfaces)
        .add_fidl_service(Service::InterfacesAdmin)
        .add_fidl_service(Service::MulticastAdminV4)
        .add_fidl_service(Service::MulticastAdminV6)
        .add_fidl_service(Service::NdpWatcher)
        .add_fidl_service(Service::Neighbor)
        .add_fidl_service(Service::NeighborController)
        .add_fidl_service(Service::PacketSocket)
        .add_fidl_service(Service::PacketCapture)
        .add_fidl_service(Service::RawSocket)
        .add_fidl_service(Service::RootFilter)
        .add_fidl_service(Service::RootInterfaces)
        .add_fidl_service(Service::RootRoutesV4)
        .add_fidl_service(Service::RootRoutesV6)
        .add_fidl_service(Service::RoutesAdminV4)
        .add_fidl_service(Service::RoutesAdminV6)
        .add_fidl_service(Service::RoutesState)
        .add_fidl_service(Service::RoutesStateV4)
        .add_fidl_service(Service::RoutesStateV6)
        .add_fidl_service(Service::RouteTableProviderV4)
        .add_fidl_service(Service::RouteTableProviderV6)
        .add_fidl_service(Service::RuleTableV4)
        .add_fidl_service(Service::RuleTableV6)
        .add_fidl_service(Service::SettingsControl)
        .add_fidl_service(Service::SettingsState)
        .add_fidl_service(Service::Socket)
        .add_fidl_service(Service::SocketControl)
        .add_fidl_service(Service::SocketDiagnostics)
        .add_fidl_service(Service::Stack)
        .add_fidl_service(Service::WakeGroupProvider);

    let seed = NetstackSeed::new(
        GlobalConfig {
            suspend_enabled: *suspend_enabled,
            sampled_stats_enabled: *sampled_stats_enabled,
            multi_vmo: *multi_vmo,
            max_rolling_capture_buffer_size: *max_rolling_capture_buffer_size,
        },
        &InterfaceConfigDefaults { opaque_iids: *opaque_iids },
    );

    let inspect_publisher = InspectPublisher::new();
    inspect_publisher
        .inspector()
        .root()
        .record_child("Config", |config_node| config.record_inspect(config_node));

    let _: &mut ServiceFs<_> = fs.take_and_serve_directory_handle().expect("directory handle");

    // Short circuit when we receive a lifecycle stop request.
    let fs = fs.take_until(get_lifecycle_stop_fut());

    executor.run(seed.serve(fs, inspect_publisher))
}

/// Takes the lifecycle handle from startup and returns a future that resolves
/// whenever the system has requested shutdown.
fn get_lifecycle_stop_fut() -> impl Future<Output = ()> {
    // Lifecycle handle takes no args, must be set to zero.
    // See zircon/processargs.h.
    const LIFECYCLE_HANDLE_ARG: u16 = 0;
    let handle = fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
        fuchsia_runtime::HandleType::Lifecycle,
        LIFECYCLE_HANDLE_ARG,
    ))
    .expect("missing lifecycle handle");

    async move {
        let mut request_stream = fprocess_lifecycle::LifecycleRequestStream::from_channel(
            fasync::Channel::from_channel(handle.into()).into(),
        );
        loop {
            match request_stream.next().await {
                Some(Ok(fprocess_lifecycle::LifecycleRequest::Stop { control_handle })) => {
                    info!("received shutdown request");
                    // Shutdown request is acknowledged by the lifecycle
                    // channel shutting down. Intentionally leak the channel
                    // so it'll only be closed on process termination,
                    // allowing clean process termination to always be
                    // observed.

                    // Must drop the control_handle to unwrap the
                    // lifecycle channel.
                    std::mem::drop(control_handle);
                    let (inner, _terminated): (_, bool) = request_stream.into_inner();
                    let inner = std::sync::Arc::try_unwrap(inner)
                        .expect("failed to retrieve lifecycle channel");
                    let inner: zx::Channel = inner.into_channel().into_zx_channel();
                    std::mem::forget(inner);
                    break;
                }
                Some(Err(e)) => error!("observed error in lifecycle request stream: {e:?}"),
                None => {
                    // Something really bad must've happened here. We chose
                    // not to panic because the system must be in a bad
                    // state, log an error and hold forever.
                    error!("lifecycle channel closed");
                    futures::future::pending::<()>().await;
                }
            }
        }
    }
}
