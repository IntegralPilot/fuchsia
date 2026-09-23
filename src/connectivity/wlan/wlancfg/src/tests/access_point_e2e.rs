// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::access_point::AccessPoint;
use crate::client::connection_selection;
use crate::client::roaming::lib::ROAMING_CHANNEL_BUFFER_SIZE;
use crate::client::roaming::local_roam_manager::RoamManager;
use crate::config_management::SavedNetworksManager;
use crate::legacy;
use crate::mode_management::iface_manager_api::IfaceManagerApi;
use crate::mode_management::phy_manager::{PhyManager, PhyManagerApi};
use crate::mode_management::{DEFECT_CHANNEL_SIZE, create_iface_manager, device_monitor, recovery};
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use crate::util::listener;
use crate::util::testing::{run_until_completion, run_while};
use anyhow::{Error, format_err};
use assert_matches::assert_matches;
use fidl::endpoints::{create_proxy, create_request_stream};
use fidl_fuchsia_wlan_device_service::DeviceWatcherEvent;
use fuchsia_async::{self as fasync, TestExecutor};
use fuchsia_inspect::{self as inspect};
use futures::channel::mpsc;
use futures::future::{JoinAll, join_all};
use futures::lock::Mutex;
use futures::prelude::*;
use futures::stream::StreamExt;
use futures::task::Poll;
use std::convert::Infallible;
use std::pin::{Pin, pin};
use std::sync::Arc;
use wlan_common::test_utils::ExpectWithin;
use {
    fidl_fuchsia_wlan_common as fidl_common, fidl_fuchsia_wlan_policy as fidl_policy,
    fidl_fuchsia_wlan_sme as fidl_sme,
};

pub const TEST_AP_IFACE_ID: u16 = 43;
pub const TEST_PHY_ID: u16 = 41;

struct TestValues {
    internal_objects: InternalObjects,
    external_interfaces: ExternalInterfaces,
}

// Internal policy objects, used for manipulating state within tests
#[allow(clippy::type_complexity)]
struct InternalObjects {
    internal_futures: JoinAll<Pin<Box<dyn Future<Output = Result<Infallible, Error>>>>>,
    phy_manager: Arc<Mutex<dyn PhyManagerApi + Send>>,
    iface_manager: Arc<Mutex<dyn IfaceManagerApi + Send>>,
}

struct ExternalInterfaces {
    monitor_service_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
    monitor_service_stream: fidl_fuchsia_wlan_device_service::DeviceMonitorRequestStream,
    ap_controller: fidl_policy::AccessPointControllerProxy,
    _listener_updates_stream: fidl_policy::AccessPointStateUpdatesRequestStream,
    _telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
}

