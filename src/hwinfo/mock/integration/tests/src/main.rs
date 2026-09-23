// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_hwinfo::{
    Architecture, BoardInfo, BoardMarker, BoardProxy, DeviceInfo, DeviceMarker, DeviceProxy,
    ProductInfo, ProductMarker, ProductProxy,
};
use fidl_fuchsia_hwinfo_mock::{SetterMarker, SetterProxy};
use fidl_fuchsia_intl::RegulatoryDomain;
use fidl_test_mock as ftest;
use fuchsia_component::client::connect_to_protocol;
use log::info;
use realm_proxy_client::RealmProxyClient;

async fn create_realm(options: ftest::RealmOptions) -> Result<RealmProxyClient> {
    let realm_factory = connect_to_protocol::<ftest::RealmFactoryMarker>()?;
    let (client, server) = create_endpoints();

    realm_factory
        .create_realm(options, server)
        .await?
        .map_err(realm_proxy_client::Error::OperationError)?;

    info!("Connected to realm");

    Ok(RealmProxyClient::from(client))
}

#[fuchsia::test]
async fn test_invalid_board() -> Result<()> {
    let realm_options = ftest::RealmOptions { ..Default::default() };
    let client = create_realm(realm_options).await?;
    let proxy: BoardProxy = client.connect_to_protocol::<BoardMarker>().await?;
    assert!(proxy.get_info().await.is_err(), "Expect that reading without setting fails");
    Ok(())
}

#[fuchsia::test]
async fn test_invalid_product() -> Result<()> {
    let realm_options = ftest::RealmOptions { ..Default::default() };
    let client = create_realm(realm_options).await?;
    let proxy: ProductProxy = client.connect_to_protocol::<ProductMarker>().await?;
    assert!(proxy.get_info().await.is_err(), "Expect that reading without setting fails");
    Ok(())
}

#[fuchsia::test]
async fn test_invalid_device() -> Result<()> {
    let realm_options = ftest::RealmOptions { ..Default::default() };
    let client = create_realm(realm_options).await?;
    let proxy: DeviceProxy = client.connect_to_protocol::<DeviceMarker>().await?;
    assert!(proxy.get_info().await.is_err(), "Expect that reading without setting fails");
    Ok(())
}

#[fuchsia::test]
async fn test_valid_data() -> Result<()> {
    let realm_options = ftest::RealmOptions { ..Default::default() };
    let client = create_realm(realm_options).await?;

    let device_info = DeviceInfo {
        serial_number: Some("1234".to_string()),
        is_retail_demo: Some(false),
        retail_sku: Some("5678".to_string()),
        ..DeviceInfo::default()
    };

    let board_info = BoardInfo {
        name: Some("fake-board".to_string()),
        revision: Some("10".to_string()),
        cpu_architecture: Some(Architecture::X64),
        ..BoardInfo::default()
    };

    let product_info = ProductInfo {
        sku: Some("abcd".to_string()),
        language: Some("unknown".to_string()),
        regulatory_domain: Some(RegulatoryDomain {
            country_code: Some("us".to_string()),
            ..RegulatoryDomain::default()
        }),
        locale_list: Some(vec![]),
        name: Some("my product".to_string()),
        model: Some("my model".to_string()),
        manufacturer: Some("google".to_string()),
        build_date: Some("20230525".to_string()),
        build_name: Some("BUILD".to_string()),
        colorway: Some("colorway".to_string()),
        display: Some("display".to_string()),
        memory: Some("100GB".to_string()),
        nand_storage: Some("1000GB".to_string()),
        emmc_storage: Some("10TB".to_string()),
        microphone: Some("my microphone".to_string()),
        audio_amplifier: Some("my audio amplifier".to_string()),
        ..ProductInfo::default()
    };

    let setter_proxy: SetterProxy = client.connect_to_protocol::<SetterMarker>().await?;
    setter_proxy.set_responses(&device_info, &product_info, &board_info).await?;

    let device: DeviceProxy = client.connect_to_protocol::<DeviceMarker>().await?;
    let board: BoardProxy = client.connect_to_protocol::<BoardMarker>().await?;
    let product: ProductProxy = client.connect_to_protocol::<ProductMarker>().await?;

    let actual_info = device.get_info().await?;
    assert_eq!(actual_info, device_info);

    let actual_info = board.get_info().await?;
    assert_eq!(actual_info, board_info);

    let actual_info = product.get_info().await?;
    assert_eq!(actual_info, product_info);

    Ok(())
}
