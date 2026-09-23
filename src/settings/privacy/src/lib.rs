// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
pub mod privacy_controller;
pub mod privacy_fidl_handler;
pub mod types;

use self::privacy_controller::PrivacyController;
use self::privacy_fidl_handler::PrivacyFidlHandler;
use settings_common::inspect::event::{SettingValuePublisher, UsagePublisher};
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::PrivacyInfo;

pub struct SetupResult {
    pub privacy_fidl_handler: PrivacyFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_privacy_api<F>(
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<PrivacyInfo>,
    usage_publisher: UsagePublisher<PrivacyInfo>,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut privacy_controller =
        PrivacyController::new(storage_factory, setting_value_publisher.clone()).await;
    let initial_value = privacy_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (privacy_fidl_handler, request_rx) =
        PrivacyFidlHandler::new(&mut privacy_controller, usage_publisher, initial_value);
    let task = privacy_controller.handle(request_rx).await;
    SetupResult { privacy_fidl_handler, task }
}
