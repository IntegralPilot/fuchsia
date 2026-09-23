// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::Result;
use args::ListHostsCommand;
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use std::collections::{BTreeMap, BTreeSet};

pub async fn list_hosts(
    _cmd: ListHostsCommand,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    println!("This command is deprecated, please use \"driver host list\" in the future.");

    let device_info = fuchsia_driver_dev::get_device_info(
        &driver_development_proxy,
        &[],
        /* exact_match= */ false,
    )
    .await?;

    let mut driver_hosts = BTreeMap::new();

    for device in device_info {
        if let Some(koid) = device.driver_host_koid {
            if let Some(url) = device.bound_driver_url {
                driver_hosts.entry(koid).or_insert(BTreeSet::new()).insert(url);
            }
        }
    }

    for (koid, drivers) in driver_hosts {
        if termion::is_tty(&std::io::stdout()) {
            println!("Driver Host: {}", koid);
            for driver in drivers {
                println!("{:>4}{}", "", driver);
            }
            println!("");
        } else {
            for driver in drivers {
                println!("Driver Host: {:<6}{}", koid, driver);
            }
        }
    }
    Ok(())
}
