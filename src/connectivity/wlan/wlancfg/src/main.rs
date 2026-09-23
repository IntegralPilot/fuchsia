// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The complexity of a separate struct doesn't seem universally better than having many arguments
#![allow(clippy::too_many_arguments)]
#![recursion_limit = "1024"]
// Turn on additional lints that could lead to unexpected crashes in production code
#![warn(clippy::indexing_slicing)]
#![cfg_attr(test, allow(clippy::indexing_slicing))]
#![warn(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(clippy::expect_used)]
#![cfg_attr(test, allow(clippy::expect_used))]
#![warn(clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unreachable))]
#![warn(clippy::unimplemented)]
#![cfg_attr(test, allow(clippy::unimplemented))]

use anyhow::{Context as _, Error, format_err};
use diagnostics_log::PublishOptions;
use fidl_fuchsia_location_namedplace::RegulatoryRegionWatcherMarker;
use fidl_fuchsia_wlan_device_service::DeviceMonitorMarker;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fuchsia_async as fasync;
use fuchsia_async::DurationExt;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_trace_provider as ftrace_provider;
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::prelude::*;
use futures::{TryFutureExt, select};
use log::{error, info, warn};
use std::convert::Infallible;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use wlan_trace as wtrace;
use wlancfg_lib::access_point::AccessPoint;
use wlancfg_lib::client::connection_selection::{
    CONNECTION_SELECTION_REQUEST_BUFFER_SIZE, ConnectionSelectionRequester, ConnectionSelector,
    serve_connection_selection_request_loop,
};
use wlancfg_lib::client::roaming::lib::ROAMING_CHANNEL_BUFFER_SIZE;
use wlancfg_lib::client::roaming::local_roam_manager::{
    RoamManager, serve_local_roam_manager_requests,
};
use wlancfg_lib::client::{self, scan};
use wlancfg_lib::config_management::{SavedNetworksManager, SavedNetworksManagerApi};
use wlancfg_lib::legacy::{self, IfaceRef};
use wlancfg_lib::mode_management::iface_manager_api::IfaceManagerApi;
use wlancfg_lib::mode_management::phy_manager::PhyManager;
use wlancfg_lib::mode_management::{
    DEFECT_CHANNEL_SIZE, create_iface_manager, device_monitor, recovery,
};
use wlancfg_lib::regulatory_manager::RegulatoryManager;
use wlancfg_lib::telemetry::{
    TelemetrySender, connect_to_metrics_logger_factory, create_metrics_logger, serve_telemetry,
};
use wlancfg_lib::util;

const REGULATORY_LISTENER_TIMEOUT_SEC: i64 = 30;

