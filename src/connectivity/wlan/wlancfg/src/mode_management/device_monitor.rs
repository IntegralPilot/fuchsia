// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::legacy::{Iface, IfaceRef};
use crate::mode_management::iface_manager_api::IfaceManagerApi;
use crate::mode_management::phy_manager::PhyManagerApi;
use anyhow::format_err;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_device_service::{DeviceMonitorProxy, DeviceWatcherEvent};
use futures::lock::Mutex;
use log::{error, info};
use std::sync::Arc;

pub struct Listener {
    proxy: DeviceMonitorProxy,
    legacy_shim: IfaceRef,
    phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
    iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
}

pub async fn handle_event(listener: &Listener, evt: DeviceWatcherEvent) {
    info!("got event: {:?}", evt);
    match evt {
        DeviceWatcherEvent::OnPhyAdded { phy_id } => {
            on_phy_added(listener, phy_id).await;
        }
        DeviceWatcherEvent::OnPhyRemoved { phy_id } => {
            info!("phy removed: {}", phy_id);
            let mut phy_manager = listener.phy_manager.lock().await;
            phy_manager.remove_phy(phy_id);
        }
        DeviceWatcherEvent::OnIfaceAdded { iface_id } => {
            // Ensure the new interface is usable by the policy internals before providing it to the
            // legacy shim.
            let mut iface_manager = listener.iface_manager.lock().await;
            if let Err(e) = iface_manager.handle_added_iface(iface_id).await {
                error!("Failed to add interface {} to IfaceManager: {}", iface_id, e);
                return;
            }

            if let Err(e) = on_iface_added_legacy(listener, iface_id).await {
                error!("error adding new iface {}: {}", iface_id, e)
            }
        }
        DeviceWatcherEvent::OnIfaceRemoved { iface_id } => {
            let mut iface_manager = listener.iface_manager.lock().await;
            match iface_manager.handle_removed_iface(iface_id).await {
                Ok(()) => {}
                Err(e) => info!("Unable to record idle interface {}: {:?}", iface_id, e),
            }

            listener.legacy_shim.remove_if_matching(iface_id);
            info!("iface removed: {}", iface_id);
        }
    }
}

async fn on_phy_added(listener: &Listener, phy_id: u16) {
    info!("phy {} added", phy_id);
    let mut phy_manager = listener.phy_manager.lock().await;
    if let Err(e) = phy_manager.add_phy(phy_id).await {
        info!("error adding new phy {}: {}", phy_id, e);
        phy_manager.log_phy_add_failure();
    }
}

/// Configured the interface that is used to service the legacy WLAN API.
async fn on_iface_added_legacy(listener: &Listener, iface_id: u16) -> Result<(), anyhow::Error> {
    let response = match listener.proxy.query_iface(iface_id).await? {
        Ok(response) => response,
        Err(zx::sys::ZX_ERR_NOT_FOUND) => {
            return Err(format_err!("Could not find iface: {}", iface_id));
        }
        Err(status) => return Err(format_err!("Could not query iface information: {}", status)),
    };

    let service = listener.proxy.clone();

    match response.role {
        fidl_common::WlanMacRole::Client => {
            let legacy_shim = listener.legacy_shim.clone();
            let (sme, remote) = create_proxy();

            let result = service
                .get_client_sme(iface_id, remote)
                .await
                .map_err(|e| format_err!("Failed to get client SME: {}", e))?;
            result.map_err(|e| {
                format_err!("GetClientSme returned an error: {}", zx::Status::err_from_raw(e))
            })?;

            let lc = Iface { sme: sme.clone(), iface_id };
            legacy_shim.set_if_empty(lc);
        }
        // The AP service make direct use of the PhyManager to get interfaces.
        fidl_common::WlanMacRole::Ap => {}
        fidl_common::WlanMacRole::Mesh => {
            return Err(format_err!("Unexpectedly observed a mesh iface: {}", iface_id));
        }
        fidl_common::WlanMacRoleUnknown!() => {
            return Err(format_err!(
                "Unknown WlanMacRole type {:?} on iface {}",
                response.role,
                iface_id
            ));
        }
    }

    info!("new iface {} added successfully", iface_id);
    Ok(())
}

