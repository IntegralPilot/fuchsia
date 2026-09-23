// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Result, format_err};
use args::RegisterCommand;
use flex_fuchsia_driver_development as fdd;
use flex_fuchsia_driver_registrar as fdr;
use std::io::Write;
use zx_status::Status;

pub async fn register(
    cmd: RegisterCommand,
    writer: &mut dyn Write,
    driver_registrar_proxy: fdr::DriverRegistrarProxy,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    writeln!(
        writer,
        "Registering {}, restarting driver hosts, and attempting to bind to unbound nodes",
        cmd.url
    )?;
    let register_result = driver_registrar_proxy.register(&cmd.url).await?;

    match register_result {
        Ok(_) => {}
        Err(e) => {
            return Err(format_err!("Failed to register driver: {}", e));
        }
    }

    let mut existing = false;
    let restart_result = driver_development_proxy
        .restart_driver_hosts(cmd.url.as_str(), fdd::RestartRematchFlags::empty())
        .await?;
    match restart_result {
        Ok(count) => {
            if count > 0 {
                existing = true;
                writeln!(writer, "Successfully restarted {} driver hosts with the driver.", count)?;
            }
        }
        Err(err) => {
            return Err(format_err!(
                "Failed to restart existing drivers: {:?}",
                Status::err_from_raw(err)
            ));
        }
    }

    let bind_result = driver_development_proxy.bind_all_unbound_nodes2().await?;

    match bind_result {
        Ok(result) => {
            if result.is_empty() {
                if !existing {
                    writeln!(
                        writer,
                        "{}\n{}",
                        "There are no existing driver hosts with this driver.",
                        "No new nodes were bound to the driver being registered.",
                    )?;
                }
            } else {
                writeln!(writer, "Successfully bound:")?;
                for info in result {
                    writeln!(
                        writer,
                        "Node '{}':\nDriver '{:#?}'\nComposite Specs '{:#?}'",
                        info.node_name.unwrap_or_else(|| "<NA>".to_string()),
                        info.driver_url,
                        info.composite_parents,
                    )?;
                }
            }
        }
        Err(err) => {
            return Err(format_err!("Failed to bind nodes: {:?}", Status::err_from_raw(err)));
        }
    };
    Ok(())
}