async fn serve_fidl(
    ap: AccessPoint,
    configurator: legacy::deprecated_configuration::DeprecatedConfigurator,
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    legacy_client_ref: IfaceRef,
    saved_networks: Arc<dyn SavedNetworksManagerApi>,
    scan_requester: Arc<dyn scan::ScanRequestApi>,
    client_sender: util::listener::ClientListenerMessageSender,
    client_listener_msgs: mpsc::UnboundedReceiver<util::listener::ClientListenerMessage>,
    ap_listener_msgs: mpsc::UnboundedReceiver<util::listener::ApMessage>,
    regulatory_receiver: oneshot::Receiver<()>,
    telemetry_sender: TelemetrySender,
) -> Result<Infallible, Error> {
    // Wait a bit for the country code to be set before serving the policy APIs.
    let regulatory_listener_timeout = fasync::Timer::new(
        zx::MonotonicDuration::from_seconds(REGULATORY_LISTENER_TIMEOUT_SEC).after_now(),
    );
    select! {
        _ = regulatory_listener_timeout.fuse() => {
            // Log at info level because we expect RegulatoryManager may in some
            // cases be delayed.
            info!(
                "RegulatoryManager did not send a country code after {} seconds.",
                REGULATORY_LISTENER_TIMEOUT_SEC,
            );
        },
        result = regulatory_receiver.fuse() => {
            match result {
                Ok(()) => {
                    info!("Received country code from RegulatoryManager.");
                },
                // Log at warning level because RegulatoryManager should not
                // normally drop its end.
                Err(e) => warn!("Failed to receive country code from RegulatoryManager: {:?}", e),
            }
        }
    }
    info!("Proceeding to serve policy API.");

    let mut fs = ServiceFs::new_local();

    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );

    let client_sender1 = client_sender.clone();
    let client_sender2 = client_sender.clone();

    let second_ap = ap.clone();

    let saved_networks_clone = saved_networks.clone();

    let client_provider_lock = Arc::new(Mutex::new(()));

    // TODO(sakuma): Once the legacy API is deprecated, the interface manager should default to
    // stopped.
    {
        let mut iface_manager = iface_manager.lock().await;
        iface_manager.start_client_connections().await?;
    }

    let _ = fs
        .dir("svc")
        .add_fidl_service(move |reqs| {
            fasync::Task::local(client::serve_provider_requests(
                iface_manager.clone(),
                client_sender1.clone(),
                Arc::clone(&saved_networks_clone),
                scan_requester.clone(),
                client_provider_lock.clone(),
                reqs,
                telemetry_sender.clone(),
            ))
            .detach()
        })
        .add_fidl_service(move |reqs| {
            fasync::Task::local(client::serve_listener_requests(client_sender2.clone(), reqs))
                .detach()
        })
        .add_fidl_service(move |reqs| {
            fasync::Task::local(ap.clone().serve_provider_requests(reqs)).detach()
        })
        .add_fidl_service(move |reqs| {
            fasync::Task::local(second_ap.clone().serve_listener_requests(reqs)).detach()
        })
        .add_fidl_service(move |reqs| {
            fasync::Task::local(configurator.clone().serve_deprecated_configuration(reqs)).detach()
        })
        .add_fidl_service(|reqs| {
            let fut =
                legacy::deprecated_client::serve_deprecated_client(reqs, legacy_client_ref.clone())
                    .unwrap_or_else(|e| error!("error serving deprecated client API: {}", e));
            fasync::Task::local(fut).detach()
        });
    let service_fut = fs.take_and_serve_directory_handle()?.collect::<()>().fuse();
    let mut service_fut = pin!(service_fut);

    let serve_client_policy_listeners = util::listener::serve::<
        fidl_policy::ClientStateUpdatesProxy,
        fidl_policy::ClientStateSummary,
        util::listener::ClientStateUpdate,
    >(client_listener_msgs)
    .fuse();
    let mut serve_client_policy_listeners = pin!(serve_client_policy_listeners);

    let serve_ap_policy_listeners = util::listener::serve::<
        fidl_policy::AccessPointStateUpdatesProxy,
        Vec<fidl_policy::AccessPointState>,
        util::listener::ApStatesUpdate,
    >(ap_listener_msgs)
    .fuse();
    let mut serve_ap_policy_listeners = pin!(serve_ap_policy_listeners);

    loop {
        select! {
            _ = service_fut => (),
            _ = serve_client_policy_listeners => (),
            _ = serve_ap_policy_listeners => (),
        }
    }
}

/// Calls the metric recording function immediately and every 24 hours.
async fn saved_networks_manager_metrics_loop(saved_networks: Arc<dyn SavedNetworksManagerApi>) {
    loop {
        saved_networks.record_periodic_metrics().await;
        fasync::Timer::new(zx::MonotonicDuration::from_hours(24).after_now()).await;
    }
}