impl Listener {
    pub fn new(
        proxy: DeviceMonitorProxy,
        legacy_shim: IfaceRef,
        phy_manager: Arc<Mutex<dyn PhyManagerApi>>,
        iface_manager: Arc<Mutex<dyn IfaceManagerApi>>,
    ) -> Self {
        Listener { proxy, legacy_shim, phy_manager, iface_manager }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_point::{state_machine as ap_fsm, types as ap_types};
    use crate::client::types as client_types;
    use crate::mode_management::Defect;
    use crate::mode_management::iface_manager_api::{ConnectAttemptRequest, SmeForScan};
    use crate::mode_management::phy_manager::{CreateClientIfacesReason, PhyManagerError};
    use crate::mode_management::recovery::RecoverySummary;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl_fuchsia_wlan_device_service as fidl_service;
    use fidl_fuchsia_wlan_sme as fidl_sme;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use futures::channel::oneshot;
    use futures::task::Poll;
    use ieee80211::MacAddr;
    use std::collections::HashMap;
    use std::pin::pin;

    struct TestValues {
        phy_manager: Arc<Mutex<FakePhyManager>>,
        iface_manager: Arc<Mutex<FakeIfaceManager>>,
        monitor_proxy: fidl_service::DeviceMonitorProxy,
        monitor_stream: fidl_service::DeviceMonitorRequestStream,
    }

    fn test_setup(add_phy_succeeds: bool, add_iface_succeeds: bool) -> TestValues {
        let phy_manager =
            Arc::new(Mutex::new(FakePhyManager::new(add_phy_succeeds, add_iface_succeeds)));
        let iface_manager = Arc::new(Mutex::new(FakeIfaceManager::new()));
        let (monitor_proxy, monitor_requests) = create_proxy::<fidl_service::DeviceMonitorMarker>();
        let monitor_stream = monitor_requests.into_stream();

        TestValues { phy_manager, iface_manager, monitor_proxy, monitor_stream }
    }

    #[fuchsia::test]
    fn test_phy_add_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(true, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Add Phy 0.
        let fut = on_phy_added(&listener, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that Phy 0 is now present
        let list_phys_fut = async move {
            let phy_manager = test_values.phy_manager.lock().await;
            phy_manager.phys.clone()
        };
        let mut list_phys_fut = pin!(list_phys_fut);
        let phys =
            assert_matches!(exec.run_until_stalled(&mut list_phys_fut), Poll::Ready(phys) => phys);

        assert_eq!(phys, vec![0]);
    }

    #[fuchsia::test]
    fn test_phy_add_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Add Phy 0.
        let fut = on_phy_added(&listener, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that Phy 0 was not added and its failure was logged.
        let phy_manager_fut = async move {
            let phy_manager = test_values.phy_manager.lock().await;
            assert!(phy_manager.phys.is_empty());
            assert_eq!(phy_manager.failed_phys, 1);
        };
        let mut phy_manager_fut = pin!(phy_manager_fut);
        assert_matches!(exec.run_until_stalled(&mut phy_manager_fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn test_add_legacy_ap_iface() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        let fut = on_iface_added_legacy(&listener, 0);
        let mut fut = pin!(fut);

        // Run the future until it queries the interface's properties.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Reply to the query indicating that this is an AP.
        let iface_response = Some(fidl_service::QueryIfaceResponse {
            role: fidl_common::WlanMacRole::Ap,
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr: [0, 1, 2, 3, 4, 5],
            factory_addr: [0, 1, 2, 3, 4, 5],
        });
        send_query_iface_response(&mut exec, &mut test_values.monitor_stream, iface_response);

        // Nothing special should happen for the AP interface and the future should complete.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_add_legacy_unknown_iface() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        let fut = on_iface_added_legacy(&listener, 0);
        let mut fut = pin!(fut);

        // Run the future until it queries the interface's properties.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Reply to the query indicating that this is an AP.
        let iface_response = Some(fidl_service::QueryIfaceResponse {
            role: fidl_common::WlanMacRole::unknown(),
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr: [0, 1, 2, 3, 4, 5],
            factory_addr: [0, 1, 2, 3, 4, 5],
        });
        send_query_iface_response(&mut exec, &mut test_values.monitor_stream, iface_response);

        // The future should return an error in this case since an unknown WlanMacRole is not
        // supported.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_add_legacy_mesh_iface() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        let fut = on_iface_added_legacy(&listener, 0);
        let mut fut = pin!(fut);

        // Run the future until it queries the interface's properties.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Reply to the query indicating that this is a mesh interface.
        let iface_response = Some(fidl_service::QueryIfaceResponse {
            role: fidl_common::WlanMacRole::Mesh,
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr: [0, 1, 2, 3, 4, 5],
            factory_addr: [0, 1, 2, 3, 4, 5],
        });
        send_query_iface_response(&mut exec, &mut test_values.monitor_stream, iface_response);

        // The future should return an error in this case since mesh is not supported.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_add_legacy_client_iface_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        let fut = on_iface_added_legacy(&listener, 0);
        let mut fut = pin!(fut);

        // Run the future until it queries the interface's properties.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Reply to the query indicating that this is a client.
        let iface_response = Some(fidl_service::QueryIfaceResponse {
            role: fidl_common::WlanMacRole::Client,
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr: [0, 1, 2, 3, 4, 5],
            factory_addr: [0, 1, 2, 3, 4, 5],
        });
        send_query_iface_response(&mut exec, &mut test_values.monitor_stream, iface_response);

        // The future should stall again while requesting a client SME proxy.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_service::DeviceMonitorRequest::GetClientSme {
                iface_id: 0, sme_server: _, responder
            }))) => {
                assert!(responder.send(Ok(())).is_ok())
            }
        );

