// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use fidl_fuchsia_wlan_common::WlanMacRole;
use fidl_fuchsia_wlan_device_service::{
    DestroyIfaceRequest, DeviceMonitorCreateIfaceRequest, DeviceMonitorCreateIfaceResponse,
    DeviceMonitorProxy, QueryIfaceResponse,
};
use log::info;

use ieee80211::{MacAddr, MacAddrBytes};

pub mod ap;
pub mod client;

// Helper methods for calling wlan_service fidl methods
pub async fn get_iface_list(monitor_proxy: &DeviceMonitorProxy) -> Result<Vec<u16>, Error> {
    monitor_proxy.list_ifaces().await.context("Error getting iface list")
}

/// Returns the first iface id with the requested role
///
/// # Arguments: 2
/// * `monitor_proxy`: a DeviceMonitorProxy
/// * 'role' : requested WlanMacRole (client or ap)
pub async fn get_first_iface(
    monitor_proxy: &DeviceMonitorProxy,
    role: WlanMacRole,
) -> Result<u16, Error> {
    let wlan_iface_ids =
        get_iface_list(monitor_proxy).await.context("Connect: failed to get wlan iface list")?;

    if wlan_iface_ids.len() == 0 {
        return Err(format_err!("No wlan interface found"));
    }
    info!("Found {} wlan iface entries", wlan_iface_ids.len());
    for iface_id in wlan_iface_ids {
        let iface_info = query_iface(monitor_proxy, iface_id).await?;

        if iface_info.role == role {
            return Ok(iface_id);
        }
    }
    Err(format_err!("interface with role {:?} not found", role))
}
/// Returns the list of Phy IDs for this system.
///
/// # Arguments
/// * `monitor_proxy`: a DeviceMonitorProxy
pub async fn get_phy_list(monitor_proxy: &DeviceMonitorProxy) -> Result<Vec<u16>, Error> {
    let phys = monitor_proxy.list_phys().await.context("Error getting phy list")?;
    Ok(phys)
}

pub async fn create_iface(
    monitor_proxy: &DeviceMonitorProxy,
    phy_id: u16,
    role: WlanMacRole,
    sta_address: MacAddr,
) -> Result<u16, Error> {
    let req = DeviceMonitorCreateIfaceRequest {
        phy_id: Some(phy_id),
        role: Some(role),
        sta_address: Some(sta_address.to_array()),
        ..Default::default()
    };

    match monitor_proxy.create_iface(&req).await.context("FIDL error creating iface")? {
        Err(e) => Err(format_err!("Error creating iface: {:?}", e)),
        Ok(DeviceMonitorCreateIfaceResponse { iface_id: None, .. }) => {
            Err(format_err!("Create iface completed without returning an iface id"))
        }
        Ok(DeviceMonitorCreateIfaceResponse { iface_id: Some(iface_id), .. }) => {
            info!("Created iface {:?}", iface_id);
            Ok(iface_id)
        }
    }
}

pub async fn destroy_iface(monitor_proxy: &DeviceMonitorProxy, iface_id: u16) -> Result<(), Error> {
    let req = DestroyIfaceRequest { iface_id };

    let response = monitor_proxy.destroy_iface(&req).await.context("Error destroying iface")?;
    zx::Status::ok(response).context("Destroy iface returned non-OK status")?;
    Ok(info!("Destroyed iface {:?}", iface_id))
}

async fn query_iface(
    monitor_proxy: &DeviceMonitorProxy,
    iface_id: u16,
) -> Result<QueryIfaceResponse, Error> {
    monitor_proxy.query_iface(iface_id).await.context("Error querying iface")?.map_err(|e| {
        format_err!("query_iface {} failed: {}", iface_id, zx::Status::err_from_raw(e))
    })
}

