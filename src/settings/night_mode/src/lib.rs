// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod night_mode_controller;
pub mod night_mode_fidl_handler;
pub mod types;

use self::night_mode_controller::NightModeController;
use self::night_mode_fidl_handler::NightModeFidlHandler;
use settings_common::inspect::event::{SettingValuePublisher, UsagePublisher};
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::NightModeInfo;

pub struct SetupResult {
    pub night_mode_fidl_handler: NightModeFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_night_mode_api<F>(
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<NightModeInfo>,
    usage_publisher: UsagePublisher<NightModeInfo>,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut night_mode_controller =
        NightModeController::new(storage_factory, setting_value_publisher.clone()).await;
    let initial_value = night_mode_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (night_mode_fidl_handler, request_rx) =
        NightModeFidlHandler::new(&mut night_mode_controller, usage_publisher, initial_value);
    let task = night_mode_controller.handle(request_rx).await;
    SetupResult { night_mode_fidl_handler, task }
}
