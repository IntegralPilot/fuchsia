// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder};
use fidl_next_examples_keyvaluestore_baseline::{
    Item, Store, StoreServerHandler, WriteError, store,
};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use regex::Regex;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::LazyLock;

static KEY_VALIDATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z]\w+[A-Za-z0-9]$").expect("Key validation regex failed to compile")
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

        // The `responder` parameter is a special struct that manages the outgoing reply
        // to this method call. Calling `send` on the responder exactly once will send
        // the reply.
        match result {
            Ok(()) => {
                responder.respond(()).await.unwrap();
            }
            Err(e) => {
                responder.respond_err(e).await.unwrap();
            }
        }
        println!("WriteItem response sent");
    }
}

impl StoreServer {
    fn write_item_impl(&mut self, attempt: &Item) -> Result<(), WriteError> {
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