// wlancfg expects to be able to get updates from the RegulatoryRegionWatcher UNLESS the
// service is not present in wlancfg's sandbox OR the product configuration does not offer the
// service to wlancfg.  If the RegulatoryRegionWatcher is not available for either of these
// allowed reasons, wlancfg will continue serving the WLAN policy API in WW mode.
async fn run_regulatory_manager(
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    regulatory_sender: oneshot::Sender<()>,
) -> Result<(), Error> {
    // This initial connection will always succeed due to the presence of the protocol in the
    // component manifest.
    let req = match fuchsia_component::client::new_protocol_connector::<RegulatoryRegionWatcherMarker>(
    ) {
        Ok(req) => req,
        Err(e) => {
            warn!("error probing RegulatoryRegionWatcher service: {:?}", e);
            return Ok(());
        }
    };

    // An error here indicates that there is a missing `use` statement and the
    // RegulatoryRegionWatcher is not available in this namespace.  This should never happen since
    // the build system checks the capability routes and wlancfg needs this service.
    if !req.exists().await.context("error checking for RegulatoryRegionWatcher existence")? {
        warn!("RegulatoryRegionWatcher is not available per the component manifest");
        return Ok(());
    }

    // The connect call will always succeed because all it does is send the server end of a channel
    // to the directory that holds the capability.
    let regulatory_svc =
        req.connect().context("unable to connect RegulatoryRegionWatcher proxy")?;

    let regulatory_manager = RegulatoryManager::new(regulatory_svc, iface_manager);

    // The only way to test for the presence of the RegulatoryRegionWatcher service is to actually
    // use the handle to poll for updates.  If the RegulatoryManager future exits, simply log the
    // reason and return Ok(()) so that the WLAN policy API will continue to be served.
    if let Some(e) = regulatory_manager.run(regulatory_sender).await.err() {
        warn!("RegulatoryManager exiting: {:?}", e);
    }
    Ok(())
}

