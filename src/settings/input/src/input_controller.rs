// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device_configuration::InputConfiguration;
use crate::input_fidl_handler::Publisher;
use crate::types::{
    DeviceState, DeviceStateSource, InputDevice, InputDeviceType, InputInfo, InputInfoSources,
    InputState, Microphone,
};
use anyhow::{Context, Error};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use serde::{Deserialize, Serialize};
use settings_camera::connect_to_camera;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::event::{
    ExternalEventPublisher, ResponseType, SettingValuePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_media_buttons::{Event, MediaButtons};
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::borrow::Cow;
use std::rc::Rc;

pub(crate) const DEFAULT_CAMERA_NAME: &str = "camera";
pub(crate) const DEFAULT_MIC_NAME: &str = "microphone";

// The MAX_INPUT_DEVICES, in conjunction with the FIDL API's constraint of 128-byte names, ensures
// that we don't exceed the 64 KiB default transport limit for the API (with some padding). It also
// lessens the chance that we OOM while loading the device list from disk to memory.
pub(crate) const MAX_INPUT_DEVICES: usize = fidl_fuchsia_settings::MAX_INPUT_DEVICES as usize;

type UpdateInputResult = Result<Option<InputInfo>, InputError>;
fn check_publish(
    result: UpdateInputResult,
    publish: impl Fn(InputInfo),
) -> Result<Option<()>, InputError> {
    result.map(|info| info.map(publish))
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum InputError {
    #[error("Failed to initialize controller: {0:?}")]
    InitFailure(Error),
    #[error("Unsupported device type: {0:?}")]
    Unsupported(InputDeviceType),
    #[error("External failure for Input dependency: {0:?} request:{1:?} error:{2}")]
    ExternalFailure(Cow<'static, str>, Cow<'static, str>, Cow<'static, str>),
    #[error("Write failed for Input: {0:?}")]
    WriteFailure(Error),
    #[error("The maximum number of input devices has been reached.")]
    MaximumInputDeviceLimitReached(Cow<'static, str>),
    #[error("Unexpected error: {0}")]
    UnexpectedError(Cow<'static, str>),
}

impl From<&InputError> for ResponseType {
    fn from(error: &InputError) -> Self {
        match error {
            InputError::InitFailure(..) => ResponseType::InitFailure,
            InputError::Unsupported(..) => ResponseType::UnsupportedError,
            InputError::ExternalFailure(..) => ResponseType::ExternalFailure,
            InputError::WriteFailure(..) => ResponseType::StorageFailure,
            InputError::MaximumInputDeviceLimitReached(..) => {
                ResponseType::MaximumInputDevicesReached
            }
            InputError::UnexpectedError(..) => ResponseType::UnexpectedError,
        }
    }
}

impl DeviceStorageCompatible for InputInfoSources {
    type Loader = NoneT;
    const KEY: &'static str = "input_info";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|e| {
            log::info!("Failed to deserialize InputInfoSources. Falling back to V2: {e:?}");
            InputInfoSourcesV2::try_deserialize_from(value).map(Self::from)
        })
    }
}

impl From<InputInfoSourcesV2> for InputInfoSources {
    fn from(v2: InputInfoSourcesV2) -> Self {
        let mut input_state = v2.input_device_state;

        // Convert the old states into an input device.
        input_state.set_source_state(
            InputDeviceType::MICROPHONE,
            DEFAULT_MIC_NAME.to_string(),
            DeviceStateSource::HARDWARE,
            if v2.hw_microphone.muted { DeviceState::MUTED } else { DeviceState::AVAILABLE },
        );
        input_state.set_source_state(
            InputDeviceType::MICROPHONE,
            DEFAULT_MIC_NAME.to_string(),
            DeviceStateSource::SOFTWARE,
            if v2.sw_microphone.muted { DeviceState::MUTED } else { DeviceState::AVAILABLE },
        );

        InputInfoSources { input_device_state: input_state }
    }
}

impl From<InputInfoSources> for InputInfo {
    fn from(info: InputInfoSources) -> InputInfo {
        InputInfo { input_device_state: info.input_device_state }
    }
}

#[derive(PartialEq, Default, Debug, Clone, Serialize, Deserialize)]
pub struct InputInfoSourcesV2 {
    hw_microphone: Microphone,
    sw_microphone: Microphone,
    input_device_state: InputState,
}

impl DeviceStorageCompatible for InputInfoSourcesV2 {
    type Loader = NoneT;
    const KEY: &'static str = "input_info_sources_v2";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|e| {
            log::info!("Failed to deserialize InputInfoSourcesV2. Falling back to V1: {e:?}");
            InputInfoSourcesV1::try_deserialize_from(value).map(Self::from)
        })
    }
}

