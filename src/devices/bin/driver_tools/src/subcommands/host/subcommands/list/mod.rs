// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::Result;
use args::ListCommand;
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

#[derive(Serialize)]
pub struct DriverHost {
    pub koid: u64,
    pub name: Option<String>,
    pub drivers: Vec<String>,
}

pub async fn get_driver_hosts(
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<Vec<DriverHost>> {
    let device_info = fuchsia_driver_dev::get_device_info(
        driver_development_proxy,
        &[],
        /* exact_match= */ false,
    )
    .await?;

    let driver_host_info =
        fuchsia_driver_dev::get_driver_host_info(driver_development_proxy).await?;

    let mut driver_host_drivers = BTreeMap::new();

    for device in device_info {
        if let Some(koid) = device.driver_host_koid
            && let Some(url) = device.bound_driver_url
        {
            driver_host_drivers.entry(koid).or_insert(BTreeSet::new()).insert(url);
        }
    }

    let mut driver_hosts_names = BTreeMap::new();

    for host in driver_host_info {
        if let Some(koid) = host.process_koid
            && let Some(name) = host.name
            && !name.is_empty()
        {
            driver_hosts_names.insert(koid, name);
        }
    }

    let mut result = Vec::new();
    for (koid, drivers) in driver_host_drivers {
        result.push(DriverHost {
            koid,
            name: driver_hosts_names.get(&koid).cloned(),
            drivers: drivers.into_iter().collect(),
        });
    }

    Ok(result)
}

pub async fn list(
    _cmd: ListCommand,
    w: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let hosts = get_driver_hosts(&driver_development_proxy).await?;

    for host in hosts {
        if termion::is_tty(&std::io::stdout()) {
            writeln!(w, "Driver Host: {}", host.koid)?;
            if let Some(name) = &host.name {
                writeln!(w, "Name: {}", name)?;
            }
            for driver in &host.drivers {
                writeln!(w, "{:>4}{}", "", driver)?;
            }
            writeln!(w, "")?;
        } else {
            for driver in &host.drivers {
                writeln!(w, "Driver Host: {:<6}{}", host.koid, driver)?;
            }
        }
    }
    Ok(())
}
