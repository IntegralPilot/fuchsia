// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Error, format_err};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_camera3::{
    DeviceMarker, DeviceProxy as Camera3DeviceProxy, DeviceWatcherMarker,
    DeviceWatcherProxy as Camera3DeviceWatcherProxy, WatchDevicesEvent,
};
use fuchsia_async::{self as fasync, DurationExt};
use futures::FutureExt;
use futures::future::Fuse;
use settings_common::inspect::event::ExternalEventPublisher;
use settings_common::service_context::{ExternalServiceProxy, ServiceContext};
use settings_common::{call, call_async};
use zx::MonotonicDuration;

/// The amount of time in milliseconds to wait for a camera device to be detected.
pub const CAMERA_WATCHER_TIMEOUT: i64 = 30_000;

/// Retrieves the id of a camera device given the camera device watcher proxy.
async fn get_camera_id(
    camera_watcher_proxy: &ExternalServiceProxy<Camera3DeviceWatcherProxy, ExternalEventPublisher>,
) -> Result<u64, Error> {
    // Get a list of id structs containing existing, new, and removed ids.

    // Sets a timer and watches for changes from the camera api. If the first response is empty,
    // continue to watch for an update to the devices. If we receive a nonempty response,
    // we extract the id and return. If the timeout is reached, then it is assumed to be an error.
    let timer =
        fasync::Timer::new(MonotonicDuration::from_millis(CAMERA_WATCHER_TIMEOUT).after_now())
            .fuse();
    let camera_ids = call_async!(camera_watcher_proxy => watch_devices()).fuse();

    // Used to add the second watch call if the first comes back with empty devices.
    let unfulfilled_future = Fuse::terminated();

    futures::pin_mut!(timer, camera_ids, unfulfilled_future);
    loop {
        futures::select! {
            ids_result = camera_ids => {
                let ids = ids_result?;
                if ids.is_empty() {
                    // The camera list might not be initialized yet, make another watch call and
                    // keep waiting.
                    let next_camera_ids = call_async!(camera_watcher_proxy => watch_devices()).fuse();
                    unfulfilled_future.set(next_camera_ids);
                } else {
                    // Nonempty response, extract id.
                    return extract_cam_id(ids);
                }
            }
            ids_result_second = unfulfilled_future => {
                let ids = ids_result_second?;
                return extract_cam_id(ids);
            }
            _ = timer => {
                return Err(format_err!("Could not find a camera"));
            }
        }
    }
}

/// Extract the camera id from the list of ids. Assumes there is only one camera.
fn extract_cam_id(ids: Vec<WatchDevicesEvent>) -> Result<u64, Error> {
    let first_cam = ids.first();
    if let Some(WatchDevicesEvent::Existing(id)) | Some(WatchDevicesEvent::Added(id)) = first_cam {
        Ok(*id)
    } else {
        Err(format_err!("Could not find a camera"))
    }
}

/// Establishes a connection to the fuchsia.camera3.Device api by watching
/// the camera id and using it to connect to the device.
pub async fn connect_to_camera(
    service_context: &ServiceContext,
    external_publisher: ExternalEventPublisher,
) -> Result<Camera3DeviceProxy, Error> {
    // Connect to the camera device watcher to get camera ids. This will
    // be used to connect to the camera.
    let camera_watcher_proxy = service_context
        .connect_with_publisher::<DeviceWatcherMarker, _>(external_publisher)
        .await?;
    let camera_id = get_camera_id(&camera_watcher_proxy).await?;

    // Connect to the camera device with the found id.
    let (camera_proxy, device_server) = create_proxy::<DeviceMarker>();
    if call!(camera_watcher_proxy => connect_to_device(camera_id, device_server)).is_err() {
        return Err(format_err!("Could not connect to fuchsia.camera3.DeviceWatcher device"));
    }
    Ok(camera_proxy)
}
