// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use config::Config;
use fidl_next_examples_keyvaluestore_usegenericvalues::{
    Item, Store, StoreWriteItemRequest, Value, WriteOptions,
};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use std::{thread, time};

// A helper function to sequentially write a single item to the key-value store and print a log when
// successful.
async fn write_next_item(
    store: &fidl_next::Client<Store>,
    key: &str,
    value: Value,
    options: WriteOptions,
) -> Result<(), Error> {
    // Create an empty request payload using `::default()`.
    let mut req = StoreWriteItemRequest::default();
    req.options = Some(options);

    // Fill in the `Item` we will be attempting to write.
    println!("WriteItem request sent: key: {}, value: {:?}", key, value);
    req.attempt = Some(Item { key: key.to_string(), value: value });

    // Send and async `WriteItem` request to the server.
    match store.write_item_with(&req).await.context("Error sending request")? {
        Ok(value) => println!("WriteItem response received: {:?}", value),
        Err(err) => println!("WriteItem Error: {:?}", err),
    }
    Ok(())
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Load the structured config values passed to this component at startup.
    let config = Config::take_from_startup_handle();

    // Use the Component Framework runtime to connect to the newly spun up server component.
    let store = connect_to_protocol::<Store>()?.spawn();
    println!("Outgoing connection enabled");

    // All of our requests will have the same bitflags set. Pull these settings from the config.
    let mut options = WriteOptions::empty();
    options.set(WriteOptions::OVERWRITE, config.set_overwrite_option);
    options.set(WriteOptions::CONCAT, config.set_concat_option);

    // The structured config provides one input for most data types that can be stored in the data
    // store. Iterate through those inputs in the order we see them in the FIDL file.
    for value in config.write_bytes.into_iter() {
        write_next_item(&store, "bytes", Value::Bytes(value.into()), options).await?;
    }
    for value in config.write_strings.into_iter() {
        write_next_item(&store, "string", Value::String(value), options).await?;
    }
    for value in config.write_uint64s.into_iter() {
        write_next_item(&store, "uint64", Value::Uint64(value), options).await?;
    }
    for value in config.write_int64s.into_iter() {
        write_next_item(&store, "int64", Value::Int64(value), options).await?;
    }
    for value in config.write_uint128s.into_iter() {
        write_next_item(&store, "uint128", Value::Uint128([0, value]), options).await?;
    }

    // TODO(https://fxbug.dev/42156498): We need to sleep here to make sure all logs get drained. Once the
    // referenced bug has been resolved, we can remove the sleep.
    thread::sleep(time::Duration::from_secs(2));
    Ok(())
}
