// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(crate) use ::input_pipeline as lib;
pub(crate) use ::scene_management;

use crate::lib::Incoming;
use anyhow::Error;
use fuchsia_runtime::HandleType;

#[cfg(fuchsia_api_level_at_least = "HEAD")]
mod color_transform_manager;
mod device_listener_registry_server;
mod factory_reset_countdown_server;
mod factory_reset_device_server;
mod input_device_registry_server;
mod input_pipeline;
mod light_sensor_server;
mod media_buttons_listener_registry_server;
mod top;

const ROLE_NAME: &str = "fuchsia.ui.scene_manager";

#[fuchsia::main(logging_tags = [ "scene_manager" ], thread_role = ROLE_NAME)]
async fn main() -> Result<(), Error> {
    let incoming = Incoming::new();
    let outgoing_dir = zx::Channel::from(
        fuchsia_runtime::take_startup_handle(HandleType::DirectoryRequest.into())
            .expect("no DirectoryRequest"),
    );
    let config = zx::Vmo::from(
        fuchsia_runtime::take_startup_handle(HandleType::ComponentConfigVmo.into())
            .expect("no Config"),
    );
    crate::top::start(incoming, outgoing_dir.into(), config, ROLE_NAME, "/pkg/bin/scene_manager")
        .await
}
