// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod display_configuration;
pub mod display_controller;
mod display_fidl_handler;
pub mod types;

#[cfg(test)]
mod test_fakes;

use self::display_controller::DisplayController;
use self::display_fidl_handler::DisplayFidlHandler;
use anyhow::{Context, Result};
pub use display_configuration::build_display_default_settings;
use display_controller::BrightnessManager;
use settings_common::inspect::event::{
    ExternalEventPublisher, SettingValuePublisher, UsagePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::DisplayInfo;

pub struct SetupResult {
    pub display_fidl_handler: DisplayFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_display_api<F, T>(
    service_context: &ServiceContext,
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<DisplayInfo>,
    usage_publisher: UsagePublisher<DisplayInfo>,
    external_publisher: ExternalEventPublisher,
) -> Result<SetupResult>
where
    F: StorageFactory<Storage = DeviceStorage>,
    T: BrightnessManager + 'static,
{
    let mut display_controller = DisplayController::<T>::new(
        service_context,
        storage_factory,
        setting_value_publisher.clone(),
        external_publisher,
    )
    .await
    .context("initializing display controller")?;
    let initial_value = display_controller.restore().await.context("Failed to restore display")?;
    let _ = setting_value_publisher.publish(&initial_value);

    let (display_fidl_handler, request_rx) =
        DisplayFidlHandler::new(&mut display_controller, usage_publisher, initial_value);
    let task = display_controller.handle(request_rx).await;
    Ok(SetupResult { display_fidl_handler, task })
}
