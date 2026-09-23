// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
pub mod setup_controller;
pub mod setup_fidl_handler;
pub mod types;

use self::setup_controller::SetupController;
use self::setup_fidl_handler::SetupFidlHandler;
use self::types::SetupInfo;
use settings_common::inspect::event::{
    ExternalEventPublisher, SettingValuePublisher, UsagePublisher,
};
use settings_common::service_context::ServiceContext;
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;

pub struct SetupResult {
    pub setup_fidl_handler: SetupFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_setup_api<F>(
    service_context: Rc<ServiceContext>,
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<SetupInfo>,
    usage_publisher: UsagePublisher<SetupInfo>,
    external_publisher: ExternalEventPublisher,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut setup_controller = SetupController::new(
        service_context,
        storage_factory,
        setting_value_publisher.clone(),
        external_publisher,
    )
    .await;
    let initial_value = setup_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (setup_fidl_handler, request_rx) =
        SetupFidlHandler::new(&mut setup_controller, usage_publisher, initial_value);
    let task = setup_controller.handle(request_rx).await;
    SetupResult { setup_fidl_handler, task }
}