        // The future should now run to completion.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // The listener should have a client interface.
        assert!(listener.legacy_shim.get().is_ok());
    }

    #[fuchsia::test]
    fn test_add_legacy_client_iface_fails() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        let fut = on_iface_added_legacy(&listener, 0);
        let mut fut = pin!(fut);

        // Run the future until it queries the interface's properties.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Reply to the query indicating that this is a client.
        let iface_response = Some(fidl_service::QueryIfaceResponse {
            role: fidl_common::WlanMacRole::Client,
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr: [0, 1, 2, 3, 4, 5],
            factory_addr: [0, 1, 2, 3, 4, 5],
        });
        send_query_iface_response(&mut exec, &mut test_values.monitor_stream, iface_response);

        // The future should stall again while requesting a client SME proxy.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_service::DeviceMonitorRequest::GetClientSme {
                iface_id: 0, sme_server: _, responder
            }))) => {
                assert!(responder.send(Err(zx::sys::ZX_ERR_NOT_FOUND)).is_ok())
            }
        );

        // The future should now run to completion.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // The listener should not have a client interface.
        assert!(listener.legacy_shim.get().is_err());
    }

    #[fuchsia::test]
    fn test_add_legacy_client_iface_query_fails() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(false, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Drop the monitor stream so the QueryIface request fails.
        drop(test_values.monitor_stream);

        let fut = on_iface_added_legacy(&listener, 0);
        let mut fut = pin!(fut);

        // Run the future should immediately return an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));

        // The listener should not have a client interface.
        assert!(listener.legacy_shim.get().is_err());
    }

    #[fuchsia::test]
    fn test_handle_add_phy_event() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(true, false);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Simulate an OnPhyAdded event
        let fut = handle_event(&listener, DeviceWatcherEvent::OnPhyAdded { phy_id: 0 });
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that Phy 0 is now present
        let list_phys_fut = async move {
            let phy_manager = test_values.phy_manager.lock().await;
            phy_manager.phys.clone()
        };
        let mut list_phys_fut = pin!(list_phys_fut);
        let phys =
            assert_matches!(exec.run_until_stalled(&mut list_phys_fut), Poll::Ready(phys) => phys);

        assert_eq!(phys, vec![0]);
    }

    #[fuchsia::test]
    fn test_handle_remove_phy_event() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(false, false);

        // Preload a fake PHY ID into the PhyManager
        {
            let phy_manager = test_values.phy_manager.clone();
            let add_phy_fut = async move {
                let mut phy_manager = phy_manager.lock().await;
                phy_manager.phys.push(0);
            };
            let mut add_phy_fut = pin!(add_phy_fut);
            assert_matches!(exec.run_until_stalled(&mut add_phy_fut), Poll::Ready(()));
        }

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Simulate an OnPhyRemoved event.
        let fut = handle_event(&listener, DeviceWatcherEvent::OnPhyRemoved { phy_id: 0 });
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that the PHY ID is no longer present
        let list_phys_fut = async move {
            let phy_manager = test_values.phy_manager.lock().await;
            phy_manager.phys.clone()
        };
        let mut list_phys_fut = pin!(list_phys_fut);
        let phys =
            assert_matches!(exec.run_until_stalled(&mut list_phys_fut), Poll::Ready(phys) => phys);

        assert!(phys.is_empty());
    }

    #[fuchsia::test]
    fn test_handle_remove_nonexistent_iface_event() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(false, false);

        // Load a fake iface ID into the IfaceManager.
        {
            let iface_manager = test_values.iface_manager.clone();
            let add_iface_fut = async move {
                let mut iface_manager = iface_manager.lock().await;
                iface_manager.ifaces.push(0);
            };
            let mut add_iface_fut = pin!(add_iface_fut);
            assert_matches!(exec.run_until_stalled(&mut add_iface_fut), Poll::Ready(()));
        }

        // Setup the Listener to look like it has an interface.
        let (sme, _) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let iface_ref = IfaceRef::new();
        iface_ref.set_if_empty(Iface { sme, iface_id: 0 });

        let listener = Listener::new(
            test_values.monitor_proxy,
            iface_ref,
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Run the iface removal handler.
        let fut = handle_event(&listener, DeviceWatcherEvent::OnIfaceRemoved { iface_id: 123 });
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // The IfaceRef should still have its interface.
        assert!(listener.legacy_shim.get().is_ok());
    }

    #[fuchsia::test]
    fn test_handle_remove_iface_event() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(false, false);

        // Load a fake iface ID into the IfaceManager.
        {
            let iface_manager = test_values.iface_manager.clone();
            let add_iface_fut = async move {
                let mut iface_manager = iface_manager.lock().await;
                iface_manager.ifaces.push(0);
            };
            let mut add_iface_fut = pin!(add_iface_fut);
            assert_matches!(exec.run_until_stalled(&mut add_iface_fut), Poll::Ready(()));
        }

        // Setup the Listener to look like it has an interface.
        let (sme, _) = create_proxy::<fidl_sme::ClientSmeMarker>();
        let iface_ref = IfaceRef::new();
        iface_ref.set_if_empty(Iface { sme, iface_id: 0 });

        let listener = Listener::new(
            test_values.monitor_proxy,
            iface_ref,
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Run the iface removal handler.
        let fut = handle_event(&listener, DeviceWatcherEvent::OnIfaceRemoved { iface_id: 0 });
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // The PhyManager and IfaceManager should have no reference to the interface.
        {
            let phy_manager = test_values.phy_manager.clone();
            let iface_manager = test_values.iface_manager.clone();
            let verify_fut = async move {
                let phy_manager = phy_manager.lock().await;
                let iface_manager = iface_manager.lock().await;
                assert!(phy_manager.ifaces.is_empty());
                assert!(iface_manager.ifaces.is_empty());
            };
            let mut verify_fut = pin!(verify_fut);
            assert_matches!(exec.run_until_stalled(&mut verify_fut), Poll::Ready(()));
        }

        // The IfaceRef should be empty.
        assert!(listener.legacy_shim.get().is_err());
    }

    #[fuchsia::test]
    fn test_handle_iface_added_succeeds() {
        let mut exec = fasync::TestExecutor::new();
        let mut test_values = test_setup(false, true);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        let fut = handle_event(&listener, DeviceWatcherEvent::OnIfaceAdded { iface_id: 0 });
        let mut fut = pin!(fut);

        // The future should stall out while performing the legacy add interface routine.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Reply to the query indicating that this is a client.
        let iface_response = Some(fidl_service::QueryIfaceResponse {
            role: fidl_common::WlanMacRole::Client,
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr: [0, 1, 2, 3, 4, 5],
            factory_addr: [0, 1, 2, 3, 4, 5],
        });
        send_query_iface_response(&mut exec, &mut test_values.monitor_stream, iface_response);

        // The future should stall again while requesting a client SME proxy.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_service::DeviceMonitorRequest::GetClientSme {
                iface_id: 0, sme_server: _, responder
            }))) => {
                assert!(responder.send(Ok(())).is_ok())
            }
        );

        // The future should not run to completion
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that the IfaceManager has been notified of the new interface.
        {
            let iface_manager = test_values.iface_manager.clone();
            let verify_fut = async move {
                let iface_manager = iface_manager.lock().await;
                assert_eq!(iface_manager.ifaces, vec![0]);
            };
            let mut verify_fut = pin!(verify_fut);
            assert_matches!(exec.run_until_stalled(&mut verify_fut), Poll::Ready(()));
        }

        // The IfaceRef should have also been updated.
        assert!(listener.legacy_shim.get().is_ok());
    }

    #[fuchsia::test]
    fn test_handle_iface_added_fails_due_to_monitor_service() {
        let mut exec = fasync::TestExecutor::new();
        let test_values = test_setup(false, true);

        let listener = Listener::new(
            test_values.monitor_proxy,
            IfaceRef::new(),
            test_values.phy_manager.clone(),
            test_values.iface_manager.clone(),
        );

        // Drop the monitor stream so that querying the interface fails while attempting to create
        // the legacy shim.
        drop(test_values.monitor_stream);

        // Handle the interface addition and expect it to complete immediately.
        let fut = handle_event(&listener, DeviceWatcherEvent::OnIfaceAdded { iface_id: 0 });
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));

        // Verify that the IfaceManager is updated.
        {
            let iface_manager = test_values.iface_manager.clone();
            let verify_fut = async move {
                let iface_manager = iface_manager.lock().await;
                assert_eq!(iface_manager.ifaces, vec![0]);
            };
            let mut verify_fut = pin!(verify_fut);
            assert_matches!(exec.run_until_stalled(&mut verify_fut), Poll::Ready(()));
        }

        // Verify that the IfaceRef was not updated.
        assert!(listener.legacy_shim.get().is_err());
    }

    #[derive(Debug)]
    struct FakePhyManager {
        phys: Vec<u16>,
        ifaces: Vec<u16>,
        failed_phys: u32,
        add_phy_succeeds: bool,
        add_iface_succeeds: bool,
    }

    impl FakePhyManager {
        fn new(add_phy_succeeds: bool, add_iface_succeeds: bool) -> Self {
            FakePhyManager {
                phys: Vec::new(),
                ifaces: Vec::new(),
                failed_phys: 0,
                add_phy_succeeds,
                add_iface_succeeds,
            }
        }
    }

    #[async_trait(?Send)]
    impl PhyManagerApi for FakePhyManager {
        async fn add_phy(&mut self, phy_id: u16) -> Result<(), PhyManagerError> {
            if self.add_phy_succeeds {
                self.phys.push(phy_id);
                Ok(())
            } else {
                Err(PhyManagerError::PhyQueryFailure)
            }
        }

        fn remove_phy(&mut self, phy_id: u16) {
            self.phys.retain(|phy| *phy != phy_id)
        }

        async fn on_iface_added(&mut self, iface_id: u16) -> Result<(), PhyManagerError> {
            if self.add_iface_succeeds {
                self.ifaces.push(iface_id);
                Ok(())
            } else {
                Err(PhyManagerError::IfaceQueryFailure)
            }
        }

        fn on_iface_removed(&mut self, iface_id: u16) {
            self.ifaces.retain(|iface| *iface != iface_id)
        }

        async fn create_all_client_ifaces(
            &mut self,
            _reason: CreateClientIfacesReason,
        ) -> HashMap<u16, Result<Vec<u16>, PhyManagerError>> {
            unimplemented!()
        }

        fn client_connections_enabled(&self) -> bool {
            unimplemented!()
        }

        async fn destroy_all_client_ifaces(&mut self) -> Result<(), PhyManagerError> {
            unimplemented!()
        }

        fn get_client(&mut self) -> Option<u16> {
            unimplemented!();
        }

        async fn create_or_get_ap_iface(&mut self) -> Result<Option<u16>, PhyManagerError> {
            unimplemented!();
        }

        async fn destroy_ap_iface(&mut self, _iface_id: u16) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        async fn destroy_all_ap_ifaces(&mut self) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn suggest_ap_mac(&mut self, _mac: MacAddr) {
            unimplemented!()
        }

        fn get_phy_ids(&self) -> Vec<u16> {
            unimplemented!()
        }

        fn log_phy_add_failure(&mut self) {
            self.failed_phys += 1;
        }

        async fn set_country_code(
            &mut self,
            _country_code: Option<client_types::CountryCode>,
        ) -> Result<(), PhyManagerError> {
            unimplemented!();
        }

        fn record_defect(&mut self, _defect: Defect) {
            unimplemented!();
        }

        async fn perform_recovery(&mut self, _summary: RecoverySummary) {
            unimplemented!();
        }
    }

    #[derive(Debug)]
    struct FakeIfaceManager {
        ifaces: Vec<u16>,
    }

    impl FakeIfaceManager {
        fn new() -> Self {
            FakeIfaceManager { ifaces: Vec::new() }
        }
    }

    #[async_trait(?Send)]
    impl IfaceManagerApi for FakeIfaceManager {
        async fn disconnect(
            &mut self,
            _network_id: client_types::NetworkIdentifier,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!();
        }

        async fn connect(&mut self, _connect_req: ConnectAttemptRequest) -> Result<(), Error> {
            unimplemented!();
        }

        async fn record_idle_client(&mut self, _iface_id: u16) -> Result<(), Error> {
            unimplemented!();
        }

        async fn has_idle_client(&mut self) -> Result<bool, Error> {
            unimplemented!();
        }

        async fn handle_added_iface(&mut self, iface_id: u16) -> Result<(), Error> {
            self.ifaces.push(iface_id);
            Ok(())
        }

        async fn handle_removed_iface(&mut self, iface_id: u16) -> Result<(), Error> {
            self.ifaces.retain(|iface| *iface != iface_id);
            Ok(())
        }

        async fn get_sme_proxy_for_scan(&mut self) -> Result<SmeForScan, Error> {
            unimplemented!()
        }

        async fn stop_client_connections(
            &mut self,
            _reason: client_types::DisconnectReason,
        ) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_client_connections(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn start_ap(
            &mut self,
            _config: ap_fsm::ApConfig,
        ) -> Result<oneshot::Receiver<()>, Error> {
            unimplemented!();
        }

        async fn stop_ap(
            &mut self,
            _ssid: ap_types::Ssid,
            _password: Vec<u8>,
        ) -> Result<(), Error> {
            unimplemented!();
        }

        async fn stop_all_aps(&mut self) -> Result<(), Error> {
            unimplemented!()
        }

        async fn set_country(
            &mut self,
            _country_code: Option<client_types::CountryCode>,
        ) -> Result<(), Error> {
            unimplemented!();
        }
    }

    #[track_caller]
    fn send_query_iface_response(
        exec: &mut fasync::TestExecutor,
        server: &mut fidl_service::DeviceMonitorRequestStream,
        iface_info: Option<fidl_service::QueryIfaceResponse>,
    ) {
        let response = iface_info.as_ref().ok_or(zx::sys::ZX_ERR_NOT_FOUND);
        assert_matches!(
            exec.run_until_stalled(&mut server.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::QueryIface {
                    iface_id: _,
                    responder,
                }
            ))) => {
                responder.send(response).expect("sending fake iface info");
            }
        );
    }
}
