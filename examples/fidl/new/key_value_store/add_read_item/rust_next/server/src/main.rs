// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder};
use fidl_next_examples_keyvaluestore_addreaditem::{
    Item, ReadError, Store, StoreServerHandler, StoreWriteItemResponse, WriteError, store,
};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use regex::Regex;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::LazyLock;

static KEY_VALIDATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z][A-Za-z0-9_\./]{2,62}[A-Za-z0-9]$")
        .expect("Key validation regex failed to compile")
});

struct StoreServer {
    store: HashMap<String, Vec<u8>>,
}

impl StoreServerHandler for StoreServer {
    async fn write_item(
        &mut self,
        request: Request<store::WriteItem>,
        responder: Responder<store::WriteItem>,
    ) {
        let attempt = &request.payload().attempt;
        println!("WriteItem request received");

        let result = self.write_item_impl(attempt);
        match result {
            Ok(response) => responder.respond(response).await.unwrap(),
            Err(error) => responder.respond_err(error).await.unwrap(),
        }
        println!("WriteItem response sent");
    }

    async fn read_item(
        &mut self,
        request: Request<store::ReadItem>,
        responder: Responder<store::ReadItem>,
    ) {
        let key = &request.payload().key;
        println!("ReadItem request received");

        let result = self.read_item_impl(key);

        match result {
            Ok(response) => responder.respond(response).await.unwrap(),
            Err(error) => responder.respond_err(error).await.unwrap(),
        }
        println!("ReadItem response sent");
    }
}

impl StoreServer {
    fn write_item_impl(&mut self, attempt: &Item) -> Result<StoreWriteItemResponse, WriteError> {
        // Validate the key.
        if !KEY_VALIDATION_REGEX.is_match(&attempt.key) {
            println!("Write error: INVALID_KEY, For key: {}", attempt.key);
            return Err(WriteError::InvalidKey);
        }

        // Validate the value.
        if attempt.value.is_empty() {
            println!("Write error: INVALID_VALUE, For key: {}", attempt.key);
            return Err(WriteError::InvalidValue);
        }

        // Write to the store, validating that the key did not already exist.
        match self.store.entry(attempt.key.clone()) {
            Entry::Occupied(entry) => {
                println!("Write error: ALREADY_EXISTS, For key: {}", entry.key());
                Err(WriteError::AlreadyExists)
            }
            Entry::Vacant(entry) => {
                println!("Wrote value at key: {}", entry.key());
                entry.insert(attempt.value.clone());
                Ok(())
            }
        }
    }

    fn read_item_impl(&mut self, key: &str) -> Result<Item, ReadError> {
        match self.store.get(key) {
            Some(found) => {
                println!("Read value at key: {}", key);
                Ok(Item { key: key.to_string(), value: found.clone() })
            }
            None => {
                println!("Read error: NOT_FOUND, For key: {}", key);
                Err(ReadError::NotFound)
            }
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Add a discoverable instance of our `Store` protocol - this will allow the client to see the
    // server and connect to it.
    let mut fs = ServiceFs::new_local();
    fs.dir("svc")
        .add_fidl_next_protocol::<Store, _>(|_sender| StoreServer { store: HashMap::new() });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    // Run the service fs.
    fs.collect::<()>().await;

    Ok(())
}
