// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fidl_fuchsia_hardware_vreg as fvreg;
use fuchsia_component::client::connect_to_protocol_at_path;

#[derive(FromArgs, Debug)]
/// Vreg driver control.
struct VregCtlArgs {
    #[argh(switch, short = 'l')]
    /// lists all available vreg instances.
    list: bool,

    #[argh(option, short = 'i')]
    /// instance name in the service directory path specified by service_dir.
    instance: Option<String>,

    #[argh(option, short = 's')]
    /// service directory path, defaults to /exposed/fuchsia.hardware.vreg.Service.
    service_dir: Option<String>,

    #[argh(switch)]
    /// obtain parameters for the voltage regulator.
    params: bool,

    #[argh(switch)]
    /// enable the voltage regulator.
    enable: bool,

    #[argh(switch)]
    /// disable the voltage regulator.
    disable: bool,

    #[argh(option)]
    /// set the voltage step.
    set_voltage_step: Option<u32>,

    #[argh(switch)]
    /// get the current voltage step.
    get_voltage_step: bool,
}

// Default to /exposed/... so that running the CLI via `component explore`
// does not require specifying the path.
const DEFAULT_VREG_SERVICE_DIR: &str = "/exposed/fuchsia.hardware.vreg.Service";

async fn list_devices(service_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !std::path::Path::new(service_dir).exists() {
        println!("Folder {} not found", service_dir);
        return Ok(());
    }

    println!("Voltage regulators found:");
    for entry in std::fs::read_dir(service_dir)? {
        let entry = entry?;
        println!("- {}", entry.file_name().to_string_lossy());
    }

    Ok(())
}

async fn get_vreg_client(
    service_dir: &str,
    instance_name: &str,
) -> Result<fvreg::VregProxy, Box<dyn std::error::Error>> {
    let path = format!("{}/{}/vreg", service_dir, instance_name);
    let proxy = connect_to_protocol_at_path::<fvreg::VregMarker>(&path)?;
    Ok(proxy)
}

#[fuchsia::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: VregCtlArgs = argh::from_env();
    let service_dir = args.service_dir.as_deref().unwrap_or(DEFAULT_VREG_SERVICE_DIR);

    if args.list {
        list_devices(service_dir).await?;
        return Ok(());
    }

    let instance_name = match args.instance {
        Some(name) => name,
        None => {
            println!("Must specify instance name with -i or --instance");
            return Ok(());
        }
    };

    let vreg = get_vreg_client(service_dir, &instance_name).await?;

    if args.params {
        match vreg.get_regulator_params().await {
            Ok(Ok(params)) => {
                println!("Min UV: {}", params.0);
                println!("Step Size UV: {}", params.1);
                println!("Num Steps: {}", params.2);
            }
            Ok(Err(status)) => println!("GetRegulatorParams failed: {}", status),
            Err(e) => println!("FIDL error: {}", e),
        }
    }

    if args.enable {
        match vreg.enable().await {
            Ok(Ok(())) => println!("Enable success"),
            Ok(Err(status)) => println!("Enable failed: {}", status),
            Err(e) => println!("FIDL error: {}", e),
        }
    }

    if args.disable {
        match vreg.disable().await {
            Ok(Ok(())) => println!("Disable success"),
            Ok(Err(status)) => println!("Disable failed: {}", status),
            Err(e) => println!("FIDL error: {}", e),
        }
    }

    if let Some(step) = args.set_voltage_step {
        match vreg.set_voltage_step(step).await {
            Ok(Ok(())) => println!("SetVoltageStep success"),
            Ok(Err(status)) => println!("SetVoltageStep failed: {}", status),
            Err(e) => println!("FIDL error: {}", e),
        }
    }

    if args.get_voltage_step {
        match vreg.get_voltage_step().await {
            Ok(Ok(step)) => println!("Current Step: {}", step),
            Ok(Err(status)) => println!("GetVoltageStep failed: {}", status),
            Err(e) => println!("FIDL error: {}", e),
        }
    }

    Ok(())
}
