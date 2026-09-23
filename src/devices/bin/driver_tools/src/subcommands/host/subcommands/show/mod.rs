// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::Result;
use args::{PidOrName, ShowCommand};
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::Write;

#[derive(Serialize)]
pub struct DriverHostDetails {
    pub name: Option<String>,
    pub koid: Option<u64>,
    pub drivers: Vec<String>,
    pub devices: Vec<String>,
}

pub async fn get_driver_host_details(
    cmd: &ShowCommand,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<DriverHostDetails> {
    let device_info = fuchsia_driver_dev::get_device_info(
        driver_development_proxy,
        &[],
        /* exact_match= */ false,
    )
    .await?;

    let driver_host_info =
        fuchsia_driver_dev::get_driver_host_info(driver_development_proxy).await?;

    let mut drivers = BTreeSet::new();
    let mut devices = Vec::new();

    let Some(driver_host) = driver_host_info.iter().find(|info| match &cmd.pid_or_name {
        PidOrName::Pid(pid) => info.process_koid.as_ref().unwrap() == pid,
        PidOrName::Name(name) => info.name.as_ref().unwrap() == name,
    }) else {
        anyhow::bail!("driver host not found");
    };

    for device in device_info {
        if let Some(koid) = device.driver_host_koid
            && koid == driver_host.process_koid.unwrap()
        {
            if let Some(url) = device.bound_driver_url {
                drivers.insert(url);
            }
            if let Some(moniker) = device.moniker {
                devices.push(moniker);
            }
        }
    }

    Ok(DriverHostDetails {
        name: driver_host.name.clone(),
        koid: driver_host.process_koid,
        drivers: drivers.into_iter().collect(),
        devices,
    })
}

pub async fn show(
    cmd: ShowCommand,
    w: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let details = get_driver_host_details(&cmd, &driver_development_proxy).await?;

    if let Some(name) = &details.name
        && !name.is_empty()
    {
        writeln!(w, "Name: {name}")?;
    }
    if let Some(koid) = details.koid {
        writeln!(w, "PID:  {koid}")?;
        writeln!(w, "")?;
    }

    writeln!(w, "Drivers:")?;
    for driver in details.drivers {
        writeln!(w, "{:>4}{}", "", driver)?;
    }
    writeln!(w, "")?;
    writeln!(w, "Devices:")?;
    for device in details.devices {
        writeln!(w, "{:>4}{}", "", device)?;
    }
    Ok(())
}
