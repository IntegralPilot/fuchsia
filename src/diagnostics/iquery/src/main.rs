// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_diagnostics_system::SerialLogControlMarker;
use fidl_fuchsia_sys2 as fsys2;
use fuchsia_component::client;
use iquery::command_line::CommandLine;
use iquery::commands::{ArchiveAccessorProvider, Command};

#[cfg(test)]
#[macro_use]
mod tests;

const ROOT_REALM_QUERY: &str = "/svc/fuchsia.sys2.RealmQuery.root";

async fn maybe_pause_serial() -> Result<zx::EventPair, Error> {
    let freezer = client::connect_to_protocol::<SerialLogControlMarker>()?;
    Ok(freezer.freeze_serial_forwarding().await?)
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let command_line: CommandLine = argh::from_env();
    let token = if command_line.serial_tag.as_ref().is_some() {
        maybe_pause_serial().await.ok()
    } else {
        None
    };
    if token.is_none() && command_line.serial_tag.is_some() {
        return Err(anyhow::anyhow!("serial-tag is not supported in this environment."));
    }
    if let Some(tag) = &command_line.serial_tag {
        eprintln!("Serial logging frozen: {tag}");
    }
    let realm_query =
        client::connect_to_protocol_at_path::<fsys2::RealmQueryMarker>(ROOT_REALM_QUERY)
            .expect("can connect to the root realm query");
    let provider = ArchiveAccessorProvider::new(realm_query);
    let maybe_serial_tag = command_line.serial_tag.clone();
    match command_line.execute(&provider).await {
        Ok(result) => {
            println!("{result}");
            if let Some(tag) = &maybe_serial_tag {
                eprintln!("Serial logging unfrozen: {tag}");
            }
        }
        Err(err) => {
            eprintln!("{err}");
            if let Some(tag) = &maybe_serial_tag {
                eprintln!("Serial logging unfrozen: {tag}");
            }
            std::process::exit(1);
        }
    }
    Ok(())
}
