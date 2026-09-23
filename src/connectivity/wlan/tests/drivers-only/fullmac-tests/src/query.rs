// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FullmacDriverFixture;
use assert_matches::assert_matches;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fullmac_helpers::config::{FullmacDriverConfig, default_fullmac_query_info};
use rand::seq::IndexedRandom as _;

#[fuchsia::test]
async fn test_generic_sme_query() {
    // The role and sta_addr are randomly generated for each run of the test case to ensure that
    // the platform driver doesn't hardcode either of these values.
    let roles = [fidl_common::WlanMacRole::Client, fidl_common::WlanMacRole::Ap];
    let config = FullmacDriverConfig {
        query_info: fidl_fullmac::WlanFullmacImplQueryResponse {
            sta_addr: Some(rand::random()),
            role: Some(*roles.choose(&mut rand::rng()).unwrap()),
            ..default_fullmac_query_info()
        },
        ..Default::default()
    };

    let mut fullmac_driver = FullmacDriverFixture::create(config).await;

    let generic_sme_proxy = &fullmac_driver.generic_sme_proxy;
    let request_stream = &mut fullmac_driver.request_stream;

    // Returns the query response
    let sme_fut = async { generic_sme_proxy.query().await.expect("Failed to request SME query") };

    let driver_fut = async {
        assert_matches!(request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::Query { responder } => {
            responder.send(Ok(&fullmac_driver.config.query_info))
                .expect("Failed to respondt to Query");
        });
    };

    let (query_resp, _) = futures::join!(sme_fut, driver_fut);
    assert_eq!(query_resp.role, fullmac_driver.config.query_info.role.unwrap());
    assert_eq!(query_resp.sta_addr, fullmac_driver.sta_addr());
}
