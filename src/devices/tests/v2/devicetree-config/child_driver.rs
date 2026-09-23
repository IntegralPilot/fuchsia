// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, driver_register};
use log::info;
use pdev::{PdevExt, PlatformDevice};
use serde::Deserialize;

#[derive(Deserialize, Debug, PartialEq)]
struct MyConfig {
    string_prop: String,
    int_prop: i64,
    nested: NestedConfig,
}

#[derive(Deserialize, Debug, PartialEq)]
struct NestedConfig {
    prop: String,
}

struct ChildDriver;

driver_register!(ChildDriver);

impl Driver for ChildDriver {
    const NAME: &str = "devicetree-config-child";

    async fn start(context: DriverContext) -> Result<Self, DriverError> {
        log::info!("Child driver starting");

        let pdev = context.connect_to_pdev()?;

        let config: MyConfig = pdev.get_deserialized_metadata().await.map_err(|e| {
            log::error!("Failed to get deserialized metadata: {:?}", e);
            e
        })?;

        log::info!("Config: {:?}", config);

        assert_eq!(config.string_prop, "hello");
        assert_eq!(config.int_prop, 0x12345678);
        assert_eq!(config.nested.prop, "world");

        log::info!("Child driver verified config successfully!");

        Ok(ChildDriver)
    }

    async fn stop(&self) {
        info!("Child driver stopping");
    }
}
