// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::light_fidl_handler::{GroupPublisher, InfoPublisher};
use crate::light_hardware_configuration::{DisableConditions, LightHardwareConfiguration};
use crate::types::{LightGroup, LightInfo, LightState, LightType, LightValue};
use anyhow::{Context, Error};
use fidl_fuchsia_hardware_light::{Info, LightMarker, LightProxy};
use fidl_fuchsia_settings_storage::LightGroups;
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use futures::lock::Mutex;
use settings_common::call_async;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::event::{
    ExternalEventPublisher, ResponseType, SettingValuePublisher,
};
use settings_common::service_context::{ExternalServiceProxy, ServiceContext};
use settings_media_buttons::{Event, MediaButtons};
use settings_storage::UpdateState;
use settings_storage::fidl_storage::{FidlStorage, FidlStorageConvertible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::rc::Rc;

/// Used as the argument field in a LightError::InvalidArgument to signal the FIDL handler to
/// signal that a fidl LightError::INVALID_NAME should be returned to the client.
pub(crate) const ARG_NAME: &str = "name";

/// Service path used to connect to light devices.
pub(crate) const SERVICE_PATH: &str = "/svc/fuchsia.hardware.light.LightService/*/light";

impl FidlStorageConvertible for LightInfo {
    type Storable = LightGroups;
    type Loader = NoneT;
    const KEY: &'static str = "light_info";

    #[allow(clippy::redundant_closure)]
    fn to_storable(self) -> Self::Storable {
        LightGroups {
            groups: self
                .light_groups
                .into_values()
                .map(|group| fidl_fuchsia_settings::LightGroup::from(group))
                .collect(),
        }
    }

    fn from_storable(storable: Self::Storable) -> Self {
        // Unwrap ok since validation would ensure non-None name before writing to storage.
        let light_groups = storable
            .groups
            .into_iter()
            .map(|group| (group.name.clone().unwrap(), group.into()))
            .collect();
        Self { light_groups }
    }
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum LightError {
    #[error("Invalid input argument for Light setting: argument:{0:?} value:{1:?}")]
    InvalidArgument(&'static str, String),
    #[error(
        "Call to an external dependency {0:?} for Light setting failed. \
         Request:{1:?}: Error:{2}"
    )]
    ExternalFailure(&'static str, Cow<'static, str>, Cow<'static, str>),
    #[error("Write failed for Light setting: {0:?}")]
    WriteFailure(Error),
    #[error("Unexpected error: {0:?}")]
    UnexpectedError(&'static str),
}

impl From<&LightError> for ResponseType {
    fn from(error: &LightError) -> Self {
        match error {
            LightError::InvalidArgument(..) => ResponseType::InvalidArgument,
            LightError::ExternalFailure(..) => ResponseType::ExternalFailure,
            LightError::WriteFailure(..) => ResponseType::StorageFailure,
            LightError::UnexpectedError(..) => ResponseType::UnexpectedError,
        }
    }
}

pub struct LightController {
    /// Proxy for interacting with light hardware.
    light_proxy: ExternalServiceProxy<LightProxy, ExternalEventPublisher>,

    /// inspect_event_tx.clone() configuration that determines what lights to return to the client.
    ///
    /// If present, overrides the lights from the underlying fuchsia.hardware.light API.
    light_hardware_config: Option<LightHardwareConfiguration>,

    /// Cache of data that includes hardware values. The data stored on disk does not persist the
    /// hardware values, so restoring does not bring the values back into memory. The data needs to
    /// be cached at this layer so we don't lose track of them.
    data_cache: Rc<Mutex<Option<LightInfo>>>,

    /// Disk storage for light setting. Stores in fidl format.
    store: Rc<FidlStorage>,

    /// HangingGet publisher for WatchLightGroups fidl api.
    publisher: Option<InfoPublisher>,

    /// HangingGet publisher for WatchLightGroup (singular) fidl api.
    group_publishers: HashMap<String, GroupPublisher>,

    /// Publisher for updates to the setting value.
    setting_value_publisher: SettingValuePublisher<LightInfo>,
}

pub(crate) enum Request {
    SetLightGroupValue(String, Vec<LightState>, Sender<Result<(), LightError>>),
}

impl StorageAccess for LightController {
    type Storage = FidlStorage;
    type Data = LightInfo;
    const STORAGE_KEY: &'static str = LightInfo::KEY;
}

/// Controller for processing requests surrounding the Light protocol.
impl LightController {
    pub(crate) async fn new<F>(
        service_context: Rc<ServiceContext>,
        default_setting: &mut DefaultSetting<LightHardwareConfiguration, &'static str>,
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<LightInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Result<Self, Error>
    where
        F: StorageFactory<Storage = FidlStorage>,
    {
        let light_hardware_config =
            default_setting.load_default_value().context("loading default value")?;

        LightController::create_with_config(
            service_context,
            light_hardware_config,
            &*storage_factory,
            setting_value_publisher,
            external_publisher,
        )
        .await
    }

    /// Alternate constructor that allows specifying a configuration.
    async fn create_with_config<F>(
        service_context: Rc<ServiceContext>,
        light_hardware_config: Option<LightHardwareConfiguration>,
        storage_factory: &F,
        setting_value_publisher: SettingValuePublisher<LightInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Result<Self, Error>
    where
        F: StorageFactory<Storage = FidlStorage>,
    {
        let light_proxy = service_context
            .connect_device_path::<LightMarker, _>(SERVICE_PATH, external_publisher)
            .await
            .context("connecting to fuchsia.hardware.light service")?;

        Ok(LightController {
            light_proxy,
            light_hardware_config,
            data_cache: Rc::new(Mutex::new(None)),
            store: storage_factory.get_store().await,
            publisher: None,
            group_publishers: HashMap::new(),
            setting_value_publisher,
        })
    }

    pub(crate) fn register_publishers(
        &mut self,
        publisher: InfoPublisher,
        group_publishers: HashMap<String, GroupPublisher>,
    ) {
        self.publisher = Some(publisher);
        self.group_publishers = group_publishers;
    }

    fn publish(&self, info: LightInfo) {
        let _ = self.setting_value_publisher.publish(&info);
        let pg = info.light_groups.iter().filter_map(|(key, group)| {
            self.group_publishers.get(key).map(|publisher| (publisher, group))
        });
        for (publisher, group) in pg {
            publisher.update(|old_group| {
                let Some(old_group) = old_group.as_mut() else {
                    *old_group = Some(group.clone());
                    return true;
                };

                if *old_group != *group {
                    *old_group = group.clone();
                    return true;
                }
                false
            });
        }

        if let Some(publisher) = self.publisher.as_ref() {
            publisher.set(info);
        }
    }

    pub(crate) async fn handle(
        self,
        mut event_rx: UnboundedReceiver<(Event, super::ResultSender)>,
        mut request_rx: UnboundedReceiver<Request>,
    ) -> Result<fasync::Task<()>, LightError> {
        Ok(fasync::Task::local(async move {
            let mut next_event = event_rx.next();
            let mut next_request = request_rx.next();
            'request: loop {
                futures::select! {
                    event = next_event => {
                        let Some((Event::OnButton(media_buttons), response_tx)) = event else {
                            continue;
                        };
                        next_event = event_rx.next();
                        let res = if let MediaButtons { mic_mute: Some(mic_mute), .. } = media_buttons {
                            self.on_mic_mute(mic_mute).await.map(|res| res.map(|info| self.publish(info)))
                        } else {
                            Ok(None)
                        };
                        let _ = response_tx.send(res);
                    }
                    request = next_request => {
                        let Some(Request::SetLightGroupValue(name, state, tx)) = request else {
                            continue;
                        };
                        next_request = request_rx.next();
                        // Validate state contains valid float numbers.
                        for light_state in &state {
                            if !light_state.is_finite() {
                                let _ = tx.send(Err(LightError::InvalidArgument(
                                    "state",
                                    format!("{light_state:?}"),
                                )));
                                continue 'request;
                            }
                        }

                        match self.set(name, state).await {
                            Ok(info) => {
                                if let Some(info) = info {
                                    self.publish(info);
                                }
                                let _ = tx.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                            }
                        }
                    }
                }
            }
        }))
    }

    async fn set(
        &self,
        name: String,
        state: Vec<LightState>,
    ) -> Result<Option<LightInfo>, LightError> {
        let mut light_info = self.data_cache.lock().await;
        // TODO(https://fxbug.dev/42058901) Deduplicate the code here and in mic_mute if possible.
        if light_info.is_none() {
            drop(light_info);
            let _ = self.restore().await?;
            light_info = self.data_cache.lock().await;
        }

        let current =
            light_info.as_mut().ok_or_else(|| LightError::UnexpectedError("missing data cache"))?;
        let mut entry = match current.light_groups.entry(name.clone()) {
            Entry::Vacant(_) => {
                // Reject sets if the light name is not known.
                return Err(LightError::InvalidArgument(ARG_NAME, name));
            }
            Entry::Occupied(entry) => entry,
        };

        let group = entry.get_mut();

        if state.len() != group.lights.len() {
            // If the number of light states provided doesn't match the number of lights,
            // return an error.
            return Err(LightError::InvalidArgument("state", format!("{state:?}")));
        }

        if !state.iter().filter_map(|state| state.value.clone()).all(|value| {
            match group.light_type {
                LightType::Brightness => matches!(value, LightValue::Brightness(_)),
                LightType::Rgb => matches!(value, LightValue::Rgb(_)),
                LightType::Simple => matches!(value, LightValue::Simple(_)),
            }
        }) {
            // If not all the light values match the light type of this light group, return an
            // error.
            return Err(LightError::InvalidArgument("state", format!("{state:?}")));
        }

        // After the main validations, write the state to the hardware.
        self.write_light_group_to_hardware(group, &state).await?;

        self.store
            .write(current.clone())
            .await
            .map(|state| match state {
                UpdateState::Unchanged => None,
                UpdateState::Updated => Some(current.clone()),
            })
            .map_err(|e| LightError::WriteFailure(e.context("writing light on set")))
    }

    /// Writes the given list of light states for a light group to the actual hardware.
    ///
    /// [LightState::None] elements in the vector are ignored and not written to the hardware.
    async fn write_light_group_to_hardware(
        &self,
        group: &mut LightGroup,
        state: &[LightState],
    ) -> Result<(), LightError> {
        for (i, (light, hardware_index)) in
            state.iter().zip(group.hardware_index.iter()).enumerate()
        {
            let (set_result, method_name) = match light.clone().value {
                // No value provided for this index, just skip it and don't update the
                // stored value.
                None => continue,
                Some(LightValue::Brightness(brightness)) => (
                    call_async!(self.light_proxy =>
                        set_brightness_value(*hardware_index, brightness))
                    .await,
                    "set_brightness_value",
                ),
                Some(LightValue::Rgb(rgb)) => {
                    let value = rgb
                        .clone()
                        .try_into()
                        .map_err(|_| LightError::InvalidArgument("value", format!("{rgb:?}")))?;
                    (
                        call_async!(self.light_proxy =>
                            set_rgb_value(*hardware_index, & value))
                        .await,
                        "set_rgb_value",
                    )
                }
                Some(LightValue::Simple(on)) => (
                    call_async!(self.light_proxy => set_simple_value(*hardware_index, on)).await,
                    "set_simple_value",
                ),
            };
            set_result
                .map_err(|e| format!("{e:?}"))
                .and_then(|res| res.map_err(|e| format!("{e:?}")))
                .map_err(|e| {
                    LightError::ExternalFailure(
                        "fuchsia.hardware.light",
                        Cow::Owned(format!("{method_name} for light {hardware_index}")),
                        Cow::Owned(e),
                    )
                })?;

            // Set was successful, save this light value.
            group.lights[i] = light.clone();
        }
        Ok(())
    }

    async fn on_mic_mute(&self, mic_mute: bool) -> Result<Option<LightInfo>, LightError> {
        let mut light_info = self.data_cache.lock().await;
        if light_info.is_none() {
            drop(light_info);
            let _ = self.restore().await?;
            light_info = self.data_cache.lock().await;
        }

        let current =
            light_info.as_mut().ok_or_else(|| LightError::UnexpectedError("missing data cache"))?;
        for light in current
            .light_groups
            .values_mut()
            .filter(|l| l.disable_conditions.contains(&DisableConditions::MicSwitch))
        {
            // This condition means that the LED is hard-wired to the mute switch and will only be
            // on when the mic is disabled.
            light.enabled = mic_mute;
        }

        self.store
            .write(current.clone())
            .await
            .map(|state| match state {
                UpdateState::Unchanged => None,
                UpdateState::Updated => Some(current.clone()),
            })
            .map_err(|e| LightError::WriteFailure(e.context("writing light on mic mute")))
    }

    pub(crate) async fn restore(&self) -> Result<LightInfo, LightError> {
        let light_info = if let Some(config) = self.light_hardware_config.clone() {
            // Configuration is specified, restore from the configuration.
            self.restore_from_configuration(config).await
        } else {
            // Read light info from hardware.
            self.restore_from_hardware().await
        }?;
        let mut guard = self.data_cache.lock().await;
        *guard = Some(light_info.clone());
        Ok(light_info)
    }

    /// Restores the light information from a pre-defined hardware configuration. Individual light
    /// states are read from the underlying fuchsia.hardware.Light API, but the structure of the
    /// light groups is determined by the given `config`.
    async fn restore_from_configuration(
        &self,
        config: LightHardwareConfiguration,
    ) -> Result<LightInfo, LightError> {
        let current = self.store.get::<LightInfo>().await;
        let mut light_groups: HashMap<String, LightGroup> = HashMap::new();
        for group_config in config.light_groups {
            let mut light_state: Vec<LightState> = Vec::new();

            // TODO(https://fxbug.dev/42134045): once all clients go through setui, restore state from hardware
            // only if not found in persistent storage.
            for light_index in group_config.hardware_index.iter() {
                light_state.push(
                    self.light_state_from_hardware_index(*light_index, group_config.light_type)
                        .await?,
                );
            }

            // Restore previous state.
            let enabled = current
                .light_groups
                .get(&group_config.name)
                .map(|found_group| found_group.enabled)
                .unwrap_or(true);

            let _ = light_groups.insert(
                group_config.name.clone(),
                LightGroup {
                    name: group_config.name,
                    enabled,
                    light_type: group_config.light_type,
                    lights: light_state,
                    hardware_index: group_config.hardware_index,
                    disable_conditions: group_config.disable_conditions,
                },
            );
        }

        Ok(LightInfo { light_groups })
    }

    /// Restores the light information when no hardware configuration is specified by reading from
    /// the underlying fuchsia.hardware.Light API and turning each light into a [`LightGroup`].
    ///
    /// [`LightGroup`]: ../../light/types/struct.LightGroup.html
    async fn restore_from_hardware(&self) -> Result<LightInfo, LightError> {
        let num_lights = call_async!(self.light_proxy => get_num_lights()).await.map_err(|e| {
            LightError::ExternalFailure(
                "fuchsia.hardware.light",
                Cow::Borrowed("get_num_lights"),
                Cow::Owned(format!("{e:?}")),
            )
        })?;

        let mut current = self.store.get::<LightInfo>().await;
        for i in 0..num_lights {
            let info = call_async!(self.light_proxy => get_info(i))
                .await
                .map_err(|e| format!("{e:?}"))
                .and_then(|res| res.map_err(|e| format!("{e:?}")))
                .map_err(|e| {
                    LightError::ExternalFailure(
                        "fuchsia.hardware.light",
                        Cow::Owned(format!("get_info for light {i}")),
                        Cow::Owned(e),
                    )
                })?;
            let (name, group) = self.light_info_to_group(i, info).await?;
            let _ = current.light_groups.insert(name, group);
        }

        Ok(current)
    }

    /// Converts an Info object from the fuchsia.hardware.Light API into a LightGroup, the internal
    /// representation used for our service.
    async fn light_info_to_group(
        &self,
        index: u32,
        info: Info,
    ) -> Result<(String, LightGroup), LightError> {
        let light_type: LightType = info.capability.into();

        let light_state = self.light_state_from_hardware_index(index, light_type).await?;

        Ok((
            info.name.clone(),
            LightGroup {
                name: info.name,
                // When there's no config, lights are assumed to be always enabled.
                enabled: true,
                light_type,
                lights: vec![light_state],
                hardware_index: vec![index],
                disable_conditions: vec![],
            },
        ))
    }

    /// Reads light state from the underlying fuchsia.hardware.Light API for the given hardware
    /// index and light type.
    async fn light_state_from_hardware_index(
        &self,
        index: u32,
        light_type: LightType,
    ) -> Result<LightState, LightError> {
        // Read the proper value depending on the light type.
        let value = match light_type {
            LightType::Brightness => {
                call_async!(self.light_proxy => get_current_brightness_value(index))
                    .await
                    .map_err(|e| format!("{e:?}"))
                    .and_then(|res| res.map_err(|e| format!("{e:?}")))
                    .map(LightValue::Brightness)
                    .map_err(|e| {
                        LightError::ExternalFailure(
                            "fuchsia.hardware.light",
                            Cow::Owned(format!("get_current_brightness_value for light {index}")),
                            Cow::Owned(e),
                        )
                    })?
            }
            LightType::Rgb => call_async!(self.light_proxy => get_current_rgb_value(index))
                .await
                .map_err(|e| format!("{e:?}"))
                .and_then(|res| res.map_err(|e| format!("{e:?}")))
                .map(LightValue::from)
                .map_err(|e| {
                    LightError::ExternalFailure(
                        "fuchsia.hardware.light",
                        Cow::Owned(format!("get_current_rgb_value for light {index}")),
                        Cow::Owned(e),
                    )
                })?,
            LightType::Simple => call_async!(self.light_proxy => get_current_simple_value(index))
                .await
                .map_err(|e| format!("{e:?}"))
                .and_then(|res| res.map_err(|e| format!("{e:?}")))
                .map(LightValue::Simple)
                .map_err(|e| {
                    LightError::ExternalFailure(
                        "fuchsia.hardware.light",
                        Cow::Owned(format!("get_current_simple_value for light {index}")),
                        Cow::Owned(e),
                    )
                })?,
        };

        Ok(LightState { value: Some(value) })
    }
}

#[cfg(test)]
mod tests {
    use futures::channel::mpsc;

    use super::*;
    use crate::light_fidl_handler::LightFidlHandler;
    use crate::test_fakes::hardware_light_service::HardwareLightService;
    use settings_test_common::fakes::service::ServiceRegistry;
    use settings_test_common::storage::InMemoryFidlStorageFactory;

    // Verify that a set call without a restore call succeeds. This can happen when the controller
    // is shutdown after inactivity and is brought up again to handle the set call.
    #[fuchsia::test()]
    async fn test_set_before_restore() {
        // Create a fake hardware light service that responds to FIDL calls and add it to the
        // service registry so that FIDL calls are routed to this fake service.
        let service_registry = ServiceRegistry::create();
        let light_service_handle = Rc::new(Mutex::new(HardwareLightService::new()));
        service_registry.lock().await.register_service(light_service_handle.clone());

        let service_context = ServiceContext::new(Some(ServiceRegistry::serve(service_registry)));

        // Add a light to the fake service.
        light_service_handle
            .lock()
            .await
            .insert_light(0, "light_1".to_string(), LightType::Simple, LightValue::Simple(false))
            .await;

        let storage_factory = InMemoryFidlStorageFactory::new();
        storage_factory.initialize_storage::<LightInfo>().await;

        let (tx, _rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(tx);
        let (tx, _rx) = mpsc::unbounded();
        let event_publisher = ExternalEventPublisher::new(tx);

        // Create the light controller.
        let mut light_controller = LightController::create_with_config(
            Rc::new(service_context),
            None,
            &storage_factory,
            setting_value_publisher,
            event_publisher,
        )
        .await
        .expect("Failed to create light controller");
        let info = light_controller.restore().await.unwrap();
        let (info_hanging_get, group_hanging_gets) = LightFidlHandler::build_hanging_gets(info);
        light_controller.register_publishers(
            info_hanging_get.new_publisher(),
            group_hanging_gets
                .iter()
                .map(|(key, hanging_get)| (key.clone(), hanging_get.new_publisher()))
                .collect(),
        );

        // Call set and verify it succeeds.
        let _ = light_controller
            .set("light_1".to_string(), vec![LightState { value: Some(LightValue::Simple(true)) }])
            .await
            .expect("Set call failed");

        // Verify the data cache is populated after the set call.
        let _ =
            light_controller.data_cache.lock().await.as_ref().expect("Data cache is not populated");
    }

    // Verify that an on_mic_mute event without a restore call succeeds. This can happen when the
    // controller is shutdown after inactivity and is brought up again to handle the set call.
    #[fuchsia::test()]
    async fn test_on_mic_mute_before_restore() {
        // Create a fake hardware light service that responds to FIDL calls and add it to the
        // service registry so that FIDL calls are routed to this fake service.
        let service_registry = ServiceRegistry::create();
        let light_service_handle = Rc::new(Mutex::new(HardwareLightService::new()));
        service_registry.lock().await.register_service(light_service_handle.clone());

        let service_context = ServiceContext::new(Some(ServiceRegistry::serve(service_registry)));

        // Add a light to the fake service.
        light_service_handle
            .lock()
            .await
            .insert_light(0, "light_1".to_string(), LightType::Simple, LightValue::Simple(false))
            .await;

        let storage_factory = InMemoryFidlStorageFactory::new();
        storage_factory.initialize_storage::<LightInfo>().await;

        let (tx, _rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(tx);
        let (tx, _rx) = mpsc::unbounded();
        let event_publisher = ExternalEventPublisher::new(tx);

        // Create the light controller.
        let mut light_controller = LightController::create_with_config(
            Rc::new(service_context),
            None,
            &storage_factory,
            setting_value_publisher,
            event_publisher,
        )
        .await
        .expect("Failed to create light controller");
        let info = light_controller.restore().await.unwrap();
        let (info_hanging_get, group_hanging_gets) = LightFidlHandler::build_hanging_gets(info);
        light_controller.register_publishers(
            info_hanging_get.new_publisher(),
            group_hanging_gets
                .iter()
                .map(|(key, hanging_get)| (key.clone(), hanging_get.new_publisher()))
                .collect(),
        );

        // Call on_mic_mute and verify it succeeds.
        let _ = light_controller.on_mic_mute(false).await.expect("Set call failed");

        // Verify the data cache is populated after the set call.
        let _ =
            light_controller.data_cache.lock().await.as_ref().expect("Data cache is not populated");
    }
}
