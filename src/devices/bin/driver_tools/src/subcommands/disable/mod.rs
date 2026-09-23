// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Result, format_err};
use args::DisableCommand;
use flex_fuchsia_driver_development as fdd;
use std::io::Write;
use zx_status::Status;

pub async fn disable(
    cmd: DisableCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    writeln!(writer, "Disabling {} and restarting driver hosts with rematching enabled.", cmd.url)?;

    let result = driver_development_proxy.disable_driver(&cmd.url, None).await?;
    match result {
        Ok(_) => {
            writeln!(writer, "Disabled driver successfully.")?;
        }
        Err(e) => {
            if e == Status::NOT_FOUND.into_raw() {
                writeln!(writer, "No drivers affected in this disable operation.")?;
            } else {
                writeln!(writer, "Unexpected error from disable: {}", e)?;
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
                    "Successfully restarted. Rematched {} driver hosts that had the disabled driver.",
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

    Ok(())
}
