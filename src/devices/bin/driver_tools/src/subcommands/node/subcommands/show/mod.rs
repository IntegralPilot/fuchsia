// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use crate::subcommands::node::common;

use anyhow::{Result, bail};
use args::ShowNodeCommand;
use flex_fuchsia_driver_development as fdd;
use flex_fuchsia_driver_framework as fdf;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use itertools::Itertools;
use prettytable::format::FormatBuilder;
use prettytable::{Table, row};
use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
pub struct NodeDetails {
    pub name: String,
    pub moniker: String,
    pub owner: String,
    pub state: String,
    pub host_koid: String,
    pub parent_count: usize,
    pub child_count: usize,
    pub bus_topology: Vec<BusTopology>,
    pub properties: Vec<NodeProperty>,
    pub offers: Vec<NodeOffer>,
    // TODO(https://fxbug.dev/500119481): Remove this field once all clients are
    // migrated away from devfs.
    pub topological_path: String,
}

#[derive(Serialize)]
pub struct BusTopology {
    pub bus_type: String,
    pub stability: String,
    pub address: String,
}

#[derive(Serialize)]
pub struct NodeProperty {
    pub key: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct NodeOffer {
    pub service: String,
    pub source: String,
    pub instances: String,
}

pub async fn get_node_details(
    cmd: &ShowNodeCommand,
    driver_development_proxy: &fdd::ManagerProxy,
) -> Result<Vec<NodeDetails>> {
    let nodes = fuchsia_driver_dev::get_device_info(driver_development_proxy, &[], false).await?;
    let matching_nodes = common::get_nodes_from_query(&cmd.query, nodes).await?;

    if matching_nodes.is_empty() {
        bail!("No matching node found for query {:?}.", cmd.query);
    }

    let mut details_list = Vec::new();
    for node in matching_nodes {
        details_list.push(make_node_details(node));
    }
    Ok(details_list)
}

fn make_node_details(node: fdd::NodeInfo) -> NodeDetails {
    // No style for machine output
    let (state, owner) =
        common::get_state_and_owner(node.quarantined, &node.bound_driver_url, false);

    let moniker = node.moniker.clone().expect("Node does not have a moniker");
    let (_, name) = moniker.rsplit_once('.').unwrap_or(("", &moniker));

    let bus_topology = node
        .bus_topology
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|b| BusTopology {
            bus_type: b.bus.map(|b| format!("{:?}", b)).unwrap_or_else(|| "No bus".to_string()),
            stability: b
                .address_stability
                .map(|a| format!("{:?}", a))
                .unwrap_or_else(|| "No stability".to_string()),
            address: b
                .address
                .map(|a| match a {
                    fdf::DeviceAddress::IntValue(i) => {
                        format!("{}", i)
                    }
                    fdf::DeviceAddress::ArrayIntValue(items) => {
                        format!("{}", items.iter().map(|u| u.to_string()).join(", "))
                    }
                    fdf::DeviceAddress::CharIntValue(c) => {
                        format!("{}", c)
                    }
                    fdf::DeviceAddress::ArrayCharIntValue(items) => {
                        format!("{}", items.join(", "))
                    }
                    fdf::DeviceAddress::StringValue(s) => {
                        format!("{}", s)
                    }
                    _ => format!("unknown"),
                })
                .unwrap_or_else(|| "No Address".to_string()),
        })
        .collect();

    let properties = node
        .node_property_list
        .clone()
        .unwrap_or_else(|| vec![])
        .into_iter()
        .map(|p| {
            let key = match p.key {
                fdf::NodePropertyKey::IntValue(i) => format!("{}", i),
                fdf::NodePropertyKey::StringValue(s) => format!("{}", s),
            };
            let value = match p.value {
                fdf::NodePropertyValue::IntValue(i) => format!("{}", i),
                fdf::NodePropertyValue::StringValue(s) => {
                    format!("{}", s)
                }
                fdf::NodePropertyValue::BoolValue(b) => format!("{}", b),
                fdf::NodePropertyValue::EnumValue(e) => format!("{}", e),
                _ => format!("unknown"),
            };
            NodeProperty { key, value }
        })
        .collect();

    let offers = node
        .offer_list
        .clone()
        .unwrap_or_else(|| vec![])
        .into_iter()
        .map(|o| {
            if let fidl_fuchsia_component_decl::Offer::Service(service) = o {
                let service_str = service.target_name.unwrap_or_else(|| "<unknown>".to_string());

                let source_name = if let Some(fidl_fuchsia_component_decl::Ref::Child(source)) =
                    service.source.as_ref()
                {
                    source.name.clone()
                } else {
                    "Unknown source".to_string()
                };

                let filter = if let Some(filter) = &service.source_instance_filter {
                    filter.join(", ")
                } else {
                    "All instances".to_string()
                };
                NodeOffer { service: service_str, source: source_name, instances: filter }
            } else {
                NodeOffer {
                    service: "Non-service offer".to_string(),
                    source: "".to_string(),
                    instances: "".to_string(),
                }
            }
        })
        .collect();

    NodeDetails {
        name: name.to_string(),
        moniker,
        owner,
        state,
        host_koid: node.driver_host_koid.map(|k| format!("{}", k)).unwrap_or_default(),
        parent_count: node.parent_ids.as_ref().map(|ids| ids.len()).unwrap_or(0),
        child_count: node.child_ids.as_ref().map(|ids| ids.len()).unwrap_or(0),
        bus_topology,
        properties,
        offers,
        topological_path: node.topological_path.unwrap_or_default(),
    }
}

