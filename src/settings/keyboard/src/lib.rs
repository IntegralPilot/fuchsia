// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust-clippy/issues/13885) Upstream issue
// Clippy lints on the license comment above.
#![allow(clippy::literal_string_with_formatting_args)]

pub mod keyboard_controller;
pub mod keyboard_fidl_handler;
pub mod types;

use self::keyboard_controller::KeyboardController;
use self::keyboard_fidl_handler::KeyboardFidlHandler;
use settings_common::inspect::event::{SettingValuePublisher, UsagePublisher};
use settings_storage::device_storage::DeviceStorage;
use settings_storage::storage_factory::StorageFactory;
use std::rc::Rc;
use types::KeyboardInfo;

pub struct SetupResult {
    pub keyboard_fidl_handler: KeyboardFidlHandler,
    pub task: fuchsia_async::Task<()>,
}

pub async fn setup_keyboard_api<F>(
    storage_factory: Rc<F>,
    setting_value_publisher: SettingValuePublisher<KeyboardInfo>,
    usage_publisher: UsagePublisher<KeyboardInfo>,
) -> SetupResult
where
    F: StorageFactory<Storage = DeviceStorage>,
{
    let mut keyboard_controller =
        KeyboardController::new(storage_factory, setting_value_publisher.clone()).await;
    let initial_value = keyboard_controller.restore().await;
    let _ = setting_value_publisher.publish(&initial_value);

    let (keyboard_fidl_handler, request_rx) =
        KeyboardFidlHandler::new(&mut keyboard_controller, usage_publisher, initial_value);
    let task = keyboard_controller.handle(request_rx).await;
    SetupResult { keyboard_fidl_handler, task }
}
