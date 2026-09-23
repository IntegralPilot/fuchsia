// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::accessibility_fidl_handler::Publisher;
use crate::types::AccessibilityInfo;
use anyhow::{Context, Error};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::inspect::event::{ResponseType, SettingValuePublisher};
use settings_common::utils::Merge;
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::rc::Rc;

impl DeviceStorageCompatible for AccessibilityInfo {
    type Loader = NoneT;
    const KEY: &'static str = "accessibility_info";
}

#[derive(thiserror::Error, Debug)]
pub enum AccessibilityError {
    #[error("Invalid argument: arg: {0:?}, value: {1:?}")]
    InvalidArgument(&'static str, String),
    #[error("Write failed for Display: {0:?}")]
    WriteFailure(Error),
}

impl From<&AccessibilityError> for ResponseType {
    fn from(error: &AccessibilityError) -> Self {
        match error {
            AccessibilityError::InvalidArgument(..) => ResponseType::InvalidArgument,
            AccessibilityError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(AccessibilityInfo, Sender<Result<(), AccessibilityError>>),
}

impl StorageAccess for AccessibilityController {
    type Storage = DeviceStorage;
    type Data = AccessibilityInfo;
    const STORAGE_KEY: &'static str = AccessibilityInfo::KEY;
}

pub struct AccessibilityController {
    store: Rc<DeviceStorage>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<AccessibilityInfo>,
}

impl AccessibilityController {
    pub(super) async fn new<F>(
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<AccessibilityInfo>,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        AccessibilityController {
            store: storage_factory.get_store().await,
            publisher: None,
            setting_value_publisher,
        }
    }

    pub(super) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: AccessibilityInfo) {
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
        info: AccessibilityInfo,
    ) -> Result<Option<AccessibilityInfo>, AccessibilityError> {
        let original_info = self.store.get::<AccessibilityInfo>().await;
        assert!(original_info.is_finite());
        // Validate accessibility info contains valid float numbers.
        if !info.is_finite() {
            return Err(AccessibilityError::InvalidArgument("accessibility", format!("{info:?}")));
        }

        let info = original_info.merge(info);
        self.store
            .write(&info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .context("updating accessibility info")
            .map_err(AccessibilityError::WriteFailure)
    }

    pub(super) async fn restore(&self) -> AccessibilityInfo {
        self.store.get::<AccessibilityInfo>().await
    }
}