fn test_setup(
    exec: &mut TestExecutor,
    recovery_profile: &str,
    recovery_enabled: bool,
) -> TestValues {
    // Mock out the DeviceMonitor service.
    let (monitor_service_proxy, monitor_service_requests) =
        create_proxy::<fidl_fuchsia_wlan_device_service::DeviceMonitorMarker>();
    let monitor_service_stream = monitor_service_requests.into_stream();

    // Create the telemetry communication channel.
    let (telemetry_sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
    let telemetry_sender = TelemetrySender::new(telemetry_sender);

    // Create the recovery communication channel.
    let (recovery_sender, recovery_receiver) =
        mpsc::channel::<recovery::RecoverySummary>(recovery::RECOVERY_SUMMARY_CHANNEL_CAPACITY);

    // Create the proxy that will be used to make requests of the AP policy API.
    let (ap_provider_proxy, ap_provider_requests) =
        create_proxy::<fidl_policy::AccessPointProviderMarker>();
    let ap_provider_requests = ap_provider_requests.into_stream();

    // Create the policy update listeners.
    let (client_update_sender, _client_update_receiver) = mpsc::unbounded();
    let (ap_update_sender, ap_update_receiver) = mpsc::unbounded();

    // Create a fake saved networks store for the construction of the IfaceManager.
    let mut saved_networks_mgt_fut = pin!(SavedNetworksManager::new_for_test());
    let saved_networks = run_until_completion(exec, &mut saved_networks_mgt_fut);
    let saved_networks = Arc::new(saved_networks);

    // Construct the ConnectionSelectionRequester.  This is needed for the construction of the
    // IfaceManager but will not be used in tests.
    let (connection_selection_request_sender, _connection_selection_request_receiver) =
        mpsc::channel(5);
    let connection_selection_requester = connection_selection::ConnectionSelectionRequester::new(
        connection_selection_request_sender,
    );

    // Construct the RoamManager.
    let (roam_service_request_sender, _roam_service_request_receiver) =
        mpsc::channel(ROAMING_CHANNEL_BUFFER_SIZE);
    let roam_manager = RoamManager::new(roam_service_request_sender);

    // Construct the PhyManager and IfaceManager.
    let phy_manager = Arc::new(Mutex::new(PhyManager::new(
        monitor_service_proxy.clone(),
        recovery::lookup_recovery_profile(recovery_profile),
        recovery_enabled,
        inspect::Inspector::default().root().create_child("phy_manager"),
        telemetry_sender.clone(),
        recovery_sender,
    )));
    let (defect_sender, defect_receiver) = mpsc::channel(DEFECT_CHANNEL_SIZE);
    let (iface_manager, iface_manager_service) = create_iface_manager(
        phy_manager.clone(),
        client_update_sender.clone(),
        ap_update_sender.clone(),
        monitor_service_proxy.clone(),
        saved_networks.clone(),
        connection_selection_requester.clone(),
        roam_manager.clone(),
        telemetry_sender.clone(),
        defect_sender,
        defect_receiver,
        recovery_receiver,
        inspect::Inspector::default().root().create_child("iface_manager"),
    );
    let iface_manager_service = Box::pin(iface_manager_service);

    // Create the AccessPoint struct that will serve Access Point policy API.
    let ap_provider_lock = Arc::new(Mutex::new(()));
    let ap = AccessPoint::new(iface_manager.clone(), ap_update_sender, ap_provider_lock);

    let serve_fut: Pin<Box<dyn Future<Output = Result<Infallible, Error>>>> = Box::pin(
        ap.serve_provider_requests(ap_provider_requests)
            // Map the output type of this future to match the other ones we want to combine with it
            .map(|_| {
                let result: Result<Infallible, Error> =
                    Err(format_err!("serve_provider_requests future exited unexpectedly"));
                result
            }),
    );

    // Create a future to serve the AP listener updates API.
    let serve_ap_policy_listeners = Box::pin(
        listener::serve::<
            fidl_policy::AccessPointStateUpdatesProxy,
            Vec<fidl_policy::AccessPointState>,
            listener::ApStatesUpdate,
        >(ap_update_receiver)
        // Map the output type of this future to match the other ones we want to combine with it
        .map(|_| {
            let result: Result<Infallible, Error> =
                Err(format_err!("serve_ap_policy_listeners future exited unexpectedly"));
            result
        })
        .fuse(),
    );

    // Get the AP policy controller
    let (ap_controller, _listener_updates_stream) = request_controller(&ap_provider_proxy);

    // Return all of the necessary test structs.
    let internal_futures =
        join_all(vec![serve_fut, iface_manager_service, serve_ap_policy_listeners]);

    let internal_objects = InternalObjects { internal_futures, phy_manager, iface_manager };

    let external_interfaces = ExternalInterfaces {
        monitor_service_proxy,
        monitor_service_stream,
        ap_controller,
        _listener_updates_stream,
        _telemetry_receiver: telemetry_receiver,
    };

    TestValues { internal_objects, external_interfaces }
}

fn request_controller(
    provider: &fidl_policy::AccessPointProviderProxy,
) -> (fidl_policy::AccessPointControllerProxy, fidl_policy::AccessPointStateUpdatesRequestStream) {
    let (controller, requests) = create_proxy::<fidl_policy::AccessPointControllerMarker>();
    let (update_sink, update_stream) =
        create_request_stream::<fidl_policy::AccessPointStateUpdatesMarker>();
    provider.get_controller(requests, update_sink).expect("error getting controller");
    (controller, update_stream)
}

fn add_phy(exec: &mut TestExecutor, test_values: &mut TestValues) {
    // Use the "legacy" module to mimic the wlancfg main module. When the main module
    // is refactored to remove the "legacy" module, we can also refactor this section.
    let legacy_client = legacy::IfaceRef::new();
    let listener = device_monitor::Listener::new(
        test_values.external_interfaces.monitor_service_proxy.clone(),
        legacy_client.clone(),
        test_values.internal_objects.phy_manager.clone(),
        test_values.internal_objects.iface_manager.clone(),
    );
    let add_phy_event = DeviceWatcherEvent::OnPhyAdded { phy_id: TEST_PHY_ID };
    let add_phy_fut = device_monitor::handle_event(&listener, add_phy_event);
    let mut add_phy_fut = pin!(add_phy_fut);

    let device_monitor_req = run_while(
        exec,
        &mut add_phy_fut,
        test_values.external_interfaces.monitor_service_stream.next(),
    );
    assert_matches!(
        device_monitor_req,
        Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetSupportedMacRoles {
            phy_id: TEST_PHY_ID, responder
        })) => {
            // Send back a positive acknowledgement.
            assert!(responder.send(Ok(&[fidl_common::WlanMacRole::Ap])).is_ok());
        }
    );

    run_until_completion(
        exec,
        pin!(
            add_phy_fut
                .expect_within(zx::MonotonicDuration::from_seconds(5), "future didn't complete")
        ),
    );
}