pub async fn show_node(
    cmd: ShowNodeCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let nodes = fuchsia_driver_dev::get_device_info(&driver_development_proxy, &[], false).await?;
    let matching_nodes = common::get_nodes_from_query(&cmd.query, nodes).await?;

    if matching_nodes.is_empty() {
        bail!("No matching node found for query {:?}.", cmd.query);
    }

    let with_style = termion::is_tty(&std::io::stdout());

    for (i, node) in matching_nodes.into_iter().enumerate() {
        if i > 0 {
            writeln!(writer, "\n--------------------------------------------------\n")?;
        }
        print_table(node, with_style, writer)?;
    }

    Ok(())
}

fn print_table(node: fdd::NodeInfo, with_style: bool, writer: &mut dyn Write) -> Result<()> {
    let mut table = Table::new();
    table.set_format(FormatBuilder::new().padding(2, 0).build());

    let parent_count = node.parent_ids.map(|ids| ids.len()).unwrap_or(0);
    let children_count = node.child_ids.map(|ids| ids.len()).unwrap_or(0);
    let moniker = node.moniker.expect("Node does not have a moniker");
    let (_, name) = moniker.rsplit_once('.').unwrap_or(("", &moniker));
    let (state, owner) =
        common::get_state_and_owner(node.quarantined, &node.bound_driver_url, with_style);

    let bus_topo = node
        .bus_topology
        .unwrap_or_default()
        .into_iter()
        .map(|b| {
            (
                b.bus.map(|b| format!("{:?}", b)).unwrap_or_else(|| "No bus".to_string()),
                b.address_stability
                    .map(|a| format!("{:?}", a))
                    .unwrap_or_else(|| "No stability".to_string()),
                b.address
                    .map(|a| match a {
                        fdf::DeviceAddress::IntValue(i) => {
                            format!("{}", i)
                        }
                        fdf::DeviceAddress::ArrayIntValue(items) => {
                            format!("{}", items.iter().map(|u| u.to_string()).join(", "))
                        }
                        fdf::DeviceAddress::CharIntValue(c) => {
                            format!("{}", c)
                        }
                        fdf::DeviceAddress::ArrayCharIntValue(items) => {
                            format!("{}", items.join(", "))
                        }
                        fdf::DeviceAddress::StringValue(s) => {
                            format!("{}", s)
                        }
                        _ => format!("unknown"),
                    })
                    .unwrap_or_else(|| "No Address".to_string()),
            )
        })
        .collect::<Vec<_>>();

    let koid = node.driver_host_koid.map(|k| format!("{}", k)).unwrap_or_default();

    let props = node
        .node_property_list
        .unwrap_or_else(|| vec![])
        .into_iter()
        .map(|p| {
            let key = match p.key {
                fdf::NodePropertyKey::IntValue(i) => format!("{}", i),
                fdf::NodePropertyKey::StringValue(s) => format!("{}", s),
            };
            let value = match p.value {
                fdf::NodePropertyValue::IntValue(i) => format!("{}", i),
                fdf::NodePropertyValue::StringValue(s) => {
                    format!("{}", s)
                }
                fdf::NodePropertyValue::BoolValue(b) => format!("{}", b),
                fdf::NodePropertyValue::EnumValue(e) => format!("{}", e),
                _ => format!("unknown"),
            };
            (key, value)
        })
        .collect::<Vec<_>>();

    let offers = node
        .offer_list
        .unwrap_or_else(|| vec![])
        .into_iter()
        .map(|o| {
            if let fidl_fuchsia_component_decl::Offer::Service(service) = o {
                let service_str = service.target_name.unwrap_or_else(|| "<unknown>".to_string());

                let source_name = if let Some(fidl_fuchsia_component_decl::Ref::Child(source)) =
                    service.source.as_ref()
                {
                    source.name.clone()
                } else {
                    "Unknown source".to_string()
                };

                let filter = if let Some(filter) = &service.source_instance_filter {
                    filter.join(", ")
                } else {
                    "All instances".to_string()
                };
                (service_str, source_name, filter)
            } else {
                ("Non-service offer".to_string(), "".to_string(), "".to_string())
            }
        })
        .collect::<Vec<_>>();

    table.add_row(row!(r->"Name:", name));
    table.add_row(row!(r->"Moniker:", moniker));
    let topo_path = node.topological_path.unwrap_or_default();
    table.add_row(row!(r->"Topological Path:", topo_path));
    table.add_row(row!(r->"Owner:", owner));
    table.add_row(row!(r->"Node State:", state));
    table.add_row(row!(r->"Host Koid:", koid));
    table.add_row(row!(r->"Parent Count:", parent_count));
    table.add_row(row!(r->"Child Count:", children_count));
    table.add_empty_row();
    table.print(writer)?;

    if !bus_topo.is_empty() {
        table = Table::new();
        table.set_format(FormatBuilder::new().padding(2, 0).build());
        table.set_titles(row!("Bus Topology:", "Bus Type", "Stability", "Address"));
        for topo in bus_topo {
            table.add_row(row!("", topo.0, topo.1, topo.2));
        }
        table.add_empty_row();
        table.print(writer)?;
    }

    if !props.is_empty() {
        table = Table::new();
        table.set_format(FormatBuilder::new().padding(2, 0).build());
        table.set_titles(row!("Node Properties:", "Key", "Value"));
        for prop in props {
            table.add_row(row!("", prop.0, prop.1));
        }
        table.add_empty_row();
        table.print(writer)?;
    }

    if !offers.is_empty() {
        table = Table::new();
        table.set_format(FormatBuilder::new().padding(2, 0).build());
        table.set_titles(row!("Node Offers:", "Service", "Source", "Instances"));
        for offer in offers {
            table.add_row(row!("", offer.0, offer.1, offer.2));
        }
        table.add_empty_row();
        table.print(writer)?;
    }

    Ok(())
}
