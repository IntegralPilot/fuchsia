// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::setup_fidl_handler::InfoPublisher;
use crate::types::{ConfigurationInterfaceFlags, SetupInfo};
use anyhow::Error;
use fidl_fuchsia_hardware_power_statecontrol::{ShutdownAction, ShutdownOptions, ShutdownReason};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot::Sender;
use settings_common::call_async;
use settings_common::inspect::event::{
    ExternalEventPublisher, ResponseType, SettingValuePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::UpdateState;
use settings_storage::device_storage::{DeviceStorage, DeviceStorageCompatible};
use settings_storage::storage_factory::{NoneT, StorageAccess, StorageFactory};
use std::borrow::Cow;
use std::rc::Rc;

#[derive(thiserror::Error, Debug)]
pub(crate) enum SetupError {
    #[error(
        "Call to an external dependency {0:?} for setting type Setup failed. \
         Request:{1:?}: Error:{2}"
    )]
    ExternalFailure(&'static str, Cow<'static, str>, Cow<'static, str>),
    #[error("Write failed for Setup: {0:?}")]
    WriteFailure(Error),
}

impl From<&SetupError> for ResponseType {
    fn from(error: &SetupError) -> Self {
        match error {
            SetupError::ExternalFailure(..) => ResponseType::ExternalFailure,
            SetupError::WriteFailure(..) => ResponseType::StorageFailure,
        }
    }
}

async fn reboot(
    service_context: &ServiceContext,
    external_publisher: ExternalEventPublisher,
) -> Result<(), SetupError> {
    let hardware_power_statecontrol_admin = service_context
        .connect_with_publisher::<fidl_fuchsia_hardware_power_statecontrol::AdminMarker, _>(
            external_publisher,
        )
        .await
        .map_err(|e| {
            SetupError::ExternalFailure(
                "hardware_power_statecontrol_manager",
                "connect".into(),
                format!("{e:?}").into(),
            )
        })?;

    let reboot_err = |e: String| {
        SetupError::ExternalFailure(
            "hardware_power_statecontrol_manager",
            "reboot".into(),
            e.into(),
        )
    };

    call_async!(hardware_power_statecontrol_admin => shutdown(&ShutdownOptions{
        action: Some(ShutdownAction::Reboot),
        reasons: Some(vec![ShutdownReason::UserRequest]), ..Default::default()
    }))
    .await
    .map_err(|e| reboot_err(format!("{e:?}")))
    .and_then(|r| {
        r.map_err(|zx_status| reboot_err(format!("{:?}", zx::Status::err_from_raw(zx_status))))
    })
}

impl DeviceStorageCompatible for SetupInfo {
    type Loader = NoneT;
    const KEY: &'static str = "setup_info";
}

pub(crate) enum Request {
    Set(ConfigurationInterfaceFlags, bool, Sender<Result<(), SetupError>>),
}

pub struct SetupController {
    service_context: Rc<ServiceContext>,
    store: Rc<DeviceStorage>,
    publisher: Option<InfoPublisher>,
    setting_value_publisher: SettingValuePublisher<SetupInfo>,
    external_publisher: ExternalEventPublisher,
}

impl StorageAccess for SetupController {
    type Storage = DeviceStorage;
    type Data = SetupInfo;
    const STORAGE_KEY: &'static str = SetupInfo::KEY;
}

impl SetupController {
    pub(super) async fn new<F>(
        service_context: Rc<ServiceContext>,
        storage_factory: Rc<F>,
        setting_value_publisher: SettingValuePublisher<SetupInfo>,
        external_publisher: ExternalEventPublisher,
    ) -> Self
    where
        F: StorageFactory<Storage = DeviceStorage>,
    {
        SetupController {
            service_context,
            store: storage_factory.get_store().await,
            publisher: None,
            setting_value_publisher,
            external_publisher,
        }
    }

    pub(super) fn register_publisher(&mut self, publisher: InfoPublisher) {
        self.publisher = Some(publisher);
    }

    fn publish(&self, info: SetupInfo) {
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
                let Request::Set(config_interfaces_flags, should_reboot, tx) = request;
                let res = self.set(config_interfaces_flags, should_reboot).await.map(|info| {
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
        config_interfaces_flags: ConfigurationInterfaceFlags,
        should_reboot: bool,
    ) -> Result<Option<SetupInfo>, SetupError> {
        let mut info = self.store.get::<SetupInfo>().await;
        info.configuration_interfaces = config_interfaces_flags;

        // If we plan to reboot, we need to ensure the setting is stored immediately.
        let write_setting_result = match should_reboot {
            true => self.store.immediate_write(&info).await,
            false => self.store.write(&info).await,
        };

        // If the write succeeded, reboot if necessary.
        if write_setting_result.is_ok() && should_reboot {
            reboot(&self.service_context, self.external_publisher.clone()).await?;
        }

        write_setting_result
            .map(|state| (UpdateState::Updated == state).then_some(info))
            .map_err(SetupError::WriteFailure)
    }

    pub(super) async fn restore(&self) -> SetupInfo {
        self.store.get::<SetupInfo>().await
    }
}
