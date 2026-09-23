// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::sensor_manager::SensorManager;
use anyhow::Error;
use fidl_fuchsia_hardware_sensors::{DriverProxy, ServiceMarker};
use fuchsia_component::client as fclient;
use futures::lock::Mutex;
use futures_util::TryStreamExt;
use std::sync::Arc;

// Watches the sensor service for new providers of fuchsia.hardware.sensors.Driver and connects to
// them. If the connection is successful, it hands the proxy to the SensorManager to manage
// their configurations and events.
pub(crate) async fn watch_service_directory(
    sensor_service: fclient::Service<ServiceMarker>,
    manager: Arc<Mutex<SensorManager>>,
) -> Result<(), Error> {
    let mut watcher = sensor_service.watch().await?;
    while let Ok(Some(instance)) = watcher.try_next().await {
        let instance_name = instance.instance_name();
        log::info!("Found new sensor service provider: {}", instance_name);
        let driver_proxy_res = instance.connect_to_driver();
        let driver_proxy: DriverProxy = if let Ok(proxy) = driver_proxy_res {
            proxy
        } else {
            log::error!("Failed to connect to driver instance: {:#?}", driver_proxy_res);
            continue;
        };
        let mut sm = manager.lock().await;
        sm.add_instance(driver_proxy).await;
    }
    Ok(())
}
