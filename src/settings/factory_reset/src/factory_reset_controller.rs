// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::factory_reset_fidl_handler::Publisher;
use crate::types::FactoryResetInfo;
use anyhow::{Context, Error};
use fidl_fuchsia_recovery_policy::{DeviceMarker, DeviceProxy};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::call;
use settings_common::inspect::event::{
    ExternalEventPublisher, ResponseType, SettingValuePublisher,
};
use settings_common::service_context::{ExternalServiceProxy, ServiceContext};
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::borrow::Cow;
use std::rc::Rc;

impl DeviceStorageCompatible for FactoryResetInfo {
    type Loader = NoneT;
    const KEY: &'static str = "factory_reset_info";
}

impl Default for FactoryResetInfo {
    fn default() -> Self {
        FactoryResetInfo::new(true)
    }
}

// Clippy doesn't like the same "Failure" suffix for all variants, but we prefer
// to follow the convention used in the other setting error types.
#[allow(clippy::enum_variant_names)]
#[derive(thiserror::Error, Debug)]
pub(crate) enum FactoryResetError {
    #[error("Failed to initialize controller: {0:?}")]
    InitFailure(Error),
    #[error("External failure for FactoryReset: dependency: {0:?} request:{1:?} error:{2}")]
    ExternalFailure(Cow<'static, str>, Cow<'static, str>, Cow<'static, str>),
    #[error("Write failed for FactoryReset: {0:?}")]
    WriteFailure(Error),
}

impl From<&FactoryResetError> for ResponseType {
    fn from(error: &FactoryResetError) -> Self {
        match error {
            FactoryResetError::InitFailure(..) => ResponseType::InitFailure,
            FactoryResetError::ExternalFailure(..) => ResponseType::ExternalFailure,
            FactoryResetError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

pub(crate) enum Request {
    Set(FactoryResetInfo, Sender<Result<(), FactoryResetError>>),
}

pub struct FactoryResetController {
    store: Rc<DeviceStorage>,
    is_local_reset_allowed: bool,
    factory_reset_policy_service: ExternalServiceProxy<DeviceProxy, ExternalEventPublisher>,
    publisher: Option<Publisher>,
    setting_value_publisher: SettingValuePublisher<FactoryResetInfo>,
}

impl StorageAccess for FactoryResetController {
    type Storage = DeviceStorage;
    type Data = FactoryResetInfo;
    const STORAGE_KEY: &'static str = FactoryResetInfo::KEY;
}

impl FactoryResetController {
    pub(crate) async fn new<F>(
        service_context: &ServiceContext,
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<FactoryResetInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Result<FactoryResetController, FactoryResetError>
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        let factory_reset_policy_service = service_context
            .connect_with_publisher::<DeviceMarker, _>(external_publisher)
            .await
            .context("connecting to factory reset service")
            .map_err(FactoryResetError::InitFailure)?;
        Ok(Self {
            store: storage_factory.get_store().await,
            is_local_reset_allowed: true,
            factory_reset_policy_service,
            publisher: None,
            setting_value_publisher,
        })
    }

    pub(crate) fn register_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: FactoryResetInfo) {
        let _ = self.setting_value_publisher.publish(&info);
        if let Some(publisher) = self.publisher.as_ref() {
            publisher.set(info);
        }
    }

    pub(crate) async fn handle(
        mut self,
        mut request_rx: UnboundedReceiver<Request>,
    ) -> fasync::Task<()> {
        fasync::Task::local(async move {
            while let Some(request) = request_rx.next().await {
                let Request::Set(info, tx) = request;
                let res =
                    self.set_local_reset_allowed(info.is_local_reset_allowed).await.map(|info| {
                        if let Some(info) = info {
                            self.publish(info);
                        }
                    });
                let _ = tx.send(res);
            }
        })
    }

    pub(crate) async fn restore(&mut self) -> Result<FactoryResetInfo, FactoryResetError> {
        let info = self.store.get::<FactoryResetInfo>().await;
        self.is_local_reset_allowed = info.is_local_reset_allowed;
        call!(self.factory_reset_policy_service =>
            set_is_local_reset_allowed(info.is_local_reset_allowed)
        )
        .map_err(|e| {
            FactoryResetError::ExternalFailure(
                "factory_reset_policy".into(),
                "restore_reset_state".into(),
                format!("{e:?}").into(),
            )
        })?;

        Ok(info)
    }

    async fn set_local_reset_allowed(
        &mut self,
        is_local_reset_allowed: bool,
    ) -> Result<Option<FactoryResetInfo>, FactoryResetError> {
        let mut info = self.store.get::<FactoryResetInfo>().await;
        self.is_local_reset_allowed = is_local_reset_allowed;
        info.is_local_reset_allowed = is_local_reset_allowed;
        call!(self.factory_reset_policy_service =>
            set_is_local_reset_allowed(info.is_local_reset_allowed)
        )
        .map_err(|e| {
            FactoryResetError::ExternalFailure(
                "factory_reset_policy".into(),
                "set_local_reset_allowed".into(),
                format!("{e:?}").into(),
            )
        })?;
        self.store
            .immediate_write(&info)
            .await
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .context("writing factory reset")
            .map_err(FactoryResetError::WriteFailure)
    }
}
