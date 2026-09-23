// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(crate) use ::input_pipeline_dso as lib;
pub(crate) use ::scene_management_dso as scene_management;

use crate::lib::Incoming;
use anyhow::Error;
use fuchsia_dso::DsoAsyncArgs;
use std::sync::Arc;

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

const ROLE_NAME: &str = "fuchsia.ui.common_dispatcher";

#[fuchsia_dso::main(async, logging_tags = [ "scene_manager" ])]
async fn main(args: DsoAsyncArgs) -> Result<(), Error> {
    let incoming = Incoming::new(Arc::new(fdf_component::Incoming::from(args.incoming)));
    let outgoing_dir = args.outgoing_dir.expect("missing outgoing dir");
    let config = args.config.expect("missing config vmo");
    crate::top::start(incoming, outgoing_dir, config, ROLE_NAME, "/pkg/lib/libscene_manager.so")
        .await
}
