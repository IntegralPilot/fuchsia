// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::proxies::Proxies;
use anyhow::anyhow;
use fidl_fuchsia_bluetooth::Uuid;
use fidl_fuchsia_bluetooth_gatt2::{
    Characteristic, LocalServiceMarker, LocalServiceRequestStream, ServiceHandle, ServiceInfo,
};
use futures::StreamExt;

pub(crate) async fn publish_service(
    proxies: &Proxies,
    uuid: Uuid,
    service_handle: ServiceHandle,
    characteristics: Vec<Characteristic>,
) -> Result<LocalServiceRequestStream, anyhow::Error> {
    let service_info = ServiceInfo {
        handle: Some(service_handle),
        kind: None, // default: ServiceKind::PRIMARY
        type_: Some(uuid),
        characteristics: Some(characteristics),
        ..Default::default()
    };
    let (service_client, service_stream) =
        fidl::endpoints::create_request_stream::<LocalServiceMarker>();

    if let Err(err) =
        proxies.gatt_server_proxy.publish_service(&service_info, service_client).await?
    {
        return Err(anyhow!("fuchsia.bluetooth.gatt2.Server/PublishService error: {err:?}"));
    }

    Ok(service_stream)
}

pub(crate) async fn discover_services(
    proxies: &mut Proxies,
) -> Result<Vec<ServiceInfo>, anyhow::Error> {
    // To ensure we receive a full snapshot of the peer's services, re-init the gatt client.
    let _ = proxies.gatt_client.take();
    let connection = proxies
        .central_connection
        .lock()
        .clone()
        .ok_or_else(|| anyhow!("GATT connection not established"))?;
    let (gatt_client, gatt_client_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_gatt2::ClientMarker>();
    connection.request_gatt_client(gatt_client_server_end)?;
    let (mut updated, _removed) = gatt_client.watch_services(&[]).await?;
    proxies.gatt_client = Some(gatt_client);

    for service in updated.iter_mut() {
        let (service_proxy, service_server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_gatt2::RemoteServiceMarker>();
        proxies
            .gatt_client
            .clone()
            .unwrap()
            .connect_to_service(&service.handle.as_ref().unwrap(), service_server_end)?;
        service.characteristics = Some(service_proxy.discover_characteristics().await?);
    }

    Ok(updated)
}

pub(crate) async fn read_characteristic(
    proxies: &Proxies,
    service_handle: ServiceHandle,
    characteristic_handle: fidl_fuchsia_bluetooth_gatt2::Handle,
) -> Result<fidl_fuchsia_bluetooth_gatt2::ReadValue, anyhow::Error> {
    let Some(gatt_client) = proxies.gatt_client.clone() else {
        return Err(anyhow!("GATT client is not connected"));
    };

    let (service_proxy, service_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_gatt2::RemoteServiceMarker>();
    gatt_client.connect_to_service(&service_handle, service_server_end)?;

    let value = service_proxy
        .read_characteristic(
            &characteristic_handle,
            &fidl_fuchsia_bluetooth_gatt2::ReadOptions::ShortRead(
                fidl_fuchsia_bluetooth_gatt2::ShortReadOptions {},
            ),
        )
        .await?
        .map_err(|e| {
            anyhow!("fuchsia.bluetooth.gatt2.RemoteService/ReadCharacteristic error: {:?}", e)
        })?;

    Ok(value)
}

pub(crate) async fn register_characteristic_notifier(
    proxies: &Proxies,
    service_handle: ServiceHandle,
    characteristic_handle: fidl_fuchsia_bluetooth_gatt2::Handle,
) -> Result<(), anyhow::Error> {
    let Some(gatt_client) = proxies.gatt_client.clone() else {
        return Err(anyhow!("GATT client is not connected"));
    };
    if let Some(_) = proxies.remote_service_proxy.lock().take() {
        println!("Clearing RemoteServiceProxy; any pending operations are cancelled.");
    }

    let (service_proxy, service_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_bluetooth_gatt2::RemoteServiceMarker>();
    gatt_client.connect_to_service(&service_handle, service_server_end)?;

    let (listener_client, mut listener_stream) = fidl::endpoints::create_request_stream::<
        fidl_fuchsia_bluetooth_gatt2::CharacteristicNotifierMarker,
    >();

    service_proxy
        .register_characteristic_notifier(&characteristic_handle, listener_client)
        .await?
        .map_err(|e| {
            anyhow!(
                "fuchsia.bluetooth.gatt2.RemoteService/RegisterCharacteristicNotifier error: {:?}",
                e
            )
        })?;
    *proxies.remote_service_proxy.lock() = Some(service_proxy);
    *proxies.characteristic_notifier_task.lock() = Some(fuchsia_async::Task::spawn(async move {
        while let Some(Ok(request)) = listener_stream.next().await {
            // Just log the request for now.
            println!("Received CharacteristicNotifier request: {:?}", request);
        }
    }));

    Ok(())
}
