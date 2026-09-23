// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod factory_reset_controller;
pub mod types;

mod factory_reset_fidl_handler;

use self::factory_reset_controller::FactoryResetController;
use self::factory_reset_fidl_handler::FactoryResetFidlHandler;
use anyhow::{Context, Result};
use settings_common::inspect::event::{
    ExternalEventPublisher, SettingValuePublisher, UsagePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::FactoryResetInfo;

pub struct SetupResult {
    pub factory_reset_fidl_handler: FactoryResetFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_factory_reset_api<F>(
    service_context: &ServiceContext,
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<FactoryResetInfo>,
    usage_publisher: UsagePublisher<FactoryResetInfo>,
    external_publisher: ExternalEventPublisher,
) -> Result<SetupResult>
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut factory_reset_controller = FactoryResetController::new(
        service_context,
        storage_factory,
        setting_value_publisher.clone(),
        external_publisher,
    )
    .await
    .context("building factory reset controller")?;
    let initial_value = factory_reset_controller
        .restore()
        .await
        .context("restoring factory reset initial value")?;
    let _ = setting_value_publisher.publish(&initial_value);

    let (factory_reset_fidl_handler, request_rx) =
        FactoryResetFidlHandler::new(&mut factory_reset_controller, usage_publisher, initial_value);
    let task = factory_reset_controller.handle(request_rx).await;
    Ok(SetupResult { factory_reset_fidl_handler, task })
}
