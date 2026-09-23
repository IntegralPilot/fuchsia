// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Result, anyhow};
use args::AddNodeCommand;
use {flex_fuchsia_driver_development as fdd, flex_fuchsia_driver_framework as fdf};

fn string_to_property(prop: &str) -> Result<fdf::NodeProperty> {
    let split: Vec<&str> = prop.split("=").collect();
    if split.len() != 2 {
        return Err(anyhow!("Bad Property '{}', properties need one '=' character", prop));
    }

    Ok(fdf::NodeProperty {
        key: fdf::NodePropertyKey::StringValue(split[0].to_string()),
        value: fdf::NodePropertyValue::StringValue(split[1].to_string()),
    })
}

pub async fn add_node(
    cmd: &AddNodeCommand,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    // Currently adding a node just means adding a test node which becomes a child of the root
    // node. Eventually we can expand this to add nodes under specific nodes if that is desired.
    driver_development_proxy
        .add_test_node(&fdd::TestNodeAddArgs {
            name: Some(cmd.name.clone()),
            properties: Some(vec![string_to_property(&cmd.property)?]),
            ..Default::default()
        })
        .await?
        .map_err(|e| anyhow!("Calling AddTestNode failed with {:#?}", e))?;
    Ok(())
}
