// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::display_configuration::{
    ConfigurationThemeMode, ConfigurationThemeType, DisplayConfiguration,
};
use crate::display_fidl_handler::Publisher;
use crate::types::{
    DisplayInfo, LowLightMode, SetDisplayInfo, Theme, ThemeBuilder, ThemeMode, ThemeType,
};
use anyhow::{Context, Error};
use async_trait::async_trait;
use fidl_fuchsia_ui_brightness::{
    ControlMarker as BrightnessControlMarker, ControlProxy as BrightnessControlProxy,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use serde::{Deserialize, Serialize};
use settings_common::call;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::event::{
    ExternalEventPublisher, ResponseType, SettingValuePublisher,
};
use settings_common::service_context::{ExternalServiceProxy, ServiceContext};
use settings_common::utils::Merge;
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{DefaultLoader, NoneT, StorageAccess, StorageFactory};
use std::rc::Rc;
use std::sync::Mutex;

pub(super) const DEFAULT_MANUAL_BRIGHTNESS_VALUE: f32 = 0.5;
pub(super) const DEFAULT_AUTO_BRIGHTNESS_VALUE: f32 = 0.5;

/// Default display used if no configuration is available.
pub(crate) const DEFAULT_DISPLAY_INFO: DisplayInfo = DisplayInfo::new(
    false,                           /*auto_brightness_enabled*/
    DEFAULT_MANUAL_BRIGHTNESS_VALUE, /*manual_brightness_value*/
    DEFAULT_AUTO_BRIGHTNESS_VALUE,   /*auto_brightness_value*/
    true,                            /*screen_enabled*/
    LowLightMode::Disable,           /*low_light_mode*/
    None,                            /*theme*/
);

/// Returns a default display [`DisplayInfo`] that is derived from
/// [`DEFAULT_DISPLAY_INFO`] with any fields specified in the
/// display configuration set.
pub struct DisplayInfoLoader {
    display_configuration: Mutex<DefaultSetting<DisplayConfiguration, &'static str>>,
}

impl DisplayInfoLoader {
    pub fn new(default_setting: DefaultSetting<DisplayConfiguration, &'static str>) -> Self {
        Self { display_configuration: Mutex::new(default_setting) }
    }
}

impl DefaultLoader for DisplayInfoLoader {
    type Result = DisplayInfo;

    fn default_value(&self) -> Self::Result {
        let mut default_display_info = DEFAULT_DISPLAY_INFO;

        if let Ok(Some(display_configuration)) =
            self.display_configuration.lock().unwrap().get_cached_value()
        {
            default_display_info.theme = Some(Theme {
                theme_type: Some(match display_configuration.theme.theme_type {
                    ConfigurationThemeType::Light => ThemeType::Light,
                }),
                theme_mode: if display_configuration
                    .theme
                    .theme_mode
                    .contains(&ConfigurationThemeMode::Auto)
                {
                    ThemeMode::AUTO
                } else {
                    ThemeMode::empty()
                },
            });
        }

        default_display_info
    }
}

impl DeviceStorageCompatible for DisplayInfo {
    type Loader = DisplayInfoLoader;
    const KEY: &'static str = "display_info";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|_| DisplayInfoV5::try_deserialize_from(value).map(Self::from))
    }
}

