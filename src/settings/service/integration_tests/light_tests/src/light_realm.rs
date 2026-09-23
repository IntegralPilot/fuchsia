// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_hardware_light::{
    Capability, Info as HardwareInfo, LightError, LightRequest, LightRequestStream,
    LightServiceMarker, Rgb,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_settings::{LightGroup, LightMarker, LightProxy, LightValue};
use fidl_fuchsia_ui_policy::{
    DeviceListenerRegistryMarker, DeviceListenerRegistryRequest,
    DeviceListenerRegistryRequestStream, MediaButtonsListenerProxy,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability as ComponentCapability, ChildOptions, LocalComponentHandles, RealmBuilder,
    RealmInstance, Route,
};
use futures::channel::mpsc::Sender;
use futures::lock::Mutex;
use futures::{FutureExt, StreamExt, TryStreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use vfs::{pseudo_directory, service};

#[derive(Clone, Debug)]
pub struct HardwareLight {
    pub name: String,
    pub value: LightValue,
}

const COMPONENT_URL: &str = "#meta/setui_service.cm";

pub struct LightRealm;

impl LightRealm {
    pub async fn create_realm(
        hardware_lights: impl Into<Option<Vec<HardwareLight>>>,
    ) -> Result<RealmInstance, Error> {
        Self::create_realm_with_input_device_registry(hardware_lights, None).await
    }

    pub async fn create_realm_with_input_device_registry(
        hardware_lights: impl Into<Option<Vec<HardwareLight>>>,
        listener_sender: impl Into<Option<Sender<MediaButtonsListenerProxy>>>,
    ) -> Result<RealmInstance, Error> {
        let hardware_lights = hardware_lights.into().unwrap_or_default();
        let builder = RealmBuilder::new().await?;
        // Add setui_service as child of the realm builder.
        let setui_service =
            builder.add_child("setui_service", COMPONENT_URL, ChildOptions::new()).await?;
        let light_service = builder
            .add_local_child(
                "light-service",
                move |handles| {
                    Box::pin(Self::serve_light_service(handles, hardware_lights.clone()))
                },
                ChildOptions::new().eager(),
            )
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(ComponentCapability::service::<LightServiceMarker>())
                    .from(&light_service)
                    .to(&setui_service),
            )
            .await?;
        if let Some(listener_sender) = listener_sender.into() {
            let input_device_registry = builder
                .add_local_child(
                    "input-device-registry",
                    move |handles| {
                        Self::input_device_registry_service(handles, listener_sender.clone())
                            .boxed()
                    },
                    ChildOptions::new().eager(),
                )
                .await?;
            builder
                .add_route(
                    Route::new()
                        .capability(ComponentCapability::protocol::<DeviceListenerRegistryMarker>())
                        .from(&input_device_registry)
                        .to(&setui_service),
                )
                .await?;
        }
        let info = utils::SettingsRealmInfo {
            builder,
            settings: &setui_service,
            has_config_data: true,
            capabilities: vec![LightMarker::PROTOCOL_NAME],
        };
        // Add basic Settings service realm information.
        utils::create_realm_basic(&info).await?;
        let instance = info.builder.build().await?;
        Ok(instance)
    }

    pub fn connect_to_light_marker(instance: &RealmInstance) -> LightProxy {
        return instance.root.connect_to_protocol_at_exposed_dir().expect("connecting to Light");
    }

    async fn input_device_registry_service(
        handles: LocalComponentHandles,
        listener_sender: Sender<MediaButtonsListenerProxy>,
    ) -> Result<(), Error> {
        let mut fs = ServiceFs::new();
        let _ = fs.dir("svc").add_fidl_service(
            move |mut stream: DeviceListenerRegistryRequestStream| {
                fasync::Task::spawn({
                    let mut listener_sender = listener_sender.clone();
                    async move {
                        while let Ok(Some(request)) = stream.try_next().await {
                            match request {
                                DeviceListenerRegistryRequest::RegisterListener {
                                    listener,
                                    responder,
                                } => {
                                    let proxy = listener.into_proxy();
                                    listener_sender.try_send(proxy).expect("test should listen");
                                    // Acknowledge the registration.
                                    responder.send().expect("failed to ack RegisterListener call");
                                }
                                _ => {
                                    panic!("Unsupported request {request:?}")
                                }
                            }
                        }
                    }
                })
                .detach()
            },
        );

        let _ = fs.serve_connection(handles.outgoing_dir)?;
        fs.collect::<()>().await;
        Ok(())
    }

    async fn hardware_light_service(
        mut stream: LightRequestStream,
        hardware_lights: Vec<HardwareLight>,
    ) {
        let mut light_info = vec![];
        let mut simple_values = HashMap::new();
        let mut brightness_values = HashMap::new();
        let mut rgb_values = HashMap::new();
        for (i, light) in hardware_lights.iter().enumerate() {
            let capability = match light.value {
                LightValue::On(value) => {
                    let _ = simple_values.insert(i, value);
                    Capability::Simple
                }
                LightValue::Brightness(value) => {
                    let _ = brightness_values.insert(i, value);
                    Capability::Brightness
                }
                LightValue::Color(value) => {
                    let _ = rgb_values.insert(
                        i,
                        Rgb {
                            red: value.red as f64,
                            green: value.green as f64,
                            blue: value.blue as f64,
                        },
                    );
                    Capability::Rgb
                }
            };

            light_info.push(HardwareInfo { name: light.name.clone(), capability });
        }

        let light_info = Arc::new(Mutex::new(light_info));
        let simple_values = Arc::new(Mutex::new(simple_values));
        let brightness_values = Arc::new(Mutex::new(brightness_values));
        let rgb_values = Arc::new(Mutex::new(rgb_values));

        fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                // Support future expansion of FIDL.
                #[allow(unreachable_patterns)]
                match req {
                    LightRequest::GetNumLights { responder } => responder
                        .send(light_info.lock().await.len() as u32)
                        .expect("get num lights"),
                    LightRequest::GetInfo { index, responder } => responder
                        .send(
                            light_info
                                .lock()
                                .await
                                .get(index as usize)
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("get info"),
                    LightRequest::GetCurrentBrightnessValue { index, responder } => responder
                        .send(
                            brightness_values
                                .lock()
                                .await
                                .get(&(index as usize))
                                .copied()
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("get brightness value"),
                    LightRequest::GetCurrentSimpleValue { index, responder } => responder
                        .send(
                            simple_values
                                .lock()
                                .await
                                .get(&(index as usize))
                                .copied()
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("get simple value"),
                    LightRequest::GetCurrentRgbValue { index, responder } => responder
                        .send(
                            rgb_values
                                .lock()
                                .await
                                .get(&(index as usize))
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("get rgb value"),
                    LightRequest::SetBrightnessValue { index, value, responder } => responder
                        .send(
                            brightness_values
                                .lock()
                                .await
                                .insert(index as usize, value)
                                .map(|_| ())
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("set brightness value"),
                    LightRequest::SetSimpleValue { index, value, responder } => responder
                        .send(
                            simple_values
                                .lock()
                                .await
                                .insert(index as usize, value)
                                .map(|_| ())
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("set simple value"),
                    LightRequest::SetRgbValue { index, value, responder } => responder
                        .send(
                            rgb_values
                                .lock()
                                .await
                                .insert(index as usize, value)
                                .map(|_| ())
                                .ok_or(LightError::InvalidIndex),
                        )
                        .expect("set rgb value"),
                    _ => {}
                }
            }
        })
        .detach();
    }

    async fn serve_light_service(
        handles: LocalComponentHandles,
        hardware_lights: Vec<HardwareLight>,
    ) -> Result<(), Error> {
        let dir = pseudo_directory! {
            "fuchsia.hardware.light.LightService" => pseudo_directory! {
                "default" => pseudo_directory! {
                    "light" => service::host(
                        move |stream: LightRequestStream| {
                            Self::hardware_light_service(stream, hardware_lights.clone())
                        }
                    ),
                }
            }
        };
        let mut fs = ServiceFs::new();
        let _ = fs.add_remote(
            "svc",
            vfs::directory::serve(
                dir,
                vfs::execution_scope::ExecutionScope::new(),
                fio::PERM_READABLE | fio::PERM_WRITABLE,
            ),
        );
        let _ = fs.serve_connection(handles.outgoing_dir).expect("failed to serve outgoing");
        fs.collect::<()>().await;
        Ok(())
    }
}

/// Compares a vector of light group from the settings FIDL API and the light groups from a
/// service-internal LightInfo object for equality.
#[macro_export]
macro_rules! assert_lights_eq {
    ($groups:expr, $info:expr) => {
        let mut groups = $groups;
        // Watch returns vector, internally we use a HashMap, so convert into a vector for
        // comparison.
        let mut expected_value = $info
            .into_iter()
            .map(|(_, value)| ::fidl_fuchsia_settings::LightGroup::from(value))
            .collect::<Vec<_>>();

        assert_eq!(groups.len(), expected_value.len(), "lights length mismatch");
        // Sort by names for stability
        groups.sort_by_key(|group: &::fidl_fuchsia_settings::LightGroup| group.name.clone());
        expected_value.sort_by_key(|group| group.name.clone());
        for i in 0..groups.len() {
            $crate::assert_fidl_light_group_eq!(&groups[i], &expected_value[i]);
        }
    };
}

#[macro_export]
macro_rules! assert_fidl_light_group_eq {
    ($left:expr, $right:expr) => {
        let left = $left;
        let right = $right;
        assert_eq!(left.name, right.name, "name mismatch");
        assert_eq!(left.enabled, right.enabled, "enabled mismatch");
        assert_eq!(left.type_, right.type_, "type mismatch");
        assert_eq!(
            left.lights.as_ref().unwrap().len(),
            right.lights.as_ref().unwrap().len(),
            "group length mismatch"
        );
        if left.type_ == Some(::fidl_fuchsia_settings::LightType::Simple) {
            assert_eq!(left.lights, right.lights);
        }
    };
}

pub fn check_fidl_light_group_eq(left: &LightGroup, right: &LightGroup) -> bool {
    left.name == right.name
        && left.enabled == right.enabled
        && left.type_ == right.type_
        && left.lights.as_ref().unwrap().len() == right.lights.as_ref().unwrap().len()
        && if left.type_ == Some(::fidl_fuchsia_settings::LightType::Simple) {
            left.lights == right.lights
        } else {
            true
        }
}
