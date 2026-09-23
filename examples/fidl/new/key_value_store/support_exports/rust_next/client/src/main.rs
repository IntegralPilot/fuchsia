// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use config::Config;
use fidl::unpersist;
use fidl_examples_keyvaluestore_supportexports as fidl_legacy;
use fidl_next_examples_keyvaluestore_supportexports::{Item, Store};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use std::{thread, time};
use zx::Vmo;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Load the structured config values passed to this component at startup.
    let config = Config::take_from_startup_handle();

    // Use the Component Framework runtime to connect to the newly spun up server component.
    let client = connect_to_protocol::<Store>()?.spawn();
    println!("Outgoing connection enabled");

    // This client's structured config has one parameter, a vector of strings. Each string is the
    // path to a resource file whose filename is a key and whose contents are a value. We iterate
    // over them and try to write each key-value pair to the remote store.
    for key in config.write_items.into_iter() {
        let path = format!("/pkg/data/{}.txt", key);
        let value = std::fs::read_to_string(path.clone())
            .with_context(|| format!("Failed to load {path}"))?;
        let item = Item { key: key, value: value.into_bytes() };
        match client.write_item(&item).await? {
            Ok(_) => println!("WriteItem Success"),
            Err(err) => println!("WriteItem Error: {:?}", err),
        }
    }

    // If the `max_export_size` is 0, no export is possible, so just ignore this block. This check
    // isn't strictly necessary, but does avoid extra work down the line.
    if config.max_export_size > 0 {
        // Create a VMO to store the resulting export.
        let vmo = Vmo::create(config.max_export_size)?;

        // Send the VMO to the server, to be populated with the current state of the key-value
        // store.
        match client.export(vmo).await? {
            Err(err) => {
                println!("Export Error: {:?}", err);
            }
            Ok(output) => {
                println!("Export Success");

                // Read the exported data (encoded in byte form as persistent FIDL) from the
                // returned VMO.
                let content_size = output.filled.get_content_size().unwrap();
                let mut encoded_bytes = vec![0; content_size as usize];
                output.filled.read(&mut encoded_bytes, 0)?;

                // Decode the persistent FIDL that was just read from the file.
                // We use the legacy bindings for this part as it's just data decoding.
                let exportable = unpersist::<fidl_legacy::Exportable>(&encoded_bytes).unwrap();
                let items = exportable.items.expect("must always be set");

                // Log some information about the exported data.
                println!("Printing {} exported entries, which are:", items.len());
                for item in items.iter() {
                    println!("  * {}", item.key);
                }
            }
        };
    }

    // TODO(https://fxbug.dev/42156498): We need to sleep here to make sure all logs get drained. Once the
    // referenced bug has been resolved, we can remove the sleep.
    thread::sleep(time::Duration::from_secs(2));
    Ok(())
}
