// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::intl_fidl_handler::Publisher;
use crate::types::{HourCycle, IntlInfo, LocaleId, TemperatureUnit};
use anyhow::Error;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::inspect::event::{ResponseType, SettingValuePublisher};
use settings_common::utils::Merge;
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::fidl_storage::FidlStorageConvertible;
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::collections::HashSet;
use std::rc::Rc;
use {fuchsia_async as fasync, rust_icu_uenum as uenum, rust_icu_uloc as uloc};

impl DeviceStorageCompatible for IntlInfo {
    type Loader = NoneT;
    const KEY: &'static str = "intl_info";
}

impl FidlStorageConvertible for IntlInfo {
    type Storable = fidl_fuchsia_settings::IntlSettings;
    type Loader = NoneT;
    const KEY: &'static str = "intl";

    fn to_storable(self) -> Self::Storable {
        self.into()
    }

    fn from_storable(storable: Self::Storable) -> Self {
        storable.into()
    }
}

impl Default for IntlInfo {
    fn default() -> Self {
        IntlInfo {
            // `-x-fxdef` is a private use extension and a special marker denoting that the
            // setting is a fallback default, and not actually set through any user action.
            locales: Some(vec![LocaleId { id: "en-US-x-fxdef".to_string() }]),
            temperature_unit: Some(TemperatureUnit::Celsius),
            time_zone_id: Some("UTC".to_string()),
            hour_cycle: Some(HourCycle::H12),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum IntlError {
    #[error("Invalid argument for intl: argument:{0:?} value:{1:?}")]
    InvalidArgument(&'static str, String),
    #[error("Write failed for Intl: {0:?}")]
    WriteFailure(Error),
}

impl From<&IntlError> for ResponseType {
    fn from(error: &IntlError) -> Self {
        match error {
            IntlError::InvalidArgument(..) => ResponseType::InvalidArgument,
            IntlError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(IntlInfo, Sender<Result<(), IntlError>>),
}

pub struct IntlController {
    store: Rc<DeviceStorage>,
    time_zone_ids: std::collections::HashSet<String>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<IntlInfo>,
}

impl StorageAccess for IntlController {
    type Storage = DeviceStorage;
    type Data = IntlInfo;
    const STORAGE_KEY: &'static str = <IntlInfo as DeviceStorageCompatible>::KEY;
}

/// Controller for processing requests surrounding the Intl protocol, backed by a number of
/// services, including TimeZone.
impl IntlController {
    pub(super) async fn new<F>(
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<IntlInfo>,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        IntlController {
            store: storage_factory.get_store().await,
            time_zone_ids: Self::load_time_zones(),
            publisher: None,
            setting_value_publisher,
        }
    }

    pub(super) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: IntlInfo) {
        let _ = self.setting_value_publisher.publish(&info);
        if let Some(publisher) = self.publisher.as_ref() {
            publisher.set(info);
        }
    }

    pub(super) async fn handle(
        self,
        mut request_rx: UnboundedReceiver<Request>,
    ) -> fasync::Task<()> {
        fasync::Task::local(async move {
            while let Some(request) = request_rx.next().await {
                let Request::Set(info, tx) = request;
                let res = self.set(info).await.map(|info| {
                    if let Some(info) = info {
                        self.publish(info);
                    }
                });
                let _ = tx.send(res);
            }
        })
    }

    /// Loads the set of valid time zones from resources.
    fn load_time_zones() -> std::collections::HashSet<String> {
        let _icu_data_loader = icu_data::Loader::new().expect("icu data loaded");

        let time_zone_list = match uenum::open_time_zones() {
            Ok(time_zones) => time_zones,
            Err(err) => {
                log::error!("Unable to load time zones: {:?}", err);
                return HashSet::new();
            }
        };

        time_zone_list.flatten().collect()
    }

    async fn set(&self, info: IntlInfo) -> Result<Option<IntlInfo>, IntlError> {
        self.validate_intl_info(&info)?;

        let current = self.store.get::<IntlInfo>().await;
        let merged = current.merge(info);
        self.store
            .write(&merged)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(merged))
            .map_err(IntlError::WriteFailure)
    }

    /// Checks if the given IntlInfo is valid.
    fn validate_intl_info(&self, info: &IntlInfo) -> Result<(), IntlError> {
        if let Some(time_zone_id) = &info.time_zone_id {
            // Make sure the given time zone ID is valid.
            if !self.time_zone_ids.contains(time_zone_id.as_str()) {
                return Err(IntlError::InvalidArgument("timezone id", time_zone_id.clone()));
            }
        }

        if let Some(time_zone_locale) = &info.locales {
            for locale in time_zone_locale {
                // NB: `try_from` doesn't actually do validation, `for_language_tag` does but doesn't
                // actually generate an error, it just ends up falling back to an empty string.
                let loc = uloc::ULoc::for_language_tag(locale.id.as_str());
                match loc {
                    Ok(parsed) => {
                        if parsed.label().is_empty() {
                            log::error!("Locale is invalid: {:?}", locale.id);
                            return Err(IntlError::InvalidArgument("locale id", locale.id.clone()));
                        }
                    }
                    Err(err) => {
                        log::error!("Error loading locale: {:?}", err);
                        return Err(IntlError::InvalidArgument("locale id", locale.id.clone()));
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn restore(&self) -> IntlInfo {
        self.store.get::<IntlInfo>().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::{mpsc, oneshot};
    use settings_test_common::storage::InMemoryStorageFactory;

    #[fuchsia::test]
    async fn set_one() {
        let storage_factory = InMemoryStorageFactory::new();
        storage_factory.initialize::<IntlController>().await.expect("should initialize storage");
        let storage_factory = Rc::new(storage_factory);

        let (tx, _rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(tx);

        let controller =
            IntlController::new(Rc::clone(&storage_factory), setting_value_publisher).await;
        let (tx, rx) = mpsc::unbounded();
        controller.handle(rx).await.detach();

        let (response_tx, response_rx) = oneshot::channel();
        tx.unbounded_send(Request::Set(
            IntlInfo {
                locales: Some(vec![LocaleId { id: "en-US".to_string() }]),
                temperature_unit: None,
                time_zone_id: None,
                hour_cycle: None,
            },
            response_tx,
        ))
        .expect("can send");

        response_rx.await.expect("can receive").expect("should succeed");
        let storage = storage_factory.get_device_storage().await;
        let info = storage.get::<IntlInfo>().await;

        assert_eq!(info.locales, Some(vec![LocaleId { id: "en-US".to_string() }]));
        assert_eq!(info.temperature_unit, Some(TemperatureUnit::Celsius));
        assert_eq!(info.time_zone_id, Some("UTC".to_string()));
        assert_eq!(info.hour_cycle, Some(HourCycle::H12));
    }
}
