// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod intl_controller;
pub mod intl_fidl_handler;
pub mod types;

use self::intl_controller::IntlController;
use self::intl_fidl_handler::IntlFidlHandler;
use settings_common::inspect::event::{SettingValuePublisher, UsagePublisher};
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::IntlInfo;

pub struct SetupResult {
    pub intl_fidl_handler: IntlFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_intl_api<F>(
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<IntlInfo>,
    usage_publisher: UsagePublisher<IntlInfo>,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut intl_controller =
        IntlController::new(storage_factory, setting_value_publisher.clone()).await;
    let initial_value = intl_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (intl_fidl_handler, request_rx) =
        IntlFidlHandler::new(&mut intl_controller, usage_publisher, initial_value);
    let task = intl_controller.handle(request_rx).await;
    SetupResult { intl_fidl_handler, task }
}