#[fuchsia::test]
fn test_ap_start_failure_recovery() {
    let mut exec = fasync::TestExecutor::new();
    let mut test_values = test_setup(&mut exec, "thresholded_recovery", true);

    // No request has been sent yet. Future should be idle.
    assert_matches!(
        exec.run_until_stalled(&mut test_values.internal_objects.internal_futures),
        Poll::Pending
    );

    // Add a fake PHY.
    add_phy(&mut exec, &mut test_values);

    // Issue a StartAccessPoint request.  Once the AP has failed to start an adequate number of
    // times, expect a PHY reset.
    //
    // The AP will attempt to start several times before giving up.  Stopping the AP should
    // succeed, but each start request should be rejected.  The destruction of the interface is an
    // indication that the state machine has given up and another start request should be issued.
    let ap_start_failure_threshold: usize = recovery::AP_START_FAILURE_RECOVERY_THRESHOLD;
    let mut ap_start_failures: usize = 0;

    let network_config = fidl_policy::NetworkConfig {
        id: Some(fidl_policy::NetworkIdentifier {
            ssid: b"test".to_vec(),
            type_: fidl_policy::SecurityType::None,
        }),
        credential: None,
        ..Default::default()
    };

    loop {
        if ap_start_failures == ap_start_failure_threshold {
            break;
        }

        // Issue the request to start the AP
        let start_ap_fut = test_values.external_interfaces.ap_controller.start_access_point(
            &network_config,
            fidl_policy::ConnectivityMode::Unrestricted,
            fidl_policy::OperatingBand::Any,
        );
        let mut start_ap_fut = pin!(start_ap_fut);
        assert_matches!(exec.run_until_stalled(&mut start_ap_fut), Poll::Pending);

        // Run the wlancfg internals so that they automatically create and destroy interfaces,
        // request AP SME proxies, attempt to bring up the soft AP.  Creating SME proxies should
        // succeed as should attempts to stop the soft AP.  Starting the AP should fail in all
        // cases.  Once the state machine runs out of retries, it exits and wlancfg requests that
        // the interface be destroyed.  This is an indication to break out and issue another policy
        // API start AP request.
        let mut sme_stream = None;
        loop {
            // Run all of the internal futures to process the start AP request.
            while exec.wake_next_timer().is_some() {}
            assert_matches!(
                exec.run_until_stalled(&mut test_values.internal_objects.internal_futures),
                Poll::Pending
            );

            // Check for Interface creation and destruction requests as well as AP SME requests.
            if let Poll::Ready(req) = exec.run_until_stalled(
                &mut test_values.external_interfaces.monitor_service_stream.next(),
            ) {
                match req {
                    Some(Ok(
                        fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                            sme_server,
                            responder,
                            iface_id: TEST_AP_IFACE_ID,
                        },
                    )) => {
                        assert!(responder.send(Ok(())).is_ok());
                        sme_stream = Some(sme_server.into_stream());
                    }
                    Some(Ok(
                        fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::DestroyIface {
                            req:
                                fidl_fuchsia_wlan_device_service::DestroyIfaceRequest {
                                    iface_id: TEST_AP_IFACE_ID,
                                },
                            responder,
                        },
                    )) => {
                        assert!(responder.send(zx::sys::ZX_OK).is_ok());

                        // Since IfaceManager has requested that the AP SME be destroyed, the AP
                        // start process needs to be re-instigated by making another policy StartAP
                        // request.
                        break;
                    }
                    Some(Ok(
                        fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::CreateIface {
                            responder,
                            payload,
                        },
                    )) => {
                        assert_eq!(payload.phy_id, Some(TEST_PHY_ID));
                        assert_eq!(payload.role, Some(fidl_common::WlanMacRole::Ap));
                        assert_eq!(payload.sta_address, Some([0, 0, 0, 0, 0, 0]));
                        assert!(responder
                            .send(
                                Ok(&fidl_fuchsia_wlan_device_service::DeviceMonitorCreateIfaceResponse {
                                    iface_id: Some(TEST_AP_IFACE_ID),
                                    ..Default::default()
                                })
                            )
                            .is_ok());
                    }
                    other => panic!("Unexpected DeviceMonitor operation: {other:?}"),
                }
                continue;
            }

            // Check for AP SME commands.
            if let Some(mut sme_req_stream) = sme_stream.take() {
                if let Poll::Ready(req) = exec.run_until_stalled(&mut sme_req_stream.next()) {
                    match req {
                        Some(Ok(fidl_sme::ApSmeRequest::Start { responder, .. })) => {
                            responder
                                .send(fidl_sme::StartApResultCode::InternalError)
                                .expect("could not send AP start response");

                            ap_start_failures += 1;
                        }
                        Some(Ok(fidl_sme::ApSmeRequest::Stop { responder, .. })) => {
                            responder
                                .send(fidl_sme::StopApResultCode::Success)
                                .expect("could not send AP stop response");
                        }
                        other => panic!("Unexpected SME operation: {other:?}"),
                    }
                    sme_stream = Some(sme_req_stream);
                    continue;
                }

                sme_stream = Some(sme_req_stream);
            }
        }
    }

    // At this point, enough stop AP failures have been encountered that a PHY reset should be
    // requested.  There is a bit of a race here since the order in which the futures are processed
    // by the IfaceManager cannot be guaranteed.  The AP state machine should be in the process of
    // terminating which would cause a DestroyIface request.  The defect reporting machinery at the
    // same time should be requesting a PHY reset.  Drain the DeviceMonitor requests until the PHY
    // reset is observed
    loop {
        let dev_monitor_req = run_while(
            &mut exec,
            &mut test_values.internal_objects.internal_futures,
            test_values.external_interfaces.monitor_service_stream.next(),
        );

        if let Some(Ok(req)) = dev_monitor_req {
            match req {
                fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::Reset {
                    phy_id: TEST_PHY_ID,
                    ..
                } => break,
                fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::DestroyIface {
                    req:
                        fidl_fuchsia_wlan_device_service::DestroyIfaceRequest {
                            iface_id: TEST_AP_IFACE_ID,
                        },
                    responder,
                } => assert!(responder.send(zx::sys::ZX_OK).is_ok()),
                other => panic!("Unexpected DeviceMonitor request: {other:?}"),
            }
        }
    }
}
