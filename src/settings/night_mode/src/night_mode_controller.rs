// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::night_mode_fidl_handler::Publisher;
use crate::types::NightModeInfo;
use anyhow::Error;
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::inspect::event::{ResponseType, SettingValuePublisher};
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::rc::Rc;

impl DeviceStorageCompatible for NightModeInfo {
    type Loader = NoneT;
    const KEY: &'static str = "night_mode_info";
}

impl StorageAccess for NightModeController {
    type Storage = DeviceStorage;
    type Data = NightModeInfo;
    const STORAGE_KEY: &'static str = NightModeInfo::KEY;
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum NightModeError {
    #[error("Write failed for NightMode: {0:?}")]
    WriteFailure(Error),
}

impl From<&NightModeError> for ResponseType {
    fn from(error: &NightModeError) -> Self {
        match error {
            NightModeError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(Option<bool>, Sender<Result<(), NightModeError>>),
}

pub struct NightModeController {
    store: Rc<DeviceStorage>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<NightModeInfo>,
}

impl NightModeController {
    pub(super) async fn new<F>(
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<NightModeInfo>,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        NightModeController {
            store: storage_factory.get_store().await,
            publisher: None,
            setting_value_publisher,
        }
    }

    pub(super) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: NightModeInfo) {
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
                let Request::Set(night_mode_enabled, tx) = request;
                let res = self.set(night_mode_enabled).await.map(|info| {
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
        night_mode_enabled: Option<bool>,
    ) -> Result<Option<NightModeInfo>, NightModeError> {
        let mut info = self.store.get::<NightModeInfo>().await;
        info.night_mode_enabled = night_mode_enabled;

        self.store
            .write(&info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .map_err(NightModeError::WriteFailure)
    }

    pub(super) async fn restore(&self) -> NightModeInfo {
        self.store.get::<NightModeInfo>().await
    }
}
