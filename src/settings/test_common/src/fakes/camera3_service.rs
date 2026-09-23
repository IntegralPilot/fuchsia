// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fakes::service::Service;
use anyhow::{Error, format_err};
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fidl_fuchsia_camera3::{
    DeviceRequest, DeviceWatcherMarker, DeviceWatcherRequest, WatchDevicesEvent,
};
use fuchsia_async::{self as fasync, DurationExt};
use futures::TryStreamExt;
use settings_camera::CAMERA_WATCHER_TIMEOUT;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use zx::MonotonicDuration;

pub struct Camera3Service {
    camera_sw_muted: Rc<AtomicBool>,
    // If true, sends the camera device response immediately. If false, sends
    // an empty device list. If None, delay_camera_device should be provided.
    has_camera_device: Rc<AtomicBool>,
    // If true, first sends an empty device list on watch, then delays, then
    // sends the camera device response.
    delay_camera_device: Rc<AtomicBool>,
}

impl Camera3Service {
    pub fn new(has_camera_device: bool) -> Self {
        Self {
            camera_sw_muted: Rc::new(AtomicBool::new(false)),
            has_camera_device: Rc::new(AtomicBool::new(has_camera_device)),
            delay_camera_device: Rc::new(AtomicBool::new(false)),
        }
    }

    pub fn new_delayed_devices(delay_camera_device: bool) -> Self {
        Self {
            camera_sw_muted: Rc::new(AtomicBool::new(false)),
            has_camera_device: Rc::new(AtomicBool::new(false)),
            delay_camera_device: Rc::new(AtomicBool::new(delay_camera_device)),
        }
    }

    pub fn camera_sw_muted(&self) -> bool {
        (*self.camera_sw_muted).load(Ordering::Relaxed)
    }

    pub fn set_camera_sw_muted(&self, muted: bool) {
        let _ = (*self.camera_sw_muted).swap(muted, Ordering::Relaxed);
    }
}

impl Service for Camera3Service {
    fn can_handle_service(&self, service_name: &str) -> bool {
        service_name == DeviceWatcherMarker::PROTOCOL_NAME
    }

    fn process_stream(&mut self, service_name: &str, channel: zx::Channel) -> Result<(), Error> {
        if !self.can_handle_service(service_name) {
            return Err(format_err!("can't handle service"));
        }

        let mut device_watcher_stream =
            ServerEnd::<DeviceWatcherMarker>::new(channel).into_stream();

        let camera_sw_muted = Rc::clone(&self.camera_sw_muted);
        let has_camera_device = Rc::clone(&self.has_camera_device);
        let delay_camera_device = Rc::clone(&self.delay_camera_device);
        let mut watch_count = 0;
        fasync::Task::local(async move {
            while let Some(req) = device_watcher_stream.try_next().await.unwrap() {
                // Support future expansion of FIDL.
                #[allow(unreachable_patterns)]
                match req {
                    DeviceWatcherRequest::WatchDevices { responder } => {
                        let camera_device = WatchDevicesEvent::Added(1);
                        let camera_devices = &[camera_device][..];
                        let empty_devices = &[][..];

                        let devices = if delay_camera_device.load(Ordering::Relaxed) {
                            if watch_count == 0 {
                                watch_count += 1;
                                empty_devices
                            } else {
                                let timer = fasync::Timer::new(
                                    MonotonicDuration::from_millis(CAMERA_WATCHER_TIMEOUT / 2)
                                        .after_now(),
                                );
                                timer.await;
                                camera_devices
                            }
                        } else if has_camera_device.load(Ordering::Relaxed) {
                            camera_devices
                        } else {
                            empty_devices
                        };
                        responder.send(devices).expect("Failed to send devices response");
                    }
                    DeviceWatcherRequest::ConnectToDevice {
                        id: _,
                        request, // ServerEnd<DeviceMarker>
                        control_handle: _,
                    } => {
                        let mut stream = request.into_stream();
                        let camera_sw_muted = Rc::clone(&camera_sw_muted);
                        fasync::Task::local(async move {
                            while let Some(req) = stream.try_next().await.unwrap() {
                                // Support future expansion of FIDL.
                                match req {
                                    DeviceRequest::SetSoftwareMuteState { muted, responder } => {
                                        let _ = camera_sw_muted.swap(muted, Ordering::Relaxed);
                                        let _ = responder.send();
                                    }
                                    DeviceRequest::WatchMuteState { responder } => {
                                        let _ = responder
                                            .send(camera_sw_muted.load(Ordering::Relaxed), false);
                                    }
                                    _ => {}
                                }
                            }
                        })
                        .detach();
                    }
                    _ => {}
                }
            }
        })
        .detach();

        Ok(())
    }
}
