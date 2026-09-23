// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use fdomain_fuchsia_bluetooth::HostId as FidlHostId;
use fdomain_fuchsia_bluetooth_affordances::HostControllerProxy;
use fdomain_fuchsia_bluetooth_sys::{AccessProxy, HostWatcherProxy};
use ffx_bluetooth_controller_args::{ControllerCommand, ControllerSubCommand};
use ffx_writer::{SimpleWriter, ToolIO as _};
use fuchsia_bluetooth::types::{HostId, HostInfo, addresses_to_custom_string};
use prettytable::format::FormatBuilder;
use prettytable::{Table, row};
use target_holders::toolbox;

pub mod device_class;
pub mod local_name;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct ControllerTool {
    #[command]
    cmd: ControllerCommand,
    #[with(toolbox())]
    host_controller: HostControllerProxy,
    #[with(toolbox())]
    host_watcher_proxy: HostWatcherProxy,
    #[with(toolbox())]
    access_proxy: AccessProxy,
}

fho::embedded_plugin!(ControllerTool);
#[async_trait(?Send)]
impl FfxMain for ControllerTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let hosts = self.get_hosts().await?;
        match self.cmd.subcommand {
            // ffx bluetooth controller show
            ControllerSubCommand::Show(ref _cmd) => {
                if let Some(host) = hosts.iter().find(|h| h.active) {
                    writer.line(host.to_string())?;
                } else {
                    writer.line("No controller found.")?;
                }
            }
            // ffx bluetooth controller list
            ControllerSubCommand::List(ref _cmd) => {
                writer.line(get_hosts_list(&hosts).unwrap())?;
            }
            // ffx bluetooth controller set
            ControllerSubCommand::Set(ref cmd) => {
                self.set_active_host(cmd.id).await?;
                writer.line(format!("Active host set to {}", cmd.id))?;
            }
            // ffx bluetooth controller local-name
            ControllerSubCommand::LocalName(ref cmd) => {
                local_name::handle_local_name(&self, cmd, &mut writer, &hosts).await?;
            }
            // ffx bluetooth controller device-class
            ControllerSubCommand::DeviceClass(ref cmd) => {
                device_class::handle_device_class(&self, cmd, &mut writer).await?;
            }
        }
        Ok(())
    }
}

impl ControllerTool {
    async fn get_hosts(&self) -> Result<Vec<HostInfo>> {
        let response = self
            .host_controller
            .get_hosts()
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.affordances.HostController error: {err:?}"
                ))
            })?;
        let hosts = response.hosts.unwrap_or_default();

        Ok(hosts
            .into_iter()
            .map(|host| HostInfo::try_from(host).expect("Failed to convert between Host types"))
            .collect())
    }

    async fn set_active_host(&self, host_id: HostId) -> Result<()> {
        let fidl_host_id: FidlHostId = host_id.into();
        Ok(self
            .host_watcher_proxy
            .set_active(&fidl_host_id)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.sys.HostWatcher/SetActive error: {err:?}"
                ))
            })?)
    }
}

fn get_hosts_list(hosts: &Vec<HostInfo>) -> Result<String> {
    if hosts.is_empty() {
        return Ok(String::from("No controllers detected"));
    }
    // Create table of results
    let mut table = Table::new();
    let table_format = FormatBuilder::new().padding(/*left*/ 0, /*right*/ 1).build();
    table.set_format(table_format);
    let _ = table.set_titles(row![
        "HostId",
        "Addresses",
        "Active",
        "Technology",
        "Name",
        "Discoverable",
        "Discovering",
    ]);
    for host in hosts {
        let _ = table.add_row(row![
            host.id.to_string(),
            addresses_to_custom_string(&host.addresses, "\n"),
            host.active.to_string(),
            format!("{:?}", host.technology),
            host.local_name.clone().unwrap_or_else(|| "(unknown)".to_string()),
            host.discoverable.to_string(),
            host.discovering.to_string(),
        ]);
    }
    Ok(format!("{}", table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_fuchsia_bluetooth_sys::TechnologyType;
    use fuchsia_bluetooth::types::{Address, HostId};
    use regex_lite::Regex;

    fn custom_host(
        id: HostId,
        address: Address,
        active: bool,
        discoverable: bool,
        discovering: bool,
        name: Option<String>,
    ) -> HostInfo {
        HostInfo {
            id,
            technology: TechnologyType::LowEnergy,
            addresses: vec![address],
            active,
            local_name: name,
            discoverable,
            discovering,
        }
    }

    #[test]
    fn test_get_hosts_list() {
        // Fields for table view of hosts
        let fields = Regex::new(r"HostId[ \t]+Addresses[ \t]+Active[ \t]+Technology[ \t]+Name[ \t]+Discoverable[ \t]+Discovering").unwrap();

        // No hosts
        let output = get_hosts_list(&vec![]).unwrap();
        assert!(!fields.is_match(&output));
        assert!(output.contains("No controllers detected"));

        let hosts = vec![
            custom_host(
                HostId(0xbeef),
                Address::Public([0x11, 0x00, 0x55, 0x7E, 0xDE, 0xAD]),
                false,
                false,
                false,
                Some("Sapphire".to_string()),
            ),
            custom_host(
                HostId(0xabcd),
                Address::Random([0x22, 0x00, 0x55, 0x7E, 0xDE, 0xAD]),
                false,
                false,
                true,
                None,
            ),
        ];

        // Hosts exist
        let output = get_hosts_list(&hosts).unwrap();
        assert!(fields.is_match(&output));
        assert!(output.contains("ef"));
        assert!(output.contains("cd"));
    }
}
