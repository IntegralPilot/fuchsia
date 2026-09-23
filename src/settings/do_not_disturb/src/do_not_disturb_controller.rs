// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::do_not_disturb_fidl_handler::Publisher;
use crate::types::DoNotDisturbInfo;
use anyhow::{Context, Error};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::inspect::event::{ResponseType, SettingValuePublisher};
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::rc::Rc;

impl DeviceStorageCompatible for DoNotDisturbInfo {
    type Loader = NoneT;
    const KEY: &'static str = "do_not_disturb_info";
}

impl Default for DoNotDisturbInfo {
    fn default() -> Self {
        DoNotDisturbInfo::new(false, false)
    }
}

impl StorageAccess for DoNotDisturbController {
    type Storage = DeviceStorage;
    type Data = DoNotDisturbInfo;
    const STORAGE_KEY: &'static str = DoNotDisturbInfo::KEY;
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum DoNotDisturbError {
    #[error("Write failed for DoNotDisturb: {0:?}")]
    WriteFailure(Error),
}

impl From<&DoNotDisturbError> for ResponseType {
    fn from(error: &DoNotDisturbError) -> Self {
        match error {
            DoNotDisturbError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(DoNotDisturbInfo, Sender<Result<(), DoNotDisturbError>>),
}

pub struct DoNotDisturbController {
    store: Rc<DeviceStorage>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<DoNotDisturbInfo>,
}

impl DoNotDisturbController {
    pub(crate) async fn new<F>(
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<DoNotDisturbInfo>,
    ) -> DoNotDisturbController
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        Self { store: storage_factory.get_store().await, publisher: None, setting_value_publisher }
    }

    pub(crate) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: DoNotDisturbInfo) {
        let _ = self.setting_value_publisher.publish(&info);
        if let Some(publisher) = self.publisher.as_ref() {
            publisher.set(info);
        }
    }

    pub(crate) async fn handle(
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

    pub(crate) async fn restore(&self) -> DoNotDisturbInfo {
        self.store.get::<DoNotDisturbInfo>().await
    }

    async fn set(
        &self,
        dnd_info: DoNotDisturbInfo,
    ) -> Result<Option<DoNotDisturbInfo>, DoNotDisturbError> {
        let mut stored_value = self.store.get::<DoNotDisturbInfo>().await;
        if dnd_info.user_dnd.is_some() {
            stored_value.user_dnd = dnd_info.user_dnd;
        }
        if dnd_info.night_mode_dnd.is_some() {
            stored_value.night_mode_dnd = dnd_info.night_mode_dnd;
        }
        self.store
            .write(&stored_value)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(stored_value))
            .context("writing do not disturb to storage")
            .map_err(DoNotDisturbError::WriteFailure)
    }
}
