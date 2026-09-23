// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use fidl_fuchsia_thermal as fthermal;
use fuchsia_component::client::connect_to_protocol;
use std::env;

fn print_usage() {
    println!("Usage: thermal-sensor-manager-cli <command> [<args>]");
    println!("Commands:");
    println!("  list                         List all thermal sensors.");
    println!("  read <sensor_name>           Get current temperature of <sensor_name>.");
    println!(
        "  override <sensor_name> <temp> Set override temperature in Celsius for <sensor_name>."
    );
    println!("  clear <sensor_name>          Clear override temperature for <sensor_name>.");
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        bail!("Missing command");
    }

    let command = &args[1];

    // Connect to SensorManager. Since we may run inside the sandbox shell of the
    // component that exposes the protocol (via `component explore`), we check for
    // the exposed protocol in `/ns/out/svc/` or `/out/svc/` first, before falling
    // back to the standard `/svc/` path.
    let sensor_manager =
        if std::path::Path::new("/ns/out/svc/fuchsia.thermal.SensorManager").exists() {
            fuchsia_component::client::connect_to_protocol_at_path::<fthermal::SensorManagerMarker>(
                "/ns/out/svc/fuchsia.thermal.SensorManager",
            )
        } else if std::path::Path::new("/out/svc/fuchsia.thermal.SensorManager").exists() {
            fuchsia_component::client::connect_to_protocol_at_path::<fthermal::SensorManagerMarker>(
                "/out/svc/fuchsia.thermal.SensorManager",
            )
        } else {
            connect_to_protocol::<fthermal::SensorManagerMarker>()
        }
        .context("Failed to connect to fuchsia.thermal.SensorManager")?;

    match command.as_str() {
        "list" => {
            let sensors =
                sensor_manager.list_sensors().await.context("Failed to call ListSensors")?;
            println!("Sensors found: {}", sensors.len());
            for sensor in sensors {
                let name = sensor.name.unwrap_or_else(|| "<unknown>".to_string());
                println!("  {}", name);
            }
        }
        "read" => {
            if args.len() < 3 {
                bail!("Missing sensor_name. Usage: read <sensor_name>");
            }
            let name = &args[2];
            let (device_proxy, device_server) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_temperature::DeviceMarker>();
            let connect_request = fthermal::SensorManagerConnectRequest {
                name: Some(name.clone()),
                server_end: Some(fthermal::SensorServer_::Temperature(device_server)),
                ..Default::default()
            };
            sensor_manager
                .connect(connect_request)
                .await
                .context("Failed to call Connect")?
                .map_err(|e| anyhow::anyhow!("Connect returned error: {:?}", e))?;

            let (status, temp) = device_proxy
                .get_temperature_celsius()
                .await
                .context("Failed to call GetTemperatureCelsius")?;
            if let Err(status) = zx::Status::ok(status) {
                bail!("Sensor '{}' returned error status: {status:?}", name);
            } else {
                println!("Sensor '{}' temperature: {:.2}°C", name, temp);
            }
        }
        "override" => {
            if args.len() < 4 {
                bail!("Missing arguments. Usage: override <sensor_name> <temperature>");
            }
            let name = &args[2];
            let temp_val: f32 = args[3].parse().context("Failed to parse temperature as float")?;
            sensor_manager
                .set_temperature_override(name, temp_val)
                .await
                .context("Failed to call SetTemperatureOverride")?
                .map_err(|e| anyhow::anyhow!("SetTemperatureOverride returned error: {:?}", e))?;
            println!("Successfully set override for '{}' to {:.2}°C", name, temp_val);
        }
        "clear" => {
            if args.len() < 3 {
                bail!("Missing sensor_name. Usage: clear <sensor_name>");
            }
            let name = &args[2];
            sensor_manager
                .clear_temperature_override(name)
                .await
                .context("Failed to call ClearTemperatureOverride")?
                .map_err(|e| anyhow::anyhow!("ClearTemperatureOverride returned error: {:?}", e))?;
            println!("Successfully cleared override for '{}'", name);
        }
        _ => {
            print_usage();
            bail!("Unknown command '{}'", command);
        }
    }

    Ok(())
}
