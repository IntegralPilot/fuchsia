// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_driver_development as fdd;
use fuchsia_component::client::connect_to_protocol;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedDriverHost {
    pub name: Option<String>,
    pub drivers: HashSet<String>,
}

pub async fn verify_driver_hosts(expected_hosts: Vec<ExpectedDriverHost>) -> Result<()> {
    let manager = connect_to_protocol::<fdd::ManagerMarker>()
        .context("Failed to connect to fuchsia.driver.development.Manager")?;

    let (iterator, server_end) =
        fidl::endpoints::create_proxy::<fdd::DriverHostInfoIteratorMarker>();
    manager.get_driver_host_info(server_end).context("Failed to call GetDriverHostInfo")?;

    let mut actual_hosts = Vec::new();
    loop {
        let hosts = iterator.get_next().await.context("Failed to get next driver host info")?;
        if hosts.is_empty() {
            break;
        }
        actual_hosts.extend(hosts);
    }

    let mut unmatched_expected = expected_hosts;
    let mut unexpected_hosts = Vec::new();

    for actual in actual_hosts {
        let actual_drivers: HashSet<String> = actual
            .drivers
            .as_ref()
            .map(|drivers| drivers.iter().cloned().collect())
            .unwrap_or_default();

        if actual_drivers.is_empty() {
            continue;
        }

        // Try to find a match in expected_hosts. Drivers starting with '?' are optional.
        let match_index = unmatched_expected.iter().position(|expected| {
            let mut matched = 0;
            for driver in &expected.drivers {
                if let Some(driver) = driver.strip_prefix('?') {
                    matched += actual_drivers.contains(driver) as usize;
                } else {
                    if !actual_drivers.contains(driver) {
                        return false;
                    }
                    matched += 1;
                }
            }
            matched == actual_drivers.len()
        });

        if let Some(index) = match_index {
            unmatched_expected.remove(index);
        } else {
            unexpected_hosts.push(actual_drivers);
        }
    }

    // An expected host is allowed to be missing if all of its drivers are optional.
    unmatched_expected
        .retain(|expected| expected.drivers.iter().any(|driver| !driver.starts_with('?')));

    if !unmatched_expected.is_empty() || !unexpected_hosts.is_empty() {
        anyhow::bail!(
            "Driver host enumeration failed.\nMissing: {:?}\nUnexpected: {:?}",
            unmatched_expected,
            unexpected_hosts
        );
    }

    Ok(())
}
