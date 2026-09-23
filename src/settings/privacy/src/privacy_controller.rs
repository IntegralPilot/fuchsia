// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::privacy_fidl_handler::Publisher;
use crate::types::PrivacyInfo;
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

impl DeviceStorageCompatible for PrivacyInfo {
    type Loader = NoneT;
    const KEY: &'static str = "privacy_info";
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum PrivacyError {
    #[error("Write failed for Setup: {0:?}")]
    WriteFailure(Error),
}

impl From<&PrivacyError> for ResponseType {
    fn from(error: &PrivacyError) -> Self {
        match error {
            PrivacyError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(Option<bool>, Sender<Result<(), PrivacyError>>),
}

pub struct PrivacyController {
    store: Rc<DeviceStorage>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<PrivacyInfo>,
}

impl PrivacyController {
    pub(super) async fn new<F>(
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<PrivacyInfo>,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        PrivacyController {
            store: storage_factory.get_store().await,
            publisher: None,
            setting_value_publisher,
        }
    }

    pub(super) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: PrivacyInfo) {
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
                let Request::Set(user_data_sharing_consent, tx) = request;
                let res = self.set(user_data_sharing_consent).await.map(|info| {
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
        user_data_sharing_consent: Option<bool>,
    ) -> Result<Option<PrivacyInfo>, PrivacyError> {
        let mut info = self.store.get::<PrivacyInfo>().await;
        info.user_data_sharing_consent = user_data_sharing_consent;

        self.store
            .write(&info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .map_err(PrivacyError::WriteFailure)
    }

    pub(super) async fn restore(&self) -> PrivacyInfo {
        self.store.get::<PrivacyInfo>().await
    }
}

impl StorageAccess for PrivacyController {
    type Storage = DeviceStorage;
    type Data = PrivacyInfo;
    const STORAGE_KEY: &'static str = PrivacyInfo::KEY;
}
