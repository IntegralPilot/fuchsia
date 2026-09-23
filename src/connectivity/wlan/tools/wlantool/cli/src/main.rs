// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use clap::Parser;
use fidl_fuchsia_wlan_device_service::DeviceMonitorMarker;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;

fn main() -> Result<(), Error> {
    println!(
        "Warning: this tool may cause state mismatches between layers of the WLAN \n\
        subsystem. It is intended for use by WLAN developers only. Please reach out \n\
        to the WLAN team if your use case relies on it."
    );
    let opt = wlan_dev::opts::Opt::parse();
    println!("{:?}", opt);

    let mut exec = fasync::LocalExecutorBuilder::new().build();
    let monitor_proxy = connect_to_protocol::<DeviceMonitorMarker>()
        .context("failed to `connect` to device monitor")?;

    let fut = wlan_dev::handle_wlantool_command(monitor_proxy, opt);
    exec.run_singlethreaded(fut)
}
