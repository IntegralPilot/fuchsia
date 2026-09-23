// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use crate::common::{node_property_key_to_string, node_property_value_to_string};
use anyhow::{Result, anyhow};
use args::ListDevicesCommand;
use fidl_fuchsia_component_decl as fdecl;
use flex_fuchsia_driver_development as fdd;
use flex_fuchsia_driver_framework as fdf;
use fuchsia_driver_dev::Device;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use itertools::Itertools;

trait DevicePrinter {
    fn print(&self) -> Result<()>;
    fn print_verbose(&self) -> Result<()>;
}

impl DevicePrinter for Device {
    fn print(&self) -> Result<()> {
        println!("{}", self.get_moniker()?);
        Ok(())
    }

    fn print_verbose(&self) -> Result<()> {
        let moniker = self.get_moniker().expect("Node does not have a moniker");
        let (_, name) = moniker.rsplit_once('.').unwrap_or(("", &moniker));
        let topological_path = self.0.topological_path.as_deref().unwrap_or("None");
        println!("{0: <9}: {1}", "Name", name);
        println!("{0: <9}: {1}", "Moniker", moniker);
        println!("{0: <9}: {1}", "Topology", topological_path);

        if self.0.quarantined == Some(true)
            && self.0.bound_driver_url != Some("unbound".to_string())
        {
            println!(
                "{0: <9}: {1} (quarantined)",
                "Driver",
                self.0.bound_driver_url.as_deref().unwrap_or("None")
            );
        } else {
            println!(
                "{0: <9}: {1}",
                "Driver",
                self.0.bound_driver_url.as_deref().unwrap_or("None")
            );
        }

        if let Some(ref bus_topology) = self.0.bus_topology {
            if !bus_topology.is_empty() {
                println!("\n{0: <9} {1: <9} {2}", "Bus Type", "Stability", "Address",);
            }
            for segment in bus_topology {
                let address = match &segment.address {
                    Some(fdf::DeviceAddress::IntValue(val)) => format!("{val:02X}"),
                    Some(fdf::DeviceAddress::ArrayIntValue(val)) => {
                        val.iter().map(|v| format!("{v:02X}")).join(":")
                    }
                    Some(fdf::DeviceAddress::CharIntValue(val)) => val.to_string(),
                    Some(fdf::DeviceAddress::ArrayCharIntValue(val)) => {
                        val.iter().map(|v| v.to_string()).join(":")
                    }
                    Some(fdf::DeviceAddress::StringValue(val)) => val.to_string(),
                    None => "None".to_string(),
                    _ => "Unknown".to_string(),
                };
                println!(
                    "{0: <9} {1: <9} {2}",
                    segment
                        .bus
                        .map(|s| format!("{s:?}").to_uppercase())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    segment
                        .address_stability
                        .map(|s| format!("{s:?}"))
                        .unwrap_or_else(|| "Unknown".to_string()),
                    address,
                );
            }
            if !bus_topology.is_empty() {
                println!("");
            }
        }

        if let Some(ref node_property_list) = self.0.node_property_list {
            println!("{} Properties", node_property_list.len());
            for i in 0..node_property_list.len() {
                let node_property = &node_property_list[i];
                println!(
                    "[{:>2}/ {:>2}] : Key {:30} Value {}",
                    i + 1,
                    node_property_list.len(),
                    node_property_key_to_string(&node_property.key),
                    node_property_value_to_string(&node_property.value),
                );
            }
        } else {
            println!("0 Properties");
        }

        if let Some(ref offer_list) = self.0.offer_list {
            println!("{} Offers", offer_list.len());
            for i in 0..offer_list.len() {
                if let fdecl::Offer::Service(service) = &offer_list[i] {
                    println!("Service: {}", service.target_name.as_deref().unwrap_or("<unknown>"));
                    if let Some(fdecl::Ref::Child(source)) = service.source.as_ref() {
                        println!("  Source: {}", source.name);
                    }
                    if let Some(filter) = &service.source_instance_filter {
                        println!("  Instances: {}", filter.join(" "));
                    }
                }
            }
        } else {
            println!("0 Offers");
        }
        println!("");
        Ok(())
    }
}

pub async fn list_devices(
    cmd: ListDevicesCommand,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let devices: Vec<Device> = match cmd.device {
        Some(device) => {
            fuchsia_driver_dev::get_device_info(&driver_development_proxy, &[device], cmd.exact)
                .await?
        }
        None => {
            fuchsia_driver_dev::get_device_info(&driver_development_proxy, &[], cmd.exact).await?
        }
    }
    .into_iter()
    .filter(|d| !cmd.unbound || d.bound_driver_url == Some("unbound".to_string()))
    .map(|device_info| Device::from(device_info))
    .collect();

    if !devices.is_empty() {
        if cmd.verbose {
            for device in devices {
                device.print_verbose()?;
            }
        } else {
            for device in devices {
                device.print()?;
            }
        }
    } else {
        if cmd.fail_on_missing {
            return Err(anyhow!("No devices found."));
        } else {
            println!("No devices found.");
        }
    }

    Ok(())
}
