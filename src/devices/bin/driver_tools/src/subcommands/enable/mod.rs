// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Result, format_err};
use args::EnableCommand;
use flex_fuchsia_driver_development as fdd;
use std::io::Write;
use zx_status::Status;

pub async fn enable(
    cmd: EnableCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    writeln!(writer, "Enabling {}.", cmd.url)?;

    let result = driver_development_proxy.enable_driver(&cmd.url, None).await?;
    match result {
        Ok(_) => {
            writeln!(writer, "Enabled driver successfully.")?;
        }
        Err(e) => {
            if e == Status::NOT_FOUND.into_raw() {
                writeln!(writer, "No drivers affected in this enable operation.")?;
            } else {
                writeln!(writer, "Unexpected error from enable: {}", e)?;
            }
        }
    }

    let rebind_result = driver_development_proxy.rebind_composites_with_driver(&cmd.url).await?;
    match rebind_result {
        Ok(count) => {
            if count > 0 {
                writeln!(writer, "Rebound {count} composites successfully.")?;
            } else {
                writeln!(writer, "No composites affected in this operation.")?;
            }
        }
        Err(e) => {
            writeln!(writer, "Unexpected error from rebind: {}", e)?;
        }
    }

    let restart_result = driver_development_proxy
        .restart_driver_hosts(
            cmd.url.as_str(),
            fdd::RestartRematchFlags::REQUESTED | fdd::RestartRematchFlags::COMPOSITE_SPEC,
        )
        .await?;

    match restart_result {
        Ok(count) => {
            if count > 0 {
                writeln!(
                    writer,
                    "Successfully restarted. Rematched {} driver hosts that had the enabled driver.",
                    count
                )?;
            } else {
                writeln!(writer, "{}", "Successfully restarted.")?;
            }
        }
        Err(err) => {
            return Err(format_err!(
                "Failed to restart existing drivers: {:?}",
                Status::err_from_raw(err)
            ));
        }
    }

    writeln!(writer, "Attempting to bind unbound nodes...")?;
    let bind_result = driver_development_proxy.bind_all_unbound_nodes2().await?;
    match bind_result {
        Ok(result) => {
            let count = result.len();
            writeln!(writer, "Bound {count} nodes successfully.")?;
        }
        Err(e) => {
            writeln!(writer, "Unexpected error from bind_all_unbound_nodes: {}", e)?;
        }
    }

    Ok(())
}