impl From<InputInfoSourcesV1> for InputInfoSourcesV2 {
    fn from(v1: InputInfoSourcesV1) -> Self {
        InputInfoSourcesV2 {
            hw_microphone: v1.hw_microphone,
            sw_microphone: v1.sw_microphone,
            input_device_state: InputState::new(),
        }
    }
}

#[derive(PartialEq, Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InputInfoSourcesV1 {
    pub hw_microphone: Microphone,
    pub sw_microphone: Microphone,
}

impl DeviceStorageCompatible for InputInfoSourcesV1 {
    type Loader = NoneT;
    const KEY: &'static str = "input_info_sources_v1";
}

pub(crate) enum Request {
    Set(Vec<InputDevice>, Sender<Result<(), InputError>>),
}

pub struct InputController {
    service_context: Rc<ServiceContext>,
    /// Persistent storage.
    store: Rc<DeviceStorage>,

    /// Local tracking of the input device states.
    input_device_state: InputState,

    /// Configuration for this device.
    input_device_config: InputConfiguration,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<InputInfo>,
    external_publisher: ExternalEventPublisher,
}

impl StorageAccess for InputController {
    type Storage = DeviceStorage;
    type Data = InputInfoSources;
    const STORAGE_KEY: &'static str = InputInfoSources::KEY;
}

impl InputController {
    pub(super) async fn new<F>(
        service_context: Rc<ServiceContext>,
        default_setting: &mut DefaultSetting<InputConfiguration, &'static str>,
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<InputInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Result<Self, InputError>
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        let input_device_config = default_setting
            .load_default_value()
            .context("Unable to load input device config")
            .map_err(InputError::InitFailure)?
            .expect("Input requires a configuration");
        Ok(InputController::create_with_config(
            service_context,
            input_device_config,
            &*storage_factory,
            setting_value_publisher,
            external_publisher,
        )
        .await)
    }

    /// Alternate constructor that allows specifying a configuration.
    pub(crate) async fn create_with_config<F>(
        service_context: Rc<ServiceContext>,
        input_device_config: InputConfiguration,
        storage_factory: &F,
        setting_value_publisher: SettingValuePublisher<InputInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        Self {
            service_context,
            store: storage_factory.get_store().await,
            input_device_state: InputState::new(),
            input_device_config,
            publisher: None,
            setting_value_publisher,
            external_publisher,
        }
    }

    // Whether the configuration for this device contains a specific |device_type|.
    async fn has_input_device(&self, device_type: InputDeviceType) -> bool {
        let input_device_config_state: InputState = self.input_device_config.clone().into();
        input_device_config_state.device_types().contains(&device_type)
    }

