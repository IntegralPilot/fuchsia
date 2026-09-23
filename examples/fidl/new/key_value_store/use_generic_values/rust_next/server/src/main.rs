// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_next::{Request, Responder};
use fidl_next_examples_keyvaluestore_usegenericvalues::{
    Item, Store, StoreServerHandler, Value, WriteError, WriteOptions, store,
};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use regex::Regex;
use std::collections::HashMap;
use std::collections::hash_map::{Entry, OccupiedEntry};
use std::ops::Add;
use std::sync::LazyLock;

static KEY_VALIDATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z]\w+[A-Za-z0-9]$").expect("Key validation regex failed to compile")
});

/// Sums any numeric type.
fn sum<T: Add + Add<Output = T> + Copy>(operands: [T; 2]) -> T {
    operands[0] + operands[1]
}

/// Clones and inserts an entry, so that the original (now concatenated) copy may be returned in the
/// response.
fn write(inserting: Value, mut entry: OccupiedEntry<'_, String, Value>) -> Value {
    entry.insert(inserting.clone());
    println!("Wrote key: {}, value: {:?}", entry.key(), inserting);
    inserting
}

struct StoreServer {
    store: HashMap<String, Value>,
}

impl StoreServer {
    fn write_item_impl(
        &mut self,
        attempt: Item,
        options: WriteOptions,
    ) -> Result<Value, WriteError> {
        // Validate the key.
        if !KEY_VALIDATION_REGEX.is_match(&attempt.key) {
            println!("Write error: INVALID_KEY for key: {}", attempt.key);
            return Err(WriteError::InvalidKey);
        }

        match self.store.entry(attempt.key) {
            Entry::Occupied(entry) => {
                // The `CONCAT` flag supersedes the `OVERWRITE` flag, so check it first.
                if options.contains(WriteOptions::CONCAT) {
                    match entry.get() {
                        Value::Bytes(old) => {
                            if let Value::Bytes(new) = attempt.value {
                                let mut combined = old.clone();
                                combined.extend(new);
                                return Ok(write(Value::Bytes(combined), entry));
                            }
                        }
                        Value::String(old) => {
                            if let Value::String(new) = attempt.value {
                                return Ok(write(Value::String(format!("{}{}", old, new)), entry));
                            }
                        }
                        Value::Uint64(old) => {
                            if let Value::Uint64(new) = attempt.value {
                                return Ok(write(Value::Uint64(sum([*old, new])), entry));
                            }
                        }
                        Value::Int64(old) => {
                            if let Value::Int64(new) = attempt.value {
                                return Ok(write(Value::Int64(sum([*old, new])), entry));
                            }
                        }
                        // Note: only works on the uint64 range in practice.
                        Value::Uint128(old) => {
                            if let Value::Uint128(new) = attempt.value {
                                return Ok(write(
                                    Value::Uint128([0, sum([old[1], new[1]])]),
                                    entry,
                                ));
                            }
                        }
                        _ => {
                            // In a real server you might return a proper error here or handle all types.
                            println!("Write error: Unsupported type for concatenation");
                            return Err(WriteError::InvalidValue);
                        }
                    }

                    // Only reachable if the type of the would be concatenated value did not match the
                    // value already occupying this entry (or we fell through from above).
                    println!("Write error: INVALID_VALUE for key: {}", entry.key());
                    return Err(WriteError::InvalidValue);
                }

                // If we're not doing CONCAT, check for OVERWRITE next.
                if options.contains(WriteOptions::OVERWRITE) {
                    return Ok(write(attempt.value, entry));
                }

                println!("Write error: ALREADY_EXISTS for key: {}", entry.key());
                Err(WriteError::AlreadyExists)
            }
            Entry::Vacant(entry) => {
                println!("Wrote key: {}, value: {:?}", entry.key(), attempt.value);
                entry.insert(attempt.value.clone());
                Ok(attempt.value)
            }
        }
    }
}

impl StoreServerHandler for StoreServer {
    async fn write_item(
        &mut self,
        request: Request<store::WriteItem>,
        responder: Responder<store::WriteItem>,
    ) {
        println!("WriteItem request received");
        let payload = request.payload();

        let result: Result<(), Error> = async {
            let attempt =
                payload.attempt.as_ref().context("WriteItem error: missing attempt")?.clone();
            let options = *payload.options.as_ref().context("WriteItem error: missing options")?;

            let result = self.write_item_impl(attempt, options);

            match result {
                Ok(val) => {
                    responder.respond(val).await.unwrap();
                }
                Err(e) => {
                    responder.respond_err(e).await.unwrap();
                }
            }
            Ok(())
        }
        .await;

        if let Err(e) = result {
            println!("{:?}", e);
        }
        println!("WriteItem response sent");
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_next_protocol::<Store, _>(move |_sender| {
        let store = HashMap::<String, Value>::new();
        StoreServer { store }
    });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    fs.collect::<()>().await;

    Ok(())
}
