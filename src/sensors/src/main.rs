// Copyright 2023 The Fuchsia Authors.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Error;
use fidl_fuchsia_hardware_sensors as driver_fidl;
use fuchsia_component::client as fclient;
use futures::channel::mpsc;
use futures::lock::Mutex;
use sensors_lib::playback::Playback;
use sensors_lib::sensor_manager::SensorManager;
use sensors_lib::sensor_update_sender::*;
use std::sync::Arc;

const SENSOR_UPDATE_BUFFER_SIZE: usize = 100;

async fn connect_to_playback() -> Option<(driver_fidl::DriverProxy, driver_fidl::PlaybackProxy)> {
    let playback_proxy_res = fclient::connect_to_protocol::<driver_fidl::PlaybackMarker>();

    let playback_proxy = if let Ok(playback_proxy) = playback_proxy_res {
        playback_proxy
    } else {
        log::warn!(
            "Failed to connect to sensor playback driver protocol. {:#?}",
            playback_proxy_res
        );
        return None;
    };

    let mut playback_driver_proxy: Option<driver_fidl::DriverProxy> = None;

    // Attempt to open and connect to the playback service exposed by the subpackaged
    // sensors_playback. This service is exposed at /svc/fuchsia.hardware.sensors.Service.Playback to
    // differentiate it from the real services.
    //
    // TODO(b/370821933): Instead of connecting to the playback service before creating the sensor
    // manager, we should wait for playback to be configured so that playback is treated as a
    // normal driver.
    if let Ok(playback_exposed_dir) =
        fclient::open_childs_exposed_directory("sensors_playback", None).await
    {
        async fn connect_to_instance(
            playback_exposed_dir: &fidl_fuchsia_io::DirectoryProxy,
        ) -> Result<driver_fidl::DriverProxy, Error> {
            Ok(fuchsia_component::client::Service::open_from_dir(
                playback_exposed_dir,
                driver_fidl::ServiceMarker,
            )?
            .watch_for_any()
            .await?
            .connect_to_driver()?)
        }
        if let Ok(proxy) = connect_to_instance(&playback_exposed_dir).await {
            playback_driver_proxy = Some(proxy);
        }
    }

    if let Some(driver_proxy) = playback_driver_proxy {
        Some((driver_proxy, playback_proxy))
    } else {
        log::warn!("Failed to connect to sensor playback driver service.");
        None
    }
}

#[fuchsia::main(logging_tags = [ "sensors" ])]
async fn main() -> Result<(), Error> {
    log::info!("Sensors Server Started");

    let playback_proxies = connect_to_playback().await;

    let mut playback: Option<Playback> = None;
    if let Some((driver_proxy, playback_proxy)) = playback_proxies {
        playback = Some(Playback::new(driver_proxy, playback_proxy));
    }

    let (sender, receiver) = mpsc::channel(SENSOR_UPDATE_BUFFER_SIZE);
    let sender = SensorUpdateSender::new(Arc::new(Mutex::new(sender)));

    fuchsia_async::Task::spawn(async move {
        handle_sensor_event_streams(receiver).await;
    })
    .detach();

    let mut sensor_manager = SensorManager::new(sender, playback);

    // This should run forever.
    let result = sensor_manager.run().await;
    log::error!("Unexpected exit with result: {:?}", result);
    result
}