impl From<DisplayInfoV5> for DisplayInfo {
    fn from(v5: DisplayInfoV5) -> Self {
        DisplayInfo {
            auto_brightness: v5.auto_brightness,
            auto_brightness_value: DEFAULT_AUTO_BRIGHTNESS_VALUE,
            manual_brightness_value: v5.manual_brightness_value,
            screen_enabled: v5.screen_enabled,
            low_light_mode: v5.low_light_mode,
            theme: v5.theme,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DisplayError {
    #[error("Failed to initialize controller: {0:?}")]
    InitFailure(Error),
    #[error("Invalid argument: arg: {0:?}, value: {1:?}")]
    InvalidArgument(&'static str, String),
    #[error("External failure for Display: dependency: {0:?} request:{1:?} error:{2}")]
    ExternalFailure(&'static str, &'static str, String),
    #[error("Write failed for Display: {0:?}")]
    WriteFailure(Error),
}

impl From<&DisplayError> for ResponseType {
    fn from(error: &DisplayError) -> Self {
        match error {
            DisplayError::InitFailure(..) => ResponseType::InitFailure,
            DisplayError::InvalidArgument(..) => ResponseType::InvalidArgument,
            DisplayError::ExternalFailure(..) => ResponseType::ExternalFailure,
            DisplayError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

#[async_trait(?Send)]
pub trait BrightnessManager: Sized {
    async fn from_context(
        service_context: &ServiceContext,
        external_publisher: ExternalEventPublisher,
    ) -> Result<Self, DisplayError>;
    async fn update_brightness(
        &self,
        info: DisplayInfo,
        store: &DeviceStorage,
        // Allows overriding of the check for whether info has changed. This is necessary for
        // the initial restore call.
        always_send: bool,
    ) -> Result<Option<DisplayInfo>, DisplayError>;
}

#[async_trait(?Send)]
impl BrightnessManager for () {
    async fn from_context(
        _: &ServiceContext,
        _: ExternalEventPublisher,
    ) -> Result<Self, DisplayError> {
        Ok(())
    }

    // This does not send the brightness value on anywhere, it simply stores it.
    // External services will pick up the value and set it on the brightness manager.
    async fn update_brightness(
        &self,
        info: DisplayInfo,
        store: &DeviceStorage,
        _: bool,
    ) -> Result<Option<DisplayInfo>, DisplayError> {
        if !info.is_finite() {
            return Err(DisplayError::InvalidArgument("display_info", format!("{info:?}")));
        }
        store
            .write(&info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .context("updating display info")
            .map_err(DisplayError::WriteFailure)
    }
}

pub struct ExternalBrightnessControl {
    brightness_service: ExternalServiceProxy<BrightnessControlProxy, ExternalEventPublisher>,
}

#[async_trait(?Send)]
impl BrightnessManager for ExternalBrightnessControl {
    async fn from_context(
        service_context: &ServiceContext,
        external_publisher: ExternalEventPublisher,
    ) -> Result<Self, DisplayError> {
        service_context
            .connect_with_publisher::<BrightnessControlMarker, _>(external_publisher)
            .await
            .map(|brightness_service| Self { brightness_service })
            .context("connecting to brightness service")
            .map_err(DisplayError::InitFailure)
    }

    async fn update_brightness(
        &self,
        info: DisplayInfo,
        store: &DeviceStorage,
        always_send: bool,
    ) -> Result<Option<DisplayInfo>, DisplayError> {
        if !info.is_finite() {
            return Err(DisplayError::InvalidArgument("display_info", format!("{info:?}")));
        }
        let new_info = store
            .write(&info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .context("updating brightness")
            .map_err(DisplayError::WriteFailure)?;
        if new_info.is_none() && !always_send {
            return Ok(None);
        }

        if info.auto_brightness {
            call!(self.brightness_service => set_auto_brightness())
        } else {
            call!(self.brightness_service => set_manual_brightness(info.manual_brightness_value))
        }
        .map(|_| new_info)
        .map_err(|e| {
            DisplayError::ExternalFailure("brightness_service", "set_brightness", format!("{e:?}"))
        })
    }
}

pub(crate) enum Request {
    Set(SetDisplayInfo, Sender<Result<(), DisplayError>>),
}

pub struct DisplayController<T = ()> {
    brightness_manager: T,
    store: Rc<DeviceStorage>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<DisplayInfo>,
}

impl<T> StorageAccess for DisplayController<T> {
    type Storage = DeviceStorage;
    type Data = DisplayInfo;
    const STORAGE_KEY: &'static str = DisplayInfo::KEY;
}

impl<T> DisplayController<T>
where
    T: BrightnessManager + 'static,
{
    pub(crate) async fn new<F>(
        service_context: &ServiceContext,
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<DisplayInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Result<DisplayController<T>, DisplayError>
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        let brightness_manager =
            <T as BrightnessManager>::from_context(service_context, external_publisher).await?;
        Ok(Self {
            brightness_manager,
            store: storage_factory.get_store().await,
            publisher: None,
            setting_value_publisher,
        })
    }

    pub(crate) async fn restore(&self) -> Result<DisplayInfo, DisplayError> {
        let display_info = self.store.get::<DisplayInfo>().await;
        assert!(display_info.is_finite());

        // Load and set value.
        self.brightness_manager
            .update_brightness(display_info, &self.store, true)
            .await
            // If there was no update to the value, just return the previously retrieved value
            // from storage.
            .map(|info| info.unwrap_or(display_info))
    }

    pub(crate) async fn handle(
        self,
        mut request_rx: UnboundedReceiver<Request>,
    ) -> fasync::Task<()> {
        fasync::Task::local(async move {
            while let Some(request) = request_rx.next().await {
                let Request::Set(mut set_display_info, tx) = request;
                let display_info = self.store.get::<DisplayInfo>().await;
                assert!(display_info.is_finite());

                if let Some(theme) = set_display_info.theme {
                    set_display_info.theme = self.build_theme(theme, &display_info);
                }
                let res = self
                    .brightness_manager
                    .update_brightness(display_info.merge(set_display_info), &self.store, false)
                    .await
                    .map(|info| {
                        if let Some(info) = info {
                            self.publish(info);
                        }
                    });
                let _ = tx.send(res);
            }
        })
    }

    fn build_theme(&self, incoming_theme: Theme, display_info: &DisplayInfo) -> Option<Theme> {
        let existing_theme_type = display_info.theme.and_then(|theme| theme.theme_type);
        let new_theme_type = incoming_theme.theme_type.or(existing_theme_type);

        ThemeBuilder::new()
            .set_theme_type(new_theme_type)
            .set_theme_mode(incoming_theme.theme_mode)
            .build()
    }
}

impl<T> DisplayController<T> {
    pub(crate) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: DisplayInfo) {
        let _ = self.setting_value_publisher.publish(&info);
        if let Some(publisher) = self.publisher.as_ref() {
            publisher.set(info);
        }
    }
}

/// The following struct should never be modified. It represents an old
/// version of the display settings.
#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DisplayInfoV1 {
    /// The last brightness value that was manually set.
    pub manual_brightness_value: f32,
    pub auto_brightness: bool,
    pub low_light_mode: LowLightMode,
}

impl DisplayInfoV1 {
    const fn new(
        auto_brightness: bool,
        manual_brightness_value: f32,
        low_light_mode: LowLightMode,
    ) -> DisplayInfoV1 {
        DisplayInfoV1 { manual_brightness_value, auto_brightness, low_light_mode }
    }
}

impl DeviceStorageCompatible for DisplayInfoV1 {
    type Loader = NoneT;
    const KEY: &'static str = "display_infoV1";
}

impl Default for DisplayInfoV1 {
    fn default() -> Self {
        DisplayInfoV1::new(
            false,                           /*auto_brightness_enabled*/
            DEFAULT_MANUAL_BRIGHTNESS_VALUE, /*brightness_value*/
            LowLightMode::Disable,           /*low_light_mode*/
        )
    }
}

/// The following struct should never be modified.  It represents an old
/// version of the display settings.
#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DisplayInfoV2 {
    pub manual_brightness_value: f32,
    pub auto_brightness: bool,
    pub low_light_mode: LowLightMode,
    pub theme_mode: ThemeModeV1,
}

impl DisplayInfoV2 {
    const fn new(
        auto_brightness: bool,
        manual_brightness_value: f32,
        low_light_mode: LowLightMode,
        theme_mode: ThemeModeV1,
    ) -> DisplayInfoV2 {
        DisplayInfoV2 { manual_brightness_value, auto_brightness, low_light_mode, theme_mode }
    }
}

impl DeviceStorageCompatible for DisplayInfoV2 {
    type Loader = NoneT;
    const KEY: &'static str = "display_infoV2";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|_| DisplayInfoV1::try_deserialize_from(value).map(Self::from))
    }
}

impl Default for DisplayInfoV2 {
    fn default() -> Self {
        DisplayInfoV2::new(
            false,                           /*auto_brightness_enabled*/
            DEFAULT_MANUAL_BRIGHTNESS_VALUE, /*brightness_value*/
            LowLightMode::Disable,           /*low_light_mode*/
            ThemeModeV1::Unknown,            /*theme_mode*/
        )
    }
}

impl From<DisplayInfoV1> for DisplayInfoV2 {
    fn from(v1: DisplayInfoV1) -> Self {
        DisplayInfoV2 {
            auto_brightness: v1.auto_brightness,
            manual_brightness_value: v1.manual_brightness_value,
            low_light_mode: v1.low_light_mode,
            theme_mode: ThemeModeV1::Unknown,
        }
    }
}

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ThemeModeV1 {
    Unknown,
    Default,
    Light,
    Dark,
    /// Product can choose a theme based on ambient cues.
    Auto,
}

impl From<ThemeModeV1> for ThemeType {
    fn from(theme_mode_v1: ThemeModeV1) -> Self {
        match theme_mode_v1 {
            ThemeModeV1::Default => ThemeType::Default,
            ThemeModeV1::Light => ThemeType::Light,
            ThemeModeV1::Dark => ThemeType::Dark,
            // ThemeType has removed Auto field, see https://fxbug.dev/42143417
            ThemeModeV1::Unknown | ThemeModeV1::Auto => ThemeType::Unknown,
        }
    }
}

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DisplayInfoV3 {
    /// The last brightness value that was manually set.
    pub manual_brightness_value: f32,
    pub auto_brightness: bool,
    pub screen_enabled: bool,
    pub low_light_mode: LowLightMode,
    pub theme_mode: ThemeModeV1,
}

impl DisplayInfoV3 {
    const fn new(
        auto_brightness: bool,
        manual_brightness_value: f32,
        screen_enabled: bool,
        low_light_mode: LowLightMode,
        theme_mode: ThemeModeV1,
    ) -> DisplayInfoV3 {
        DisplayInfoV3 {
            manual_brightness_value,
            auto_brightness,
            screen_enabled,
            low_light_mode,
            theme_mode,
        }
    }
}

impl DeviceStorageCompatible for DisplayInfoV3 {
    type Loader = NoneT;
    const KEY: &'static str = "display_info";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|_| DisplayInfoV2::try_deserialize_from(value).map(Self::from))
    }
}

impl Default for DisplayInfoV3 {
    fn default() -> Self {
        DisplayInfoV3::new(
            false,                           /*auto_brightness_enabled*/
            DEFAULT_MANUAL_BRIGHTNESS_VALUE, /*brightness_value*/
            true,                            /*screen_enabled*/
            LowLightMode::Disable,           /*low_light_mode*/
            ThemeModeV1::Unknown,            /*theme_mode*/
        )
    }
}

impl From<DisplayInfoV2> for DisplayInfoV3 {
    fn from(v2: DisplayInfoV2) -> Self {
        DisplayInfoV3 {
            auto_brightness: v2.auto_brightness,
            manual_brightness_value: v2.manual_brightness_value,
            screen_enabled: true,
            low_light_mode: v2.low_light_mode,
            theme_mode: v2.theme_mode,
        }
    }
}

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DisplayInfoV4 {
    /// The last brightness value that was manually set.
    pub manual_brightness_value: f32,
    pub auto_brightness: bool,
    pub screen_enabled: bool,
    pub low_light_mode: LowLightMode,
    pub theme_type: ThemeType,
}

impl DisplayInfoV4 {
    const fn new(
        auto_brightness: bool,
        manual_brightness_value: f32,
        screen_enabled: bool,
        low_light_mode: LowLightMode,
        theme_type: ThemeType,
    ) -> DisplayInfoV4 {
        DisplayInfoV4 {
            manual_brightness_value,
            auto_brightness,
            screen_enabled,
            low_light_mode,
            theme_type,
        }
    }
}

impl From<DisplayInfoV3> for DisplayInfoV4 {
    fn from(v3: DisplayInfoV3) -> Self {
        DisplayInfoV4 {
            auto_brightness: v3.auto_brightness,
            manual_brightness_value: v3.manual_brightness_value,
            screen_enabled: v3.screen_enabled,
            low_light_mode: v3.low_light_mode,
            // In v4, the field formerly known as theme_mode was renamed to
            // theme_type.
            theme_type: ThemeType::from(v3.theme_mode),
        }
    }
}

impl DeviceStorageCompatible for DisplayInfoV4 {
    type Loader = NoneT;
    const KEY: &'static str = "display_info";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|_| DisplayInfoV3::try_deserialize_from(value).map(Self::from))
    }
}

impl Default for DisplayInfoV4 {
    fn default() -> Self {
        DisplayInfoV4::new(
            false,                           /*auto_brightness_enabled*/
            DEFAULT_MANUAL_BRIGHTNESS_VALUE, /*brightness_value*/
            true,                            /*screen_enabled*/
            LowLightMode::Disable,           /*low_light_mode*/
            ThemeType::Unknown,              /*theme_type*/
        )
    }
}

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayInfoV5 {
    /// The last brightness value that was manually set.
    pub manual_brightness_value: f32,
    pub auto_brightness: bool,
    pub screen_enabled: bool,
    pub low_light_mode: LowLightMode,
    pub theme: Option<Theme>,
}

impl DisplayInfoV5 {
    const fn new(
        auto_brightness: bool,
        manual_brightness_value: f32,
        screen_enabled: bool,
        low_light_mode: LowLightMode,
        theme: Option<Theme>,
    ) -> DisplayInfoV5 {
        DisplayInfoV5 {
            manual_brightness_value,
            auto_brightness,
            screen_enabled,
            low_light_mode,
            theme,
        }
    }
}

impl From<DisplayInfoV4> for DisplayInfoV5 {
    fn from(v4: DisplayInfoV4) -> Self {
        DisplayInfoV5 {
            auto_brightness: v4.auto_brightness,
            manual_brightness_value: v4.manual_brightness_value,
            screen_enabled: v4.screen_enabled,
            low_light_mode: v4.low_light_mode,
            // Clients has migrated off auto theme_type, we should not get theme_type as Auto
            theme: Some(Theme::new(Some(v4.theme_type), ThemeMode::empty())),
        }
    }
}

impl DeviceStorageCompatible for DisplayInfoV5 {
    type Loader = NoneT;
    const KEY: &'static str = "display_info";

    fn try_deserialize_from(value: &str) -> Result<Self, Error> {
        Self::extract(value).or_else(|_| DisplayInfoV4::try_deserialize_from(value).map(Self::from))
    }
}

impl Default for DisplayInfoV5 {
    fn default() -> Self {
        DisplayInfoV5::new(
            false,                                                          /*auto_brightness_enabled*/
            DEFAULT_MANUAL_BRIGHTNESS_VALUE,                                /*brightness_value*/
            true,                                                           /*screen_enabled*/
            LowLightMode::Disable,                                          /*low_light_mode*/
            Some(Theme::new(Some(ThemeType::Unknown), ThemeMode::empty())), /*theme_type*/
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_display_default_settings;
    use crate::test_fakes::brightness_service::BrightnessService;
    use fuchsia_async::{Task, TestExecutor};
    use fuchsia_inspect::component;
    use futures::channel::mpsc;
    use futures::future;
    use futures::lock::Mutex;
    use settings_common::inspect::config_logger::InspectConfigLogger;
    use settings_test_common::fakes::service::ServiceRegistry;
    use settings_test_common::storage::InMemoryStorageFactory;

    #[fuchsia::test]
    fn test_display_migration_v1_to_v2() {
        let v1 = DisplayInfoV1 {
            manual_brightness_value: 0.6,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
        };

        let serialized_v1 = v1.serialize_to();
        let v2 = DisplayInfoV2::try_deserialize_from(&serialized_v1)
            .expect("deserialization should succeed");

        assert_eq!(
            v2,
            DisplayInfoV2 {
                manual_brightness_value: v1.manual_brightness_value,
                auto_brightness: v1.auto_brightness,
                low_light_mode: v1.low_light_mode,
                theme_mode: DisplayInfoV2::default().theme_mode,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v2_to_v3() {
        let v2 = DisplayInfoV2 {
            manual_brightness_value: 0.7,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme_mode: ThemeModeV1::Default,
        };

        let serialized_v2 = v2.serialize_to();
        let v3 = DisplayInfoV3::try_deserialize_from(&serialized_v2)
            .expect("deserialization should succeed");

        assert_eq!(
            v3,
            DisplayInfoV3 {
                manual_brightness_value: v2.manual_brightness_value,
                auto_brightness: v2.auto_brightness,
                screen_enabled: DisplayInfoV3::default().screen_enabled,
                low_light_mode: v2.low_light_mode,
                theme_mode: v2.theme_mode,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v3_to_v4() {
        let v3 = DisplayInfoV3 {
            manual_brightness_value: 0.7,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme_mode: ThemeModeV1::Light,
            screen_enabled: false,
        };

        let serialized_v3 = v3.serialize_to();
        let v4 = DisplayInfoV4::try_deserialize_from(&serialized_v3)
            .expect("deserialization should succeed");

        // In v4, the field formally known as theme_mode is theme_type.
        assert_eq!(
            v4,
            DisplayInfoV4 {
                manual_brightness_value: v3.manual_brightness_value,
                auto_brightness: v3.auto_brightness,
                low_light_mode: v3.low_light_mode,
                theme_type: ThemeType::Light,
                screen_enabled: v3.screen_enabled,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v4_to_v5() {
        let v4 = DisplayInfoV4 {
            manual_brightness_value: 0.7,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme_type: ThemeType::Dark,
            screen_enabled: false,
        };

        let serialized_v4 = v4.serialize_to();
        let v5 = DisplayInfoV5::try_deserialize_from(&serialized_v4)
            .expect("deserialization should succeed");

        assert_eq!(
            v5,
            DisplayInfoV5 {
                manual_brightness_value: v4.manual_brightness_value,
                auto_brightness: v4.auto_brightness,
                low_light_mode: v4.low_light_mode,
                theme: Some(Theme::new(Some(v4.theme_type), ThemeMode::empty())),
                screen_enabled: v4.screen_enabled,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v1_to_current() {
        let v1 = DisplayInfoV1 {
            manual_brightness_value: 0.6,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
        };

        let serialized_v1 = v1.serialize_to();
        let current = DisplayInfo::try_deserialize_from(&serialized_v1)
            .expect("deserialization should succeed");

        assert_eq!(
            current,
            DisplayInfo {
                manual_brightness_value: v1.manual_brightness_value,
                auto_brightness: v1.auto_brightness,
                low_light_mode: v1.low_light_mode,
                theme: Some(Theme::new(Some(ThemeType::Unknown), ThemeMode::empty())),
                // screen_enabled was added in v3.
                screen_enabled: DisplayInfoV3::default().screen_enabled,
                auto_brightness_value: DEFAULT_DISPLAY_INFO.auto_brightness_value,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v2_to_current() {
        let v2 = DisplayInfoV2 {
            manual_brightness_value: 0.6,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme_mode: ThemeModeV1::Light,
        };

        let serialized_v2 = v2.serialize_to();
        let current = DisplayInfo::try_deserialize_from(&serialized_v2)
            .expect("deserialization should succeed");

        assert_eq!(
            current,
            DisplayInfo {
                manual_brightness_value: v2.manual_brightness_value,
                auto_brightness: v2.auto_brightness,
                low_light_mode: v2.low_light_mode,
                theme: Some(Theme::new(Some(ThemeType::Light), ThemeMode::empty())),
                // screen_enabled was added in v3.
                screen_enabled: DisplayInfoV3::default().screen_enabled,
                auto_brightness_value: DEFAULT_DISPLAY_INFO.auto_brightness_value,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v3_to_current() {
        let v3 = DisplayInfoV3 {
            manual_brightness_value: 0.6,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme_mode: ThemeModeV1::Light,
            screen_enabled: false,
        };

        let serialized_v3 = v3.serialize_to();
        let current = DisplayInfo::try_deserialize_from(&serialized_v3)
            .expect("deserialization should succeed");

        assert_eq!(
            current,
            DisplayInfo {
                manual_brightness_value: v3.manual_brightness_value,
                auto_brightness: v3.auto_brightness,
                low_light_mode: v3.low_light_mode,
                theme: Some(Theme::new(Some(ThemeType::Light), ThemeMode::empty())),
                // screen_enabled was added in v3.
                screen_enabled: v3.screen_enabled,
                auto_brightness_value: DEFAULT_DISPLAY_INFO.auto_brightness_value,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v4_to_current() {
        let v4 = DisplayInfoV4 {
            manual_brightness_value: 0.6,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme_type: ThemeType::Light,
            screen_enabled: false,
        };

        let serialized_v4 = v4.serialize_to();
        let current = DisplayInfo::try_deserialize_from(&serialized_v4)
            .expect("deserialization should succeed");

        assert_eq!(
            current,
            DisplayInfo {
                manual_brightness_value: v4.manual_brightness_value,
                auto_brightness: v4.auto_brightness,
                low_light_mode: v4.low_light_mode,
                theme: Some(Theme::new(Some(ThemeType::Light), ThemeMode::empty())),
                screen_enabled: v4.screen_enabled,
                auto_brightness_value: DEFAULT_DISPLAY_INFO.auto_brightness_value,
            }
        );
    }

    #[fuchsia::test]
    fn test_display_migration_v5_to_current() {
        let v5 = DisplayInfoV5 {
            manual_brightness_value: 0.6,
            auto_brightness: true,
            low_light_mode: LowLightMode::Enable,
            theme: Some(Theme::new(Some(ThemeType::Light), ThemeMode::AUTO)),
            screen_enabled: false,
        };

        let serialized_v5 = v5.serialize_to();
        let current = DisplayInfo::try_deserialize_from(&serialized_v5)
            .expect("deserialization should succeed");

        assert_eq!(
            current,
            DisplayInfo {
                manual_brightness_value: v5.manual_brightness_value,
                auto_brightness: v5.auto_brightness,
                low_light_mode: v5.low_light_mode,
                theme: Some(Theme::new(Some(ThemeType::Light), ThemeMode::AUTO)),
                screen_enabled: v5.screen_enabled,
                auto_brightness_value: DEFAULT_DISPLAY_INFO.auto_brightness_value,
            }
        );
    }

    const AUTO_BRIGHTNESS_LEVEL: f32 = 0.9;

    fn default_settings() -> DefaultSetting<DisplayConfiguration, &'static str> {
        let config_logger =
            Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(component::inspector().root())));
        build_display_default_settings(config_logger)
    }

    // Makes sure that settings are restored from storage when service comes online.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_display_restore_with_storage_controller() {
        // Ensure auto-brightness value is restored correctly.
        validate_restore_with_storage_controller(
            0.7,
            AUTO_BRIGHTNESS_LEVEL,
            true,
            true,
            LowLightMode::Enable,
            None,
        )
        .await;

        // Ensure manual-brightness value is restored correctly.
        validate_restore_with_storage_controller(
            0.9,
            AUTO_BRIGHTNESS_LEVEL,
            false,
            true,
            LowLightMode::Disable,
            None,
        )
        .await;
    }

    async fn validate_restore_with_storage_controller(
        manual_brightness: f32,
        auto_brightness_value: f32,
        auto_brightness: bool,
        screen_enabled: bool,
        low_light_mode: LowLightMode,
        theme: Option<Theme>,
    ) {
        let service_registry = ServiceRegistry::create();
        let info = DisplayInfo {
            manual_brightness_value: manual_brightness,
            auto_brightness_value,
            auto_brightness,
            screen_enabled,
            low_light_mode,
            theme,
        };
        let storage_factory = InMemoryStorageFactory::with_initial_data(&info);
        let (tx, _) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(tx);
        let (tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(tx);

        storage_factory
            .initialize_with_loader::<DisplayController, _>(DisplayInfoLoader::new(
                default_settings(),
            ))
            .await
            .expect("initializing display storage");

        let display_controller = DisplayController::<()>::new(
            &ServiceContext::new(Some(Box::new(ServiceRegistry::serve(service_registry)))),
            Rc::new(storage_factory),
            setting_value_publisher,
            external_publisher,
        )
        .await
        .expect("constructing display controller");

        let info = display_controller.restore().await.expect("restore completed");
        let settings = fidl_fuchsia_settings::DisplaySettings::from(info);

        if auto_brightness {
            assert_eq!(settings.auto_brightness, Some(auto_brightness));
            assert_eq!(settings.adjusted_auto_brightness, Some(auto_brightness_value));
        } else {
            assert_eq!(settings.brightness_value, Some(manual_brightness));
        }
    }

    // Makes sure that settings are restored from storage when service comes online.
    #[fuchsia::test]
    fn test_display_restore_with_brightness_controller() {
        let mut exec = TestExecutor::new();

        // Ensure auto-brightness value is restored correctly.
        validate_restore_with_brightness_controller(
            &mut exec,
            0.7,
            AUTO_BRIGHTNESS_LEVEL,
            true,
            true,
            LowLightMode::Enable,
            None,
        );

        // Ensure manual-brightness value is restored correctly.
        validate_restore_with_brightness_controller(
            &mut exec,
            0.9,
            AUTO_BRIGHTNESS_LEVEL,
            false,
            true,
            LowLightMode::Disable,
            None,
        );
    }

    // Float comparisons are checking that set values are the same when retrieved.
    #[allow(clippy::float_cmp)]
    fn validate_restore_with_brightness_controller(
        exec: &mut TestExecutor,
        manual_brightness: f32,
        auto_brightness_value: f32,
        auto_brightness: bool,
        screen_enabled: bool,
        low_light_mode: LowLightMode,
        theme: Option<Theme>,
    ) {
        let brightness_service_handle = BrightnessService::create();
        let brightness_service_handle_clone = brightness_service_handle.clone();

        let _task = Task::local(async move {
            let service_registry = ServiceRegistry::create();
            service_registry
                .lock()
                .await
                .register_service(Rc::new(Mutex::new(brightness_service_handle_clone)));
            let info = DisplayInfo {
                manual_brightness_value: manual_brightness,
                auto_brightness_value,
                auto_brightness,
                screen_enabled,
                low_light_mode,
                theme,
            };
            let storage_factory = InMemoryStorageFactory::with_initial_data(&info);
            let (tx, _) = mpsc::unbounded();
            let setting_value_publisher = SettingValuePublisher::new(tx);
            let (tx, _) = mpsc::unbounded();
            let external_publisher = ExternalEventPublisher::new(tx);

            storage_factory
                .initialize_with_loader::<DisplayController, _>(DisplayInfoLoader::new(
                    default_settings(),
                ))
                .await
                .expect("initializing display storage");

            let display_controller = DisplayController::<ExternalBrightnessControl>::new(
                &ServiceContext::new(Some(Box::new(ServiceRegistry::serve(service_registry)))),
                Rc::new(storage_factory),
                setting_value_publisher,
                external_publisher,
            )
            .await
            .expect("constructing display controller");

            let _ = display_controller.restore().await.expect("restore completed");
        });

        let _ = exec.run_until_stalled(&mut future::pending::<()>());

        exec.run_singlethreaded(async {
            if auto_brightness {
                let service_auto_brightness =
                    brightness_service_handle.get_auto_brightness().lock().await.unwrap();
                assert_eq!(service_auto_brightness, auto_brightness);
            } else {
                let service_manual_brightness =
                    brightness_service_handle.get_manual_brightness().lock().await.unwrap();
                assert_eq!(service_manual_brightness, manual_brightness);
            }
        });
    }
}
