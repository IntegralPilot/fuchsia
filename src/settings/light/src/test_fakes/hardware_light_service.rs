// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fidl_fuchsia_hardware_light::{Info, LightMarker, LightRequest, Rgb};
use fuchsia_async as fasync;
use futures::lock::Mutex;
use futures::TryStreamExt;
use settings_test_common::fakes::service::Service;
use std::collections::HashMap;
use std::rc::Rc;
use zx::Channel;

use crate::types::{LightType, LightValue};

/// An implementation of fuchsia.hardware.light for testing use.
pub(crate) struct HardwareLightService {
    pub(crate) light_info: Rc<Mutex<HashMap<u32, Info>>>,
    pub(crate) simple_values: Rc<Mutex<HashMap<u32, bool>>>,
    pub(crate) brightness_values: Rc<Mutex<HashMap<u32, f64>>>,
    pub(crate) rgb_values: Rc<Mutex<HashMap<u32, Rgb>>>,
}

impl HardwareLightService {
    pub(crate) fn new() -> Self {
        Self {
            light_info: Rc::new(Mutex::new(HashMap::new())),
            simple_values: Rc::new(Mutex::new(HashMap::new())),
            brightness_values: Rc::new(Mutex::new(HashMap::new())),
            rgb_values: Rc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn insert_light(
        &self,
        index: u32,
        name: String,
        light_type: LightType,
        value: LightValue,
    ) {
        let _ = self
            .light_info
            .lock()
            .await
            .insert(index, Info { name, capability: light_type.into() });
        match value {
            LightValue::Brightness(value) => {
                let _ = self.brightness_values.lock().await.insert(index, value);
            }
            LightValue::Rgb(value) => {
                let _ = self
                    .rgb_values
                    .lock()
                    .await
                    .insert(index, value.try_into().expect("rgb conversion failed"));
            }
            LightValue::Simple(value) => {
                let _ = self.simple_values.lock().await.insert(index, value);
            }
        };
    }
}

impl Service for HardwareLightService {
    fn can_handle_service(&self, service_name: &str) -> bool {
        service_name == LightMarker::PROTOCOL_NAME
    }

    fn process_stream(&mut self, service_name: &str, channel: Channel) -> Result<(), Error> {
        if !self.can_handle_service(service_name) {
            return Err(format_err!("unsupported"));
        }

        let mut request_stream = ServerEnd::<LightMarker>::new(channel).into_stream();

        let light_info = self.light_info.clone();
        let simple_values = self.simple_values.clone();
        let brightness_values = self.brightness_values.clone();
        let rgb_values = self.rgb_values.clone();
        fasync::Task::local(async move {
            while let Some(req) = request_stream.try_next().await.unwrap() {
                match req {
                    LightRequest::GetNumLights { responder } => responder
                        .send(light_info.lock().await.len() as u32)
                        .expect("get num lights"),
                    LightRequest::GetInfo { index, responder } => responder
                        .send(Ok(light_info.lock().await.get(&index).expect("unknown light")))
                        .expect("get num lights"),
                    LightRequest::GetCurrentBrightnessValue { index, responder } => responder
                        .send(Ok(*brightness_values
                            .lock()
                            .await
                            .get(&index)
                            .expect("unknown light")))
                        .expect("get brightness value"),
                    LightRequest::GetCurrentSimpleValue { index, responder } => responder
                        .send(Ok(*simple_values.lock().await.get(&index).expect("unknown light")))
                        .expect("get simple value"),
                    LightRequest::GetCurrentRgbValue { index, responder } => responder
                        .send(Ok(rgb_values.lock().await.get(&index).expect("unknown light")))
                        .expect("get rgb value"),
                    LightRequest::SetBrightnessValue { index, value, responder } => {
                        let _ = brightness_values
                            .lock()
                            .await
                            .insert(index, value)
                            .expect("unknown light");
                        responder.send(Ok(())).expect("set brightness value")
                    }
                    LightRequest::SetSimpleValue { index, value, responder } => {
                        let _ =
                            simple_values.lock().await.insert(index, value).expect("unknown light");
                        responder.send(Ok(())).expect("set simple value")
                    }
                    LightRequest::SetRgbValue { index, value, responder } => {
                        let _ =
                            rgb_values.lock().await.insert(index, value).expect("unknown light");
                        responder.send(Ok(())).expect("set rgb value")
                    }
                    _ => {}
                }
            }
        })
        .detach();

        Ok(())
    }
}