pub async fn get_wlan_sta_addr(
    monitor_proxy: &DeviceMonitorProxy,
    iface_id: u16,
) -> Result<[u8; 6], Error> {
    let iface_info = query_iface(monitor_proxy, iface_id).await?;
    Ok(iface_info.sta_addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_fuchsia_wlan_common::WlanMacRole;
    use fidl_fuchsia_wlan_device_service::{
        DeviceMonitorCreateIfaceResponse, DeviceMonitorMarker, DeviceMonitorRequest,
        DeviceMonitorRequestStream,
    };
    use fuchsia_async::TestExecutor;
    use futures::StreamExt;
    use futures::task::Poll;
    use std::pin::pin;

    pub(crate) fn setup_fake_service<M: fidl::endpoints::ProtocolMarker>()
    -> (fuchsia_async::TestExecutor, M::Proxy, M::RequestStream) {
        let exec = fuchsia_async::TestExecutor::new();
        let (proxy, server) = fidl::endpoints::create_proxy::<M>();
        (exec, proxy, server.into_stream())
    }

    fn fake_iface_query_response(
        sta_addr: [u8; 6],
        role: fidl_fuchsia_wlan_common::WlanMacRole,
    ) -> QueryIfaceResponse {
        QueryIfaceResponse {
            role,
            id: 0,
            phy_id: 0,
            phy_assigned_id: 0,
            sta_addr,
            factory_addr: sta_addr,
        }
    }

    pub fn respond_to_query_iface_list_request(
        exec: &mut TestExecutor,
        req_stream: &mut DeviceMonitorRequestStream,
        ifaces: Vec<u16>,
    ) {
        let req = exec.run_until_stalled(&mut req_stream.next());
        let responder = assert_matches!(
            req,
            Poll::Ready(Some(Ok(DeviceMonitorRequest::ListIfaces { responder }))) => responder
        );
        responder.send(&ifaces[..]).expect("fake query iface list response: send failed")
    }

    pub fn respond_to_query_iface_request(
        exec: &mut TestExecutor,
        req_stream: &mut DeviceMonitorRequestStream,
        role: fidl_fuchsia_wlan_common::WlanMacRole,
        fake_mac_addr: Option<[u8; 6]>,
    ) {
        let req = exec.run_until_stalled(&mut req_stream.next());
        let responder = assert_matches!(
            req,
            Poll::Ready(Some(Ok(DeviceMonitorRequest::QueryIface { iface_id: _, responder }))) => responder
        );
        if let Some(mac) = fake_mac_addr {
            let response = fake_iface_query_response(mac, role);
            responder.send(Ok(&response)).expect("sending fake response with mac address");
        } else {
            responder.send(Err(zx::sys::ZX_ERR_NOT_FOUND)).expect("sending fake response with none")
        }
    }

    pub fn respond_to_create_iface_request(
        exec: &mut TestExecutor,
        req_stream: &mut DeviceMonitorRequestStream,
        fake_response: Result<u16, i32>,
    ) {
        let req = exec.run_until_stalled(&mut req_stream.next());
        let responder = assert_matches!(
            req,
            Poll::Ready(Some(Ok(DeviceMonitorRequest::CreateIface { responder, .. }))) => responder
        );

        let fake_response = fake_response
            .map(|fake_iface_id| DeviceMonitorCreateIfaceResponse {
                iface_id: Some(fake_iface_id),
                ..Default::default()
            })
            .map_err(|_| fidl_fuchsia_wlan_device_service::DeviceMonitorError::Internal);
        responder
            .send(fake_response.as_ref().map_err(|e| *e))
            .expect("sending fake response with iface id");
    }

    #[test]
    fn test_get_wlan_sta_addr_ok() {
        let (mut exec, proxy, mut req_stream) = setup_fake_service::<DeviceMonitorMarker>();
        let mac_addr_fut = get_wlan_sta_addr(&proxy, 0);
        let mut mac_addr_fut = pin!(mac_addr_fut);

        assert_matches!(exec.run_until_stalled(&mut mac_addr_fut), Poll::Pending);

        respond_to_query_iface_request(
            &mut exec,
            &mut req_stream,
            WlanMacRole::Client,
            Some([1, 2, 3, 4, 5, 6]),
        );

        let mac_addr = exec.run_singlethreaded(&mut mac_addr_fut).expect("should get a mac addr");
        assert_eq!(mac_addr, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_get_wlan_sta_addr_not_found() {
        let (mut exec, proxy, mut req_stream) = setup_fake_service::<DeviceMonitorMarker>();
        let mac_addr_fut = get_wlan_sta_addr(&proxy, 0);
        let mut mac_addr_fut = pin!(mac_addr_fut);

        assert_matches!(exec.run_until_stalled(&mut mac_addr_fut), Poll::Pending);

        respond_to_query_iface_request(&mut exec, &mut req_stream, WlanMacRole::Client, None);

        let err = exec.run_singlethreaded(&mut mac_addr_fut).expect_err("should be an error");
        assert!(format!("{}", err).contains("NOT_FOUND"));
    }

    #[test]
    fn test_get_wlan_sta_addr_service_interrupted() {
        let (mut exec, proxy, req_stream) = setup_fake_service::<DeviceMonitorMarker>();
        let mac_addr_fut = get_wlan_sta_addr(&proxy, 0);
        let mut mac_addr_fut = pin!(mac_addr_fut);

        assert_matches!(exec.run_until_stalled(&mut mac_addr_fut), Poll::Pending);

        // Simulate service not being available by closing the channel
        std::mem::drop(req_stream);

        let err = exec.run_singlethreaded(&mut mac_addr_fut).expect_err("should be an error");
        assert!(format!("{}", err).contains("Error querying iface"));
    }

    #[test]
    fn test_create_iface_ok() {
        let (mut exec, proxy, mut req_stream) = setup_fake_service::<DeviceMonitorMarker>();
        let iface_id_fut = create_iface(&proxy, 0, WlanMacRole::Client, [0, 0, 0, 0, 0, 0].into());

        let mut iface_id_fut = pin!(iface_id_fut);

        assert_matches!(exec.run_until_stalled(&mut iface_id_fut), Poll::Pending);
        respond_to_create_iface_request(&mut exec, &mut req_stream, Ok(15));

        let iface_id = exec.run_singlethreaded(&mut iface_id_fut).expect("should get an iface id");

        assert_eq!(iface_id, 15);
    }

    #[test]
    fn test_create_iface_internal_err() {
        let (mut exec, proxy, mut req_stream) = setup_fake_service::<DeviceMonitorMarker>();
        let iface_id_fut = create_iface(&proxy, 0, WlanMacRole::Client, [0, 0, 0, 0, 0, 0].into());

        let mut iface_id_fut = pin!(iface_id_fut);

        assert_matches!(exec.run_until_stalled(&mut iface_id_fut), Poll::Pending);
        respond_to_create_iface_request(&mut exec, &mut req_stream, Err(zx::sys::ZX_ERR_INTERNAL));

        let err = exec.run_singlethreaded(&mut iface_id_fut).expect_err("Should get an error");
        assert!(format!("{}", err).contains("Internal"));
    }
}
