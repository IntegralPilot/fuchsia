// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fuchsia_async as fasync;
use futures::TryStreamExt;
use futures::lock::Mutex;
use settings_test_common::fakes::service::Service;
use std::rc::Rc;

#[derive(Clone)]
pub(crate) struct BrightnessService {
    manual_brightness: Rc<Mutex<Option<f32>>>,
    auto_brightness: Rc<Mutex<Option<bool>>>,
    num_changes: Rc<Mutex<u32>>,
}

impl BrightnessService {
    pub(crate) fn create() -> Self {
        BrightnessService {
            manual_brightness: Rc::new(Mutex::new(None)),
            auto_brightness: Rc::new(Mutex::new(None)),
            num_changes: Rc::new(Mutex::new(0)),
        }
    }

    pub(crate) fn get_manual_brightness(&self) -> Rc<Mutex<Option<f32>>> {
        self.manual_brightness.clone()
    }

    pub(crate) fn get_auto_brightness(&self) -> Rc<Mutex<Option<bool>>> {
        self.auto_brightness.clone()
    }
}

impl Service for BrightnessService {
    fn can_handle_service(&self, service_name: &str) -> bool {
        service_name == fidl_fuchsia_ui_brightness::ControlMarker::PROTOCOL_NAME
    }

    fn process_stream(&mut self, service_name: &str, channel: zx::Channel) -> Result<(), Error> {
        if !self.can_handle_service(service_name) {
            return Err(format_err!("unsupported"));
        }

        let mut manager_stream =
            ServerEnd::<fidl_fuchsia_ui_brightness::ControlMarker>::new(channel).into_stream();

        let auto_brightness_handle = self.auto_brightness.clone();
        let brightness_handle = self.manual_brightness.clone();
        let num_changes_handle = self.num_changes.clone();

        fasync::Task::local(async move {
            while let Some(req) = manager_stream.try_next().await.unwrap() {
                match req {
                    fidl_fuchsia_ui_brightness::ControlRequest::WatchCurrentBrightness {
                        responder,
                    } => {
                        responder
                            .send(brightness_handle.lock().await.expect("brightness not yet set"))
                            .unwrap();
                    }
                    fidl_fuchsia_ui_brightness::ControlRequest::SetAutoBrightness {
                        control_handle: _,
                    } => {
                        *auto_brightness_handle.lock().await = Some(true);
                        *num_changes_handle.lock().await += 1;
                    }
                    fidl_fuchsia_ui_brightness::ControlRequest::SetManualBrightness {
                        value,
                        control_handle: _,
                    } => {
                        *brightness_handle.lock().await = Some(value);
                        *num_changes_handle.lock().await += 1;
                    }
                    fidl_fuchsia_ui_brightness::ControlRequest::WatchAutoBrightness {
                        responder,
                    } => {
                        responder
                            .send(auto_brightness_handle.lock().await.unwrap_or(false))
                            .unwrap();
                    }
                    _ => {}
                }
            }
        })
        .detach();

        Ok(())
    }
}
