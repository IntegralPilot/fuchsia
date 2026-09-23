// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use drivers_only_common::DriversOnlyTestRealm;
use fidl::endpoints::create_endpoints;
use fullmac_helpers::config::FullmacDriverConfig;
use fullmac_helpers::recorded_request_stream::RecordedRequestStream;
use {
    fidl_fuchsia_wlan_fullmac as fidl_fullmac, fidl_fuchsia_wlan_sme as fidl_sme,
    fidl_test_wlan_testcontroller as fidl_testcontroller, fuchsia_async as fasync,
};

mod ap;
mod client;
mod query;
mod telemetry;

/// Fixture that holds all the relevant data and proxies for the fullmac driver.
/// This can be shared among the different types of tests (client, telemetry, AP).
///
/// Note that wlanif considers it an error for |generic_sme_proxy| to be closed before wlanif is
/// torn down. To avoid that, |FullmacDriverFixture| contains |generic_sme_proxy| to ensure that it
/// outlives wlanif.
struct FullmacDriverFixture {
    id: fidl_testcontroller::FullmacId,
    config: FullmacDriverConfig,
    ifc_proxy: fidl_fullmac::WlanFullmacImplIfcProxy,
    request_stream: RecordedRequestStream,
    generic_sme_proxy: fidl_sme::GenericSmeProxy,
    realm: DriversOnlyTestRealm,
}

impl FullmacDriverFixture {
    async fn create(config: FullmacDriverConfig) -> Self {
        let realm = DriversOnlyTestRealm::new().await;
        let (id, fullmac_req_stream, fullmac_ifc_proxy, generic_sme_proxy) =
            fullmac_helpers::create_fullmac_driver(realm.testcontroller_proxy(), &config).await;
        log::info!("Created fullmac driver {:?}", id);
        Self {
            id,
            config,
            ifc_proxy: fullmac_ifc_proxy,
            generic_sme_proxy,
            request_stream: RecordedRequestStream::new(fullmac_req_stream),
            realm,
        }
    }

    fn sta_addr(&self) -> [u8; 6] {
        self.config.query_info.sta_addr.unwrap()
    }
}

impl Drop for FullmacDriverFixture {
    fn drop(&mut self) {
        log::info!("FullmacDriverFixture deleting fullmac driver {:?}...", self.id);
        let testcontroller_sync_proxy = self.realm.take_sync_testcontroller_proxy();
        testcontroller_sync_proxy
            .delete_fullmac(self.id, zx::MonotonicInstant::INFINITE)
            .expect("FIDL error when calling DeleteFullmac")
            .expect("TestController error when calling DeleteFullmac");
    }
}

/// This tests that wlanif gracefully unbinds on a delete request that is sent during startup.
/// It is expected that MLME goes through the whole startup sequence before handling the incoming
/// delete request and shutting down.
///
/// When wlanif unbinds, it sends a `Stop` event to MLME's DriverEvent queue that gets handled in
/// MLME's main loop. The purpose of this test is to show that if wlanif receives an early unbind
/// request, it will wait until MLME handles this `Stop` event and gracefully shuts down.
#[fuchsia::test]
async fn test_delete_on_startup() {
    let realm = DriversOnlyTestRealm::new().await;

    let (fullmac_bridge_client, fullmac_bridge_server) = create_endpoints();
    let fullmac_id = realm
        .testcontroller_proxy()
        .create_fullmac(fullmac_bridge_client)
        .await
        .expect("FIDL error on create_fullmac")
        .expect("TestController returned an error on create fullmac");
    let mut fullmac_bridge_stream = fullmac_bridge_server.into_stream();
    let config = FullmacDriverConfig { ..Default::default() };

    let delete_fut = realm.testcontroller_proxy().delete_fullmac(fullmac_id);
    let startup_fut = async {
        // Sleep to give time for test controller to receive (and ideally complete) the delete
        // request before handling startup. This should allow PrepareStop in wlanif to run as
        // quickly as possible.
        fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        fullmac_helpers::handle_fullmac_startup(&mut fullmac_bridge_stream, &config).await
    };

    // Bind to generic_sme_proxy on join to ensure it outlives wlanif
    let (delete_fullmac_result, (_fullmac_ifc_proxy, _generic_sme_proxy)) =
        futures::join!(delete_fut, startup_fut);
    delete_fullmac_result
        .expect("FIDL error on DeleteFullmac")
        .expect("TestController error on DeleteFullmac");
}
