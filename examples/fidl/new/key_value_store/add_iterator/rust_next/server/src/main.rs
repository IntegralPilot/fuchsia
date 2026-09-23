// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder, ServerDispatcher};
use fidl_next_examples_keyvaluestore_additerator::{
    IterateConnectionError, IteratorServerHandler, Store, StoreServerHandler, WriteError, iterator,
    store,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use regex::Regex;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::{Arc, LazyLock, Mutex};

// The regex for checking that a key is valid.
static KEY_VALIDATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-z][A-z0-9_\./]{2,62}[A-z0-9]$").unwrap());

struct StoreServer {
    store: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    scope: fasync::Scope,
}

impl StoreServerHandler for StoreServer {
    async fn write_item(
        &mut self,
        request: Request<store::WriteItem>,
        responder: Responder<store::WriteItem>,
    ) {
        let payload = request.payload();
        let attempt = &payload.attempt;
        println!("WriteItem request received");

        // Validate the key.
        if !KEY_VALIDATION_REGEX.is_match(&attempt.key) {
            println!("Write error: INVALID_KEY, For key: {}", attempt.key);
            if let Err(e) = responder.respond_err(WriteError::InvalidKey).await {
                println!("Error sending response: {:?}", e);
            }
            return;
        }

        // Validate the value.
        if attempt.value.is_empty() {
            println!("Write error: INVALID_VALUE, For key: {}", attempt.key);
            if let Err(e) = responder.respond_err(WriteError::InvalidValue).await {
                println!("Error sending response: {:?}", e);
            }
            return;
        }

        // Write to the store, validating that the key did not already exist.
        let result = {
            let mut store = self.store.lock().unwrap();
            match store.entry(attempt.key.clone()) {
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
        };

        match result {
            Ok(response) => {
                if let Err(e) = responder.respond(response).await {
                    println!("Error sending response: {:?}", e);
                }
            }
            Err(error) => {
                if let Err(e) = responder.respond_err(error).await {
                    println!("Error sending response: {:?}", e);
                }
            }
        }
        println!("WriteItem response sent");
    }

    async fn iterate(
        &mut self,
        request: Request<store::Iterate>,
        responder: Responder<store::Iterate>,
    ) {
        let payload = request.payload();
        let starting_at = payload.starting_at;
        let iterator = payload.iterator;

        println!("Iterate request received");

        // Validate that the starting key, if supplied, actually exists.
        if let Some(start_key) = &starting_at {
            if !self.store.lock().unwrap().contains_key(start_key) {
                if let Err(e) = responder.respond_err(IterateConnectionError::UnknownStartAt).await
                {
                    println!("Error sending response: {:?}", e);
                }
                return;
            }
        }

        let store = self.store.clone();

        // Spawn the iterator server on the scope of the store server.
        let server = IteratorServer {
            store,
            lower_bound: match starting_at {
                Some(start_key) => Included(start_key),
                None => Unbounded,
            },
        };
        let dispatcher = ServerDispatcher::new(iterator);
        let _ = self.scope.spawn(async move {
            let _ = dispatcher.run(server).await;
        });

        if let Err(e) = responder.respond(()).await {
            println!("Error sending response: {:?}", e);
        }
        println!("Iterate response sent");
    }
}

struct IteratorServer {
    store: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    lower_bound: std::ops::Bound<String>,
}

impl IteratorServerHandler for IteratorServer {
    async fn get(&mut self, responder: Responder<iterator::Get>) {
        println!("Iterator page request received");

        // An iterator, beginning at `lower_bound` and tracking the pagination's
        // progress through iteration as each page is pulled by a client-sent
        // `Get()` request.
        let (current_page, next_bound) = self.get_page();

        self.lower_bound = next_bound;

        // Send the page. At the end of this scope, the `held_store` lock gets
        // dropped, and therefore released.
        if let Err(e) = responder.respond(current_page).await {
            println!("Error sending response: {:?}", e);
        }
        println!("Iterator page sent");
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Add a discoverable instance of our `Store` protocol - this will allow the client to see the
    // server and connect to it.
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_next_protocol::<Store, _>(move |_sender| {
        // Create a new in-memory key-value store. The store will live for the lifetime of the
        // connection between the server and this particular client.
        let store = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
        let scope = fasync::Scope::new();
        StoreServer { store, scope }
    });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    // Serve each connection simultaneously.
    fs.collect::<()>().await;

    Ok(())
}

impl IteratorServer {
    fn get_page(&self) -> (Vec<String>, std::ops::Bound<String>) {
        static PAGE_SIZE: usize = 10;
        let held_store = self.store.lock().unwrap();
        let mut entries = held_store.range((self.lower_bound.clone(), Unbounded));
        let mut current_page = vec![];
        for _ in 0..PAGE_SIZE {
            match entries.next() {
                Some(entry) => {
                    current_page.push(entry.0.clone());
                }
                None => break,
            }
        }

        let next_bound = match entries.next() {
            Some(next) => Included(next.0.clone()),
            None => match current_page.last() {
                Some(tail) => Excluded(tail.clone()),
                None => self.lower_bound.clone(),
            },
        };
        (current_page, next_bound)
    }
}
