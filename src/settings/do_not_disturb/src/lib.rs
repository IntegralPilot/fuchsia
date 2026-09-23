// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod do_not_disturb_controller;
pub mod do_not_disturb_fidl_handler;
pub mod types;

use self::do_not_disturb_controller::DoNotDisturbController;
use self::do_not_disturb_fidl_handler::DoNotDisturbFidlHandler;
use settings_common::inspect::event::{SettingValuePublisher, UsagePublisher};
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::DoNotDisturbInfo;

pub struct SetupResult {
    pub do_not_disturb_fidl_handler: DoNotDisturbFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_do_not_disturb_api<F>(
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<DoNotDisturbInfo>,
    usage_publisher: UsagePublisher<DoNotDisturbInfo>,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut do_not_disturb_controller =
        DoNotDisturbController::new(storage_factory, setting_value_publisher.clone()).await;
    let initial_value = do_not_disturb_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (do_not_disturb_fidl_handler, request_rx) = DoNotDisturbFidlHandler::new(
        &mut do_not_disturb_controller,
        usage_publisher,
        initial_value,
    );
    let task = do_not_disturb_controller.handle(request_rx).await;
    SetupResult { do_not_disturb_fidl_handler, task }
}
