// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use config::Config;
use fidl_next_examples_keyvaluestore_supporttrees::{Item, NestedStore, Store, Value};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use std::{thread, time};

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Load the structured config values passed to this component at startup.
    let config = Config::take_from_startup_handle();

    // Use the Component Framework runtime to connect to the newly spun up server component.
    let store = connect_to_protocol::<Store>()?.spawn();
    println!("Outgoing connection enabled");

    // This client's structured config has one parameter, a vector of strings. Each string is the
    // path to a resource file whose filename is a key and whose contents are a value. We iterate
    // over them and try to write each key-value pair to the remote store.
    for key in config.write_items.into_iter() {
        let path = format!("/pkg/data/{}.txt", key);
        let value = std::fs::read_to_string(path.clone())
            .with_context(|| format!("Failed to load {path}"))?;
        let res = store
            .write_item(&Item {
                key: key.clone(),
                value: Some(Box::new(Value::Bytes(value.into_bytes()))),
            })
            .await?;
        match res {
            Ok(_) => println!("WriteItem Success at key: {}", key),
            Err(err) => println!("WriteItem Error: {:?}", err),
        }
    }

    // Add nested entries to the key-value store as well.
    for spec in config.write_nested.into_iter() {
        let mut items = vec![];
        let mut lines = spec.split("\n");
        let key = lines.next().unwrap();

        // For each entry, make a new entry in the `NestedStore` being built.
        for entry in lines {
            let path = format!("/pkg/data/{}.txt", entry);
            let contents = std::fs::read_to_string(path.clone())
                .with_context(|| format!("Failed to load {path}"))?;
            items.push(Some(Box::new(Item {
                key: entry.to_string(),
                value: Some(Box::new(Value::Bytes(contents.into()))),
            })));
        }
        let nested_store = NestedStore { items: Some(items) };

        // Send the `NestedStore`, represented as a vector of values.
        let res = store
            .write_item(&Item {
                key: key.to_string(),
                value: Some(Box::new(Value::Store(nested_store))),
            })
            .await?;
        match res {
            Ok(_) => println!("WriteItem Success at key: {}", key),
            Err(err) => println!("WriteItem Error: {:?}", err),
        }
    }

    // Each entry in this list is a null value in the store.
    for key in config.write_null.into_iter() {
        match store.write_item(&Item { key: key.to_string(), value: None }).await? {
            Ok(_) => println!("WriteItem Success at key: {}", key),
            Err(err) => println!("WriteItem Error: {:?}", err),
        }
    }

    // TODO(https://fxbug.dev/42156498): We need to sleep here to make sure all logs get drained. Once the
    // referenced bug has been resolved, we can remove the sleep.
    thread::sleep(time::Duration::from_secs(2));
    Ok(())
}
