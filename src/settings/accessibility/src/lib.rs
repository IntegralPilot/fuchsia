// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod accessibility_controller;

/// Exposes the supported data types for this setting.
pub mod types;

mod accessibility_fidl_handler;

use self::accessibility_controller::AccessibilityController;
use self::accessibility_fidl_handler::AccessibilityFidlHandler;
use settings_common::inspect::event::{SettingValuePublisher, UsagePublisher};
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::AccessibilityInfo;

pub struct SetupResult {
    pub accessibility_fidl_handler: AccessibilityFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_accessibility_api<F>(
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<AccessibilityInfo>,
    usage_publisher: UsagePublisher<AccessibilityInfo>,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut accessibility_controller =
        AccessibilityController::new(storage_factory, setting_value_publisher.clone()).await;
    let initial_value = accessibility_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (accessibility_fidl_handler, request_rx) = AccessibilityFidlHandler::new(
        &mut accessibility_controller,
        usage_publisher,
        initial_value,
    );
    let task = accessibility_controller.handle(request_rx).await;
    SetupResult { accessibility_fidl_handler, task }
}
