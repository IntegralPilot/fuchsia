// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder};
use fidl_next_examples_keyvaluestore_supporttrees::{
    Item, Store, StoreServerHandler, Value, WriteError, store,
};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use regex::Regex;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::str::from_utf8;
use std::sync::LazyLock;

static KEY_VALIDATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z]\w+[A-Za-z0-9]$").expect("Key validation regex failed to compile")
});

// A representation of a key-value store that can contain an arbitrarily deep nesting of other
// key-value stores.
#[allow(clippy::box_collection, reason = "mass allow for https://fxbug.dev/381896734")]
#[allow(dead_code)]
enum StoreNode {
    Leaf(Option<Vec<u8>>),
    Branch(Box<HashMap<String, StoreNode>>),
}

struct StoreServer {
    store: HashMap<String, StoreNode>,
}

impl StoreServerHandler for StoreServer {
    async fn write_item(
        &mut self,
        request: Request<store::WriteItem>,
        responder: Responder<store::WriteItem>,
    ) {
        let payload = request.payload();
        let result = write_item(&mut self.store, &payload.attempt, "");
        match result {
            Ok(()) => {
                responder.respond(()).await.unwrap();
            }
            Err(e) => {
                responder.respond_err(e).await.unwrap();
            }
        }
    }
}

/// Recursive item writer, which takes a `StoreNode` that may not necessarily be the root node, and
/// writes an entry to it.
fn write_item(
    store: &mut HashMap<String, StoreNode>,
    attempt: &Item,
    path: &str,
) -> Result<(), WriteError> {
    // Validate the key.
    if !KEY_VALIDATION_REGEX.is_match(attempt.key.as_str()) {
        println!("Write error: INVALID_KEY, For key: {}", attempt.key);
        return Err(WriteError::InvalidKey);
    }

    // Write to the store, validating that the key did not already exist.
    match store.entry(attempt.key.clone()) {
        Entry::Occupied(entry) => {
            println!("Write error: ALREADY_EXISTS, For key: {}", entry.key());
            Err(WriteError::AlreadyExists)
        }
        Entry::Vacant(entry) => {
            let key = format!("{}{}", path, entry.key());
            match &attempt.value {
                // Null entries are allowed.
                None => {
                    println!("Wrote value: NONE at key: {}", key);
                    entry.insert(StoreNode::Leaf(None));
                }
                Some(value) => match &**value {
                    // If this is a nested store, recursively make a new store to insert at this
                    // position.
                    Value::Store(entry_list) => {
                        // Validate the value - absent stores, items lists with no children, or any
                        // of the elements within that list being empty boxes, are all not allowed.
                        if let Some(items) = &entry_list.items {
                            if !items.is_empty() && items.iter().all(|i| i.is_some()) {
                                let nested_path = format!("{}/", key);
                                let mut nested_store = HashMap::<String, StoreNode>::new();
                                for item in items.iter() {
                                    write_item(
                                        &mut nested_store,
                                        &**item.as_ref().unwrap(),
                                        &nested_path,
                                    )?;
                                }

                                println!("Created branch at key: {}", key);
                                entry.insert(StoreNode::Branch(Box::new(nested_store)));
                                return Ok(());
                            }
                        }

                        println!("Write error: INVALID_VALUE, For key: {}", key);
                        return Err(WriteError::InvalidValue);
                    }

                    // This is a simple leaf node on this branch.
                    Value::Bytes(value) => {
                        // Validate the value.
                        if value.is_empty() {
                            println!("Write error: INVALID_VALUE, For key: {}", key);
                            return Err(WriteError::InvalidValue);
                        }

                        println!("Wrote key: {}, value: {:?}", key, from_utf8(value).unwrap());
                        entry.insert(StoreNode::Leaf(Some(value.clone())));
                    }
                },
            }
            Ok(())
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Add a discoverable instance of our `Store` protocol - this will allow the client to see the
    // server and connect to it.
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_next_protocol::<Store, _>(|_| StoreServer { store: HashMap::new() });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    fs.collect::<()>().await;

    Ok(())
}
