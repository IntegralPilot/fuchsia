// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use config::Config;
use fidl_next::fuchsia::create_channel;
use fidl_next_examples_keyvaluestore_additerator::{Item, Iterator, Store};
use fuchsia_component::client::fidl_next::connect_to_protocol;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    let config = Config::take_from_startup_handle();

    let store = connect_to_protocol::<Store>()?.spawn();
    println!("Outgoing connection enabled");

    for key in config.write_items.into_iter() {
        let path = format!("/pkg/data/{}.txt", key);
        let value = std::fs::read_to_string(path.clone())
            .with_context(|| format!("Failed to load {path}"))?;
        let item = Item { key: key, value: value.into_bytes() };
        match store.write_item(&item).await? {
            Ok(_) => println!("WriteItem Success"),
            Err(err) => println!("WriteItem Error: {:?}", err),
        }
    }

    if !config.iterate_from.is_empty() {
        let (iterator_client_end, iterator_server_end) = create_channel::<Iterator>();
        let iterator = iterator_client_end.spawn();

        let starting_at = Some(config.iterate_from);
        match store.iterate(starting_at.as_deref(), iterator_server_end).await? {
            Ok(_) => {
                println!("Iterator Connection Success");
                loop {
                    let entries = iterator.get().await?;
                    if entries.entries.is_empty() {
                        break;
                    }
                    for entry in entries.entries {
                        println!("Iterator Entry: {}", entry);
                    }
                }
            }
            Err(err) => println!("Iterator Connection Error: {:?}", err),
        }
    }

    Ok(())
}