    pub(super) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: InputInfo) {
        let _ = self.setting_value_publisher.publish(&info);
        if let Some(publisher) = self.publisher.as_ref() {
            publisher.set(info);
        }
    }

    pub(super) async fn handle(
        mut self,
        mut camera_event_rx: UnboundedReceiver<(bool, super::ResultSender)>,
        mut media_buttons_event_rx: UnboundedReceiver<(Event, super::ResultSender)>,
        mut request_rx: UnboundedReceiver<Request>,
    ) -> fasync::Task<()> {
        fasync::Task::local(async move {
            let mut next_camera_event = camera_event_rx.next();
            let mut next_media_buttons_event = media_buttons_event_rx.next();
            let mut next_request = request_rx.next();
            loop {
                futures::select! {
                    event = next_camera_event => {
                        let Some((is_muted, response_tx)) = event else {
                            continue;
                        };
                        next_camera_event = camera_event_rx.next();
                        let res = self.handle_camera_event(is_muted).await;
                        let _ = response_tx.send(res);
                    }
                    event = next_media_buttons_event => {
                        let Some((Event::OnButton(buttons), response_tx)) = event else {
                            continue;
                        };
                        next_media_buttons_event = media_buttons_event_rx.next();
                        let res = self.handle_media_buttons_event(buttons).await;
                        let _ = response_tx.send(res);
                    }
                    request = next_request => {
                        let Some(request) = request else {
                            continue;
                        };
                        next_request = request_rx.next();
                        let Request::Set(input_devices, tx) = request;
                        let res = check_publish(
                            self.set_input_states(input_devices, DeviceStateSource::SOFTWARE).await,
                            |info| self.publish(info)).map(|_|{});
                        let _ = tx.send(res);
                    }
                }
            }
        })
    }

    async fn handle_camera_event(&mut self, is_muted: bool) -> Result<Option<()>, InputError> {
        let stored_info = self.get_stored_info().await;
        let old_state = Self::get_cam_sw_state_from(&stored_info.input_device_state)?;
        if old_state.has_state(DeviceState::MUTED) != is_muted {
            check_publish(
                self.set_sw_camera_mute(is_muted, DEFAULT_CAMERA_NAME.to_string()).await,
                |info| self.publish(info),
            )
        } else {
            Ok(None)
        }
    }

    async fn handle_media_buttons_event(
        &mut self,
        mut buttons: MediaButtons,
    ) -> Result<Option<()>, InputError> {
        if buttons.mic_mute.is_some() && !self.has_input_device(InputDeviceType::MICROPHONE).await {
            buttons.set_mic_mute(None);
        }
        if buttons.camera_disable.is_some() && !self.has_input_device(InputDeviceType::CAMERA).await
        {
            buttons.set_camera_disable(None);
        }
        check_publish(self.set_hw_media_buttons_state(buttons).await, |info| self.publish(info))
    }

    // Wrapper around client.read() that fills in the config
    // as the default value if the read value is empty. It may be empty
    // after a migration from a previous InputInfoSources version
    // or on pave.
    async fn get_stored_info(&self) -> InputInfo {
        let mut input_info = InputInfo::from(self.store.get::<InputInfo>().await);
        if input_info.input_device_state.is_empty() {
            input_info.input_device_state = self.input_device_config.clone().into();
        }
        input_info
    }

    /// Restores the input state.
    pub(super) async fn restore(&mut self) -> Result<InputInfo, InputError> {
        let input_info = self.get_stored_info().await;
        self.input_device_state = input_info.input_device_state.clone();

        if self.input_device_config.devices.iter().any(|d| d.device_type == InputDeviceType::CAMERA)
        {
            match self.get_cam_sw_state() {
                Ok(state) => {
                    // Camera setup failure should not prevent start of service. This also allows
                    // clients to see that the camera may not be usable.
                    if let Err(e) = self.push_cam_sw_state(state).await {
                        log::error!("Unable to restore camera state: {e:?}");
                        self.set_cam_err_state(state);
                    }
                }
                Err(e) => {
                    log::error!("Unable to load cam sw state: {e:?}");
                    self.set_cam_err_state(DeviceState::ERROR);
                }
            }
        }
        Ok(input_info)
    }

    async fn set_sw_camera_mute(&mut self, disabled: bool, name: String) -> UpdateInputResult {
        let mut input_info = self.get_stored_info().await;
        input_info.input_device_state.set_source_state(
            InputDeviceType::CAMERA,
            name.clone(),
            DeviceStateSource::SOFTWARE,
            if disabled { DeviceState::MUTED } else { DeviceState::AVAILABLE },
        );

        self.input_device_state.set_source_state(
            InputDeviceType::CAMERA,
            name.clone(),
            DeviceStateSource::SOFTWARE,
            if disabled { DeviceState::MUTED } else { DeviceState::AVAILABLE },
        );
        self.store
            .write(&input_info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(input_info))
            .context("writing sw camera info")
            .map_err(InputError::WriteFailure)
    }

    /// Sets the hardware mic/cam state from the muted states in `media_buttons`.
    async fn set_hw_media_buttons_state(
        &mut self,
        media_buttons: MediaButtons,
    ) -> UpdateInputResult {
        let mut states_to_process = Vec::new();
        if let Some(mic_mute) = media_buttons.mic_mute {
            states_to_process.push((InputDeviceType::MICROPHONE, mic_mute));
        }
        if let Some(camera_disable) = media_buttons.camera_disable {
            states_to_process.push((InputDeviceType::CAMERA, camera_disable));
        }

        let mut input_info = self.get_stored_info().await;

        for (device_type, muted) in states_to_process.into_iter() {
            // Fetch current state.
            let hw_state_res = input_info.input_device_state.get_source_state(
                device_type,
                device_type.to_string(),
                DeviceStateSource::HARDWARE,
            );

            let mut hw_state = hw_state_res.map_err(|err| {
                InputError::UnexpectedError(
                    format!("Could not fetch current hw mute state: {err:?}").into(),
                )
            })?;

            if muted {
                // Unset available and set muted.
                hw_state &= !DeviceState::AVAILABLE;
                hw_state |= DeviceState::MUTED;
            } else {
                // Set available and unset muted.
                hw_state |= DeviceState::AVAILABLE;
                hw_state &= !DeviceState::MUTED;
            }

            // Set the updated state.
            input_info.input_device_state.set_source_state(
                device_type,
                device_type.to_string(),
                DeviceStateSource::HARDWARE,
                hw_state,
            );
            self.input_device_state.set_source_state(
                device_type,
                device_type.to_string(),
                DeviceStateSource::HARDWARE,
                hw_state,
            );
        }

        self.store
            .write(&input_info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(input_info))
            .context("writing hw media buttons")
            .map_err(InputError::WriteFailure)
    }

    /// Sets state for the given input devices.
    async fn set_input_states(
        &mut self,
        input_devices: Vec<InputDevice>,
        source: DeviceStateSource,
    ) -> UpdateInputResult {
        let mut input_info = self.get_stored_info().await;
        let device_types = input_info.input_device_state.device_types();
        let cam_state = self.get_cam_sw_state().ok();

        // Firstly, do a validation pass to make sure the input_devices contain only valid devices,
        // and any newly added devices are not going to exceed our maximum device limit.
        let mut new_devices = Vec::new();
        for input_device in input_devices.iter() {
            if !device_types.contains(&input_device.device_type) {
                return Err(InputError::Unsupported(input_device.device_type));
            }

            let already_exists = input_info
                .input_device_state
                .contains_device(input_device.device_type, &input_device.name);

            let already_counted = new_devices
                .iter()
                .any(|(dt, name)| *dt == input_device.device_type && *name == &input_device.name);

            if !already_exists && !already_counted {
                new_devices.push((input_device.device_type, &input_device.name));
            }
        }

        // Abort if applying these new devices would exceed the limit.
        if input_info.input_device_state.total_devices() + new_devices.len() > MAX_INPUT_DEVICES {
            log::error!(
                "Maximum number of supported input devices ({MAX_INPUT_DEVICES}) has been reached."
            );
            return Err(InputError::MaximumInputDeviceLimitReached(
                format!("Maximum limit of {MAX_INPUT_DEVICES} input devices has been reached.")
                    .into(),
            ));
        }

        // Update the function-scoped storage cache with the requested changes. This will allow us
        // to determine if any camera state has changed, necessary for the logic which follows.
        for input_device in &input_devices {
            input_info.input_device_state.insert_device(input_device.clone(), source);
        }

        // If the device has a camera, it should successfully get the sw state, and
        // push the state if it has changed. If the device does not have a camera,
        // it should be None both here and above, and thus not detect a change.
        let modified_cam_state = Self::get_cam_sw_state_from(&input_info.input_device_state).ok();
        if cam_state != modified_cam_state
            && let Some(state) = modified_cam_state
        {
            self.push_cam_sw_state(state).await?;
        }

        // All prerequisites are done, and camera changes are committed to the hardware. We can now
        // finalize the update by updating our cache, and queue the write to storage.
        for input_device in input_devices {
            self.input_device_state.insert_device(input_device, source);
        }

        self.store
            .write(&input_info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(input_info))
            .context("writing input states")
            .map_err(InputError::WriteFailure)
    }

    /// Pulls the software state of the camera from the given `input_state`.
    fn get_cam_sw_state_from(input_state: &InputState) -> Result<DeviceState, InputError> {
        input_state
            .get_source_state(
                InputDeviceType::CAMERA,
                DEFAULT_CAMERA_NAME.to_string(),
                DeviceStateSource::SOFTWARE,
            )
            .map_err(|e| {
                InputError::UnexpectedError(
                    format!("Could not find camera software state: {e:?}").into(),
                )
            })
    }

    /// Pulls the current software state of the camera from the device state.
    fn get_cam_sw_state(&self) -> Result<DeviceState, InputError> {
        Self::get_cam_sw_state_from(&self.input_device_state)
    }

    /// Set the camera state into an error condition.
    fn set_cam_err_state(&mut self, mut state: DeviceState) {
        state.set(DeviceState::ERROR, true);
        self.input_device_state.set_source_state(
            InputDeviceType::CAMERA,
            DEFAULT_CAMERA_NAME.to_string(),
            DeviceStateSource::SOFTWARE,
            state,
        )
    }

    /// Forwards the given software state to the camera3 api. Will first establish
    /// a connection to the camera3.DeviceWatcher api. This function should only be called
    /// when there is a camera included in the config. The config is used to populate the
    /// stored input_info, so the input_info's input_device_state can be checked whether its
    /// device_types contains Camera prior to calling this function.
    async fn push_cam_sw_state(&mut self, cam_state: DeviceState) -> Result<(), InputError> {
        let is_muted = cam_state.has_state(DeviceState::MUTED);

        // Start up a connection to the camera device watcher and connect to the
        // camera proxy using the id that is returned. The connection will drop out
        // of scope after the mute state is sent.
        let camera_proxy =
            connect_to_camera(&self.service_context, self.external_publisher.clone())
                .await
                .map_err(|e| {
                    InputError::UnexpectedError(
                        format!("Could not connect to camera device: {e:?}").into(),
                    )
                })?;

        camera_proxy.set_software_mute_state(is_muted).await.map_err(|e| {
            InputError::ExternalFailure(
                "fuchsia.camera3.Device".into(),
                "SetSoftwareMuteState".into(),
                format!("{e:?}").into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_device_configuration::{InputDeviceConfiguration, SourceState};
    use fuchsia_async as fasync;
    use fuchsia_inspect::component;
    use futures::channel::mpsc;
    use settings_common::inspect::config_logger::InspectConfigLogger;
    use settings_common::service_context::ServiceContext;
    use settings_test_common::storage::InMemoryStorageFactory;

    #[fuchsia::test]
    fn test_input_migration_v1_to_current() {
        const MUTED_MIC: Microphone = Microphone { muted: true };
        let v1 = InputInfoSourcesV1 { sw_microphone: MUTED_MIC, ..Default::default() };

        let serialized_v1 = v1.serialize_to();
        let current = InputInfoSources::try_deserialize_from(&serialized_v1)
            .expect("deserialization should succeed");
        let mut expected_input_state = InputState::new();
        expected_input_state.set_source_state(
            InputDeviceType::MICROPHONE,
            DEFAULT_MIC_NAME.to_string(),
            DeviceStateSource::SOFTWARE,
            DeviceState::MUTED,
        );
        expected_input_state.set_source_state(
            InputDeviceType::MICROPHONE,
            DEFAULT_MIC_NAME.to_string(),
            DeviceStateSource::HARDWARE,
            DeviceState::AVAILABLE,
        );
        assert_eq!(current.input_device_state, expected_input_state);
    }

    #[fuchsia::test]
    fn test_input_migration_v1_to_v2() {
        const MUTED_MIC: Microphone = Microphone { muted: true };
        let v1 = InputInfoSourcesV1 { sw_microphone: MUTED_MIC, ..Default::default() };

        let serialized_v1 = v1.serialize_to();
        let v2 = InputInfoSourcesV2::try_deserialize_from(&serialized_v1)
            .expect("deserialization should succeed");

        assert_eq!(v2.hw_microphone, Microphone { muted: false });
        assert_eq!(v2.sw_microphone, MUTED_MIC);
        assert_eq!(v2.input_device_state, InputState::new());
    }

    #[fuchsia::test]
    fn test_input_migration_v2_to_current() {
        const DEFAULT_CAMERA_NAME: &str = "camera";
        const MUTED_MIC: Microphone = Microphone { muted: true };
        let mut v2 = InputInfoSourcesV2::default();
        v2.input_device_state.set_source_state(
            InputDeviceType::CAMERA,
            DEFAULT_CAMERA_NAME.to_string(),
            DeviceStateSource::SOFTWARE,
            DeviceState::AVAILABLE,
        );
        v2.input_device_state.set_source_state(
            InputDeviceType::CAMERA,
            DEFAULT_CAMERA_NAME.to_string(),
            DeviceStateSource::HARDWARE,
            DeviceState::MUTED,
        );
        v2.sw_microphone = MUTED_MIC;

        let serialized_v2 = v2.serialize_to();
        let current = InputInfoSources::try_deserialize_from(&serialized_v2)
            .expect("deserialization should succeed");
        let mut expected_input_state = InputState::new();

        expected_input_state.set_source_state(
            InputDeviceType::MICROPHONE,
            DEFAULT_MIC_NAME.to_string(),
            DeviceStateSource::SOFTWARE,
            DeviceState::MUTED,
        );
        expected_input_state.set_source_state(
            InputDeviceType::MICROPHONE,
            DEFAULT_MIC_NAME.to_string(),
            DeviceStateSource::HARDWARE,
            DeviceState::AVAILABLE,
        );
        expected_input_state.set_source_state(
            InputDeviceType::CAMERA,
            DEFAULT_CAMERA_NAME.to_string(),
            DeviceStateSource::SOFTWARE,
            DeviceState::AVAILABLE,
        );
        expected_input_state.set_source_state(
            InputDeviceType::CAMERA,
            DEFAULT_CAMERA_NAME.to_string(),
            DeviceStateSource::HARDWARE,
            DeviceState::MUTED,
        );

        assert_eq!(current.input_device_state, expected_input_state);
    }

    #[fuchsia::test]
    async fn test_camera_error_on_restore() {
        let (event_tx, _event_rx) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);
        let storage_factory = InMemoryStorageFactory::new();
        storage_factory
            .initialize::<InputController>()
            .await
            .expect("controller should have impls");
        let (value_tx, _value_rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(value_tx);
        let mut controller: InputController =
            InputController::create_with_config::<InMemoryStorageFactory>(
                Rc::new(ServiceContext::new(None)),
                InputConfiguration {
                    devices: vec![InputDeviceConfiguration {
                        device_name: DEFAULT_CAMERA_NAME.to_string(),
                        device_type: InputDeviceType::CAMERA,
                        source_states: vec![SourceState {
                            source: DeviceStateSource::SOFTWARE,
                            state: 0,
                        }],
                        mutable_toggle_state: 0,
                    }],
                },
                &storage_factory,
                setting_value_publisher,
                external_publisher,
            )
            .await;

        // Restore should pass.
        let result = controller.restore().await;
        assert!(result.is_ok());

        // But the camera state should show an error.
        let camera_state = controller
            .input_device_state
            .get_state(InputDeviceType::CAMERA, DEFAULT_CAMERA_NAME.to_string())
            .unwrap();
        assert!(camera_state.has_state(DeviceState::ERROR));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_controller_creation_with_default_config() {
        let config_logger = InspectConfigLogger::new(component::inspector().root());
        let mut default_setting = DefaultSetting::new(
            Some(InputConfiguration::default()),
            "/config/data/input_device_config.json",
            Rc::new(std::sync::Mutex::new(config_logger)),
        );

        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let storage_factory = InMemoryStorageFactory::new();
        storage_factory
            .initialize::<InputController>()
            .await
            .expect("controller should have impls");
        let (value_tx, _value_rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(value_tx);
        let _controller = InputController::new(
            Rc::new(ServiceContext::new(None)),
            &mut default_setting,
            Rc::new(storage_factory),
            setting_value_publisher,
            external_publisher,
        )
        .await
        .expect("Should have controller");
    }

    #[fuchsia::test]
    async fn test_set_input_states_limit() {
        let (event_tx, _event_rx) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);
        let storage_factory = InMemoryStorageFactory::new();
        storage_factory
            .initialize::<InputController>()
            .await
            .expect("controller should have impls");
        let (value_tx, _value_rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(value_tx);

        let mut device_configs = Vec::new();
        for i in 0..MAX_INPUT_DEVICES {
            device_configs.push(InputDeviceConfiguration {
                device_name: format!("mic{i}"),
                device_type: InputDeviceType::MICROPHONE,
                source_states: vec![SourceState { source: DeviceStateSource::SOFTWARE, state: 0 }],
                mutable_toggle_state: 0,
            });
        }

        let mut controller: InputController =
            InputController::create_with_config::<InMemoryStorageFactory>(
                Rc::new(ServiceContext::new(None)),
                InputConfiguration { devices: device_configs },
                &storage_factory,
                setting_value_publisher,
                external_publisher,
            )
            .await;

        let _ = controller.restore().await;

        let overflow_dev = InputDevice {
            name: "mic_max_exceeded".to_string(),
            device_type: InputDeviceType::MICROPHONE,
            source_states: [(DeviceStateSource::SOFTWARE, DeviceState::AVAILABLE)].into(),
            state: DeviceState::AVAILABLE,
        };
        let res =
            controller.set_input_states(vec![overflow_dev], DeviceStateSource::SOFTWARE).await;
        match res {
            Err(InputError::MaximumInputDeviceLimitReached(msg)) => {
                assert_eq!(
                    msg,
                    format!("Maximum limit of {MAX_INPUT_DEVICES} input devices has been reached.")
                );
            }
            _ => panic!("Expected MaximumInputDeviceLimitReached, got {res:?}"),
        }
    }

    async fn handle_camera3_device(
        mut stream: fidl_fuchsia_camera3::DeviceRequestStream,
        camera_muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        use futures::StreamExt;
        use std::sync::atomic::Ordering;
        while let Some(Ok(req)) = stream.next().await {
            if let fidl_fuchsia_camera3::DeviceRequest::SetSoftwareMuteState { muted, responder } =
                req
            {
                camera_muted.store(muted, Ordering::Relaxed);
                let _ = responder.send();
            }
        }
    }

    async fn handle_camera3_device_watcher(
        mut stream: fidl_fuchsia_camera3::DeviceWatcherRequestStream,
        camera_muted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        use futures::StreamExt;
        while let Some(Ok(req)) = stream.next().await {
            #[allow(unreachable_patterns)]
            match req {
                fidl_fuchsia_camera3::DeviceWatcherRequest::WatchDevices { responder } => {
                    let _ = responder.send(&[fidl_fuchsia_camera3::WatchDevicesEvent::Added(1)]);
                }
                fidl_fuchsia_camera3::DeviceWatcherRequest::ConnectToDevice {
                    id: _,
                    request,
                    control_handle: _,
                } => {
                    let camera_muted = camera_muted.clone();
                    fasync::Task::local(handle_camera3_device(request.into_stream(), camera_muted))
                        .detach();
                }
                _ => {}
            }
        }
    }

    #[fuchsia::test]
    // This tests the bug documented in https://fxbug.dev/521503847. This test validates that
    // the in-memory cached state of the Settings service doesn't commit changes prior to
    // updating the hardware state. If that were to happen, the camera hardware could get "stuck"
    // in its current state, while further updates are rejected due to the internal cache believing
    // that the update is already applied.
    async fn test_set_input_states_retry_on_failure() {
        use fidl::endpoints::DiscoverableProtocolMarker;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let camera_muted = Arc::new(AtomicBool::new(false));
        let camera_muted_clone = camera_muted.clone();

        // Mock the camera service, rejecting the first call (count == 1) to simulate a failure.
        let service_context = Rc::new(ServiceContext::new(Some(Box::new(
            move |service_name: &str, channel: zx::Channel| {
                let count = call_count_clone.fetch_add(1, Ordering::Relaxed);
                let camera_muted_clone = camera_muted_clone.clone();
                let service_name = service_name.to_string();
                Box::pin(async move {
                    if count == 1 {
                        return Err(anyhow::Error::msg("injected failure"));
                    }
                    if service_name != fidl_fuchsia_camera3::DeviceWatcherMarker::PROTOCOL_NAME {
                        return Err(anyhow::Error::msg("unsupported service"));
                    }
                    let stream = fidl::endpoints::ServerEnd::<
                        fidl_fuchsia_camera3::DeviceWatcherMarker,
                    >::new(channel)
                    .into_stream();
                    fasync::Task::local(handle_camera3_device_watcher(stream, camera_muted_clone))
                        .detach();
                    Ok(())
                })
            },
        ))));

        // Initialize the mocked "persistent" storage.
        let storage_factory = InMemoryStorageFactory::new();
        storage_factory
            .initialize::<InputController>()
            .await
            .expect("controller should have impls");
        let (value_tx, _value_rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(value_tx);
        let (event_tx, _event_rx) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let mut controller = InputController::create_with_config::<InMemoryStorageFactory>(
            service_context,
            InputConfiguration {
                devices: vec![InputDeviceConfiguration {
                    device_name: DEFAULT_CAMERA_NAME.to_string(),
                    device_type: InputDeviceType::CAMERA,
                    source_states: vec![SourceState {
                        source: DeviceStateSource::SOFTWARE,
                        state: 0, // AVAILABLE
                    }],
                    mutable_toggle_state: 0,
                }],
            },
            &storage_factory,
            setting_value_publisher,
            external_publisher,
        )
        .await;

        let _ = controller.restore().await;

        // Verify initial state is AVAILABLE (not muted).
        let camera_state = controller
            .input_device_state
            .get_state(InputDeviceType::CAMERA, DEFAULT_CAMERA_NAME.to_string())
            .unwrap();
        assert!(!camera_state.has_state(DeviceState::MUTED));

        // Create a request to MUTE the camera. The hardware API call will fail on this request,
        // due to the previously configured mock camera service.
        let mute_device = InputDevice {
            name: DEFAULT_CAMERA_NAME.to_string(),
            device_type: InputDeviceType::CAMERA,
            source_states: [(DeviceStateSource::SOFTWARE, DeviceState::MUTED)].into(),
            state: DeviceState::MUTED,
        };

        let res = controller
            .set_input_states(vec![mute_device.clone()], DeviceStateSource::SOFTWARE)
            .await;
        assert!(res.is_err());
        assert_eq!(call_count.load(Ordering::Relaxed), 2);
        assert!(!camera_muted.load(Ordering::Relaxed));

        // Attempt to mute again. If the bug this test tracks is present, this will skip
        // push_cam_sw_state and return Ok, because the service thinks the state is already MUTED
        // due to the in-memory state being updated despite the hardware update failing.
        let res2 =
            controller.set_input_states(vec![mute_device], DeviceStateSource::SOFTWARE).await;

        // If the bug is fixed, we expect that the camera service will have been called again, the
        // result was okay and the mute state was properly updated.
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
        assert!(res2.is_ok());
        assert!(camera_muted.load(Ordering::Relaxed));
    }
}