async fn run_all_futures() -> Result<(), Error> {
    // Get wlancfg product config data.
    let cfg = wlancfg_config::Config::take_from_startup_handle();
    let monitor_svc = fuchsia_component::client::connect_to_protocol::<DeviceMonitorMarker>()
        .context("failed to connect to device monitor")?;

    let cobalt_svc = connect_to_metrics_logger_factory().await?;
    let cobalt_proxy = match create_metrics_logger(&cobalt_svc).await {
        Ok(proxy) => proxy,
        Err(e) => {
            warn!("Metrics logging is unavailable: {}", e);

            // If it is not possible to acquire a metrics logging proxy, create a disconnected
            // proxy and attempt to serve the policy API with metrics disabled.
            let (proxy, _) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();
            proxy
        }
    };

    let external_inspect_node = component::inspector().root().create_child("external");
    let (defect_sender, defect_receiver) = mpsc::channel(DEFECT_CHANNEL_SIZE);
    let (telemetry_sender, telemetry_fut) = serve_telemetry(
        monitor_svc.clone(),
        cobalt_proxy,
        component::inspector().root().create_child("client_stats"),
        external_inspect_node.create_child("client_stats"),
        defect_sender.clone(),
    );
    component::inspector().root().record(external_inspect_node);

    let (scan_request_sender, scan_request_receiver) =
        mpsc::channel(scan::SCAN_REQUEST_BUFFER_SIZE);
    let scan_requester = Arc::new(scan::ScanRequester { sender: scan_request_sender });
    let saved_networks = Arc::new(SavedNetworksManager::new(telemetry_sender.clone()).await);

    let connection_selector = Rc::new(ConnectionSelector::new(
        saved_networks.clone(),
        scan_requester.clone(),
        component::inspector().root().create_child("connection_selector"),
        telemetry_sender.clone(),
    ));
    let (connection_selection_request_sender, connection_selection_request_receiver) =
        mpsc::channel(CONNECTION_SELECTION_REQUEST_BUFFER_SIZE);
    let connection_selection_service = serve_connection_selection_request_loop(
        connection_selector,
        connection_selection_request_receiver,
    );
    let connection_selection_requester =
        ConnectionSelectionRequester::new(connection_selection_request_sender);

    info!("Roaming policy: {}", cfg.roaming_policy);
    let (roam_service_request_sender, roam_service_request_receiver) =
        mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
    let roam_manager_service_fut = serve_local_roam_manager_requests(
        cfg.roaming_policy.into(),
        roam_service_request_receiver,
        connection_selection_requester.clone(),
        telemetry_sender.clone(),
        saved_networks.clone(),
    );
    let roam_manager = RoamManager::new(roam_service_request_sender);

    let (recovery_sender, recovery_receiver) =
        mpsc::channel::<recovery::RecoverySummary>(recovery::RECOVERY_SUMMARY_CHANNEL_CAPACITY);
    // Get the recovery settings.
    info!("Recovery Profile: {}", cfg.recovery_profile);
    info!("Recovery Enabled: {}", cfg.recovery_enabled);
    info!("Power Framework Enabled: {}", cfg.power_framework_enabled);
    let phy_manager = Arc::new(Mutex::new(PhyManager::new(
        monitor_svc.clone(),
        recovery::lookup_recovery_profile(&cfg.recovery_profile),
        cfg.recovery_enabled,
        component::inspector().root().create_child("phy_manager"),
        telemetry_sender.clone(),
        recovery_sender,
    )));
    let configurator =
        legacy::deprecated_configuration::DeprecatedConfigurator::new(phy_manager.clone());

    let (watcher_proxy, watcher_server_end) = fidl::endpoints::create_proxy();
    monitor_svc.watch_devices(watcher_server_end)?;

    let (client_sender, client_receiver) = mpsc::unbounded();
    let (ap_sender, ap_receiver) = mpsc::unbounded();
    let (iface_manager, iface_manager_service) = create_iface_manager(
        phy_manager.clone(),
        client_sender.clone(),
        ap_sender.clone(),
        monitor_svc.clone(),
        saved_networks.clone(),
        connection_selection_requester,
        roam_manager,
        telemetry_sender.clone(),
        defect_sender,
        defect_receiver,
        recovery_receiver,
        component::inspector().root().create_child("iface_manager"),
    );

    let scanning_service = scan::serve_scanning_loop(
        iface_manager.clone(),
        saved_networks.clone(),
        telemetry_sender.clone(),
        scan::LocationSensorUpdater {},
        scan_request_receiver,
    );

    let legacy_client = IfaceRef::new();
    let device_event_listener = device_monitor::Listener::new(
        monitor_svc.clone(),
        legacy_client.clone(),
        phy_manager.clone(),
        iface_manager.clone(),
    );

    let (regulatory_sender, regulatory_receiver) = oneshot::channel();
    let ap = AccessPoint::new(iface_manager.clone(), ap_sender, Arc::new(Mutex::new(())));
    let fidl_fut = serve_fidl(
        ap,
        configurator,
        iface_manager.clone(),
        legacy_client,
        saved_networks.clone(),
        scan_requester,
        client_sender,
        client_receiver,
        ap_receiver,
        regulatory_receiver,
        telemetry_sender.clone(),
    );

    let dev_watcher_fut = watcher_proxy
        .take_event_stream()
        .try_for_each(|evt| device_monitor::handle_event(&device_event_listener, evt).map(Ok))
        .err_into()
        .and_then(|_| {
            let result: Result<(), Error> =
                Err(format_err!("Device watcher future exited unexpectedly"));
            future::ready(result)
        });

    let saved_networks_metrics_fut = saved_networks_manager_metrics_loop(saved_networks.clone());
    let regulatory_fut = run_regulatory_manager(iface_manager.clone(), regulatory_sender);

    let _ = futures::try_join!(
        fidl_fut,
        dev_watcher_fut,
        iface_manager_service,
        saved_networks_metrics_fut.map(Ok),
        scanning_service,
        regulatory_fut,
        telemetry_fut.map(Ok),
        roam_manager_service_fut.map(Ok),
        connection_selection_service.map(Ok)
    )?;
    Ok(())
}

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    let options =
        PublishOptions::default().tags(&["wlan"]).enable_metatag(diagnostics_log::Metatag::Target);
    // Crash instead of proceeded without logging
    #[expect(clippy::expect_used)]
    diagnostics_log::initialize(options).expect("Failed to initialize diagnostics log");
    ftrace_provider::trace_provider_create_with_fdio();
    wtrace::instant_wlancfg_start();

    // Always exit with an error so stopping wlancfg will trigger a reboot on devices
    // where wlancfg is `on_terminate: "reboot"`.
    Box::pin(run_all_futures()).await.and(Err(format_err!("WLAN policy layer stopped"))).inspect_err(
        |e| {
            // When the ELF runner executing wlancfg is configured with `forward_std_err_to: "none"`,
            // the Rust Termination trait's implementation to print an `Err` value to stderr would be lost.
            // For this reason, always send the error directly to the syslog. The additional comment in
            // the log is a reminder in the syslog that the duplicate log is intentional.
            error!("{e:?}. (ELF runner logs this again as a warning unless configured not to forward stderr.)");
        },
    )
}
