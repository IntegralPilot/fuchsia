// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::keyboard_fidl_handler::Publisher;
use crate::types::{KeyboardInfo, KeymapId};
use anyhow::Error;
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::inspect::event::{ResponseType, SettingValuePublisher};
use settings_common::trace;
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::rc::Rc;

impl DeviceStorageCompatible for KeyboardInfo {
    type Loader = NoneT;
    const KEY: &'static str = "keyboard_info";
}

impl Default for KeyboardInfo {
    fn default() -> Self {
        // The US_QWERTY keymap is the default if no settings are ever applied.
        KeyboardInfo { keymap: Some(KeymapId::UsQwerty), autorepeat: None }
    }
}

impl StorageAccess for KeyboardController {
    type Storage = DeviceStorage;
    type Data = KeyboardInfo;
    const STORAGE_KEY: &'static str = KeyboardInfo::KEY;
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum KeyboardError {
    #[error("Invalid argument for keyboard: argument:{0:?} value:{1:?}")]
    InvalidArgument(&'static str, String),
    #[error("Write failed for Keyboard: {0:?}")]
    WriteFailure(Error),
}

impl From<&KeyboardError> for ResponseType {
    fn from(error: &KeyboardError) -> Self {
        match error {
            KeyboardError::InvalidArgument(..) => ResponseType::InvalidArgument,
            KeyboardError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(KeyboardInfo, Sender<Result<(), KeyboardError>>),
}

#[cfg(test)]
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::Set(info, _) = self;
        f.debug_tuple("Set").field(info).finish_non_exhaustive()
    }
}

pub struct KeyboardController {
    store: Rc<DeviceStorage>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<KeyboardInfo>,
}

impl KeyboardController {
    pub(super) async fn new<F>(
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<KeyboardInfo>,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        KeyboardController {
            store: storage_factory.get_store().await,
            publisher: None,
            setting_value_publisher,
        }
    }

    pub(super) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: KeyboardInfo) {
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

    async fn set(
        &self,
        keyboard_info: KeyboardInfo,
    ) -> Result<Option<KeyboardInfo>, KeyboardError> {
        let id = fuchsia_trace::Id::new();
        trace!(id, "set keyboard");
        let mut current = self.store.get::<KeyboardInfo>().await;
        if !keyboard_info.is_valid() {
            return Err(KeyboardError::InvalidArgument("keyboard", format!("{keyboard_info:?}")));
        }

        current.keymap = keyboard_info.keymap.or(current.keymap);

        // Clean up Autorepeat when delay and period are set to zero.
        current.autorepeat = keyboard_info
            .autorepeat
            .or(current.autorepeat)
            .filter(|&value| !(value.delay == 0 && value.period == 0));

        self.store
            .write(&current)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(current))
            .map_err(KeyboardError::WriteFailure)
    }

    pub(super) async fn restore(&self) -> KeyboardInfo {
        self.store.get::<KeyboardInfo>().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Autorepeat;
    use futures::channel::mpsc;
    use settings_test_common::storage::InMemoryStorageFactory;

    #[fuchsia::test(allow_stalls = false)]
    async fn test_keyboard_storage() {
        let changed_value = KeyboardInfo {
            keymap: Some(KeymapId::UsDvorak),
            autorepeat: Some(Autorepeat { delay: 2, period: 1 }),
        };
        let factory = Rc::new(InMemoryStorageFactory::new());
        let (tx, _) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(tx);

        factory.initialize::<KeyboardController>().await.expect("can initialize keyboard storage");

        // Create and fetch a store from device storage so we can read stored value for testing.
        let keyboard_controller =
            KeyboardController::new(Rc::clone(&factory), setting_value_publisher).await;
        let store = factory.get_store().await;

        // Set a new value.
        let result = keyboard_controller
            .set(changed_value)
            .await
            .expect("set successful")
            .expect("keyboard info changed");
        assert_eq!(result, changed_value);

        // Verify the value we set is persisted in DeviceStorage.
        let retrieved_struct = store.get::<KeyboardInfo>().await;
        assert_eq!(changed_value, retrieved_struct);
    }
}
