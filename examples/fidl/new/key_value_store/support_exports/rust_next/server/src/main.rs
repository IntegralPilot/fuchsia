// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::persist;
use fidl_examples_keyvaluestore_supportexports as fidl_legacy;
use fidl_next::{Request, Responder};
use fidl_next_examples_keyvaluestore_supportexports::{
    ExportError, Item, Store, StoreServerHandler, WriteError, store,
};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use regex::Regex;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::LazyLock;
use zx::Vmo;

static KEY_VALIDATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z]\w+[A-Za-z0-9]$").expect("Key validation regex failed to compile")
});

struct StoreServer {
    store: HashMap<String, Vec<u8>>,
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

    fn export_impl(&mut self, vmo: Vmo) -> Result<Vmo, ExportError> {
        // Empty stores cannot be exported.
        if self.store.is_empty() {
            return Err(ExportError::Empty);
        }

        // Build the `Exportable` vector locally. That means iterating over the map, and turning it into
        // a vector of items instead.
        // We use the legacy bindings types here to leverage fidl::persist.
        let mut items = self
            .store
            .iter()
            .map(|(k, v)| fidl_legacy::Item { key: k.clone(), value: v.clone() })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| a.key.cmp(&b.key));

        let exportable = fidl_legacy::Exportable { items: Some(items), ..Default::default() };

        // Encode the bytes.
        let encoded_bytes = persist(&exportable).map_err(|_| ExportError::Unknown)?;

        // Check that the VMO has enough space.
        let content_size = vmo.get_content_size().map_err(|_| ExportError::Unknown)?;
        if encoded_bytes.len() as u64 > content_size {
            return Err(ExportError::StorageTooSmall);
        }

        // Write the (now encoded) persistent FIDL data to the VMO.
        vmo.set_content_size(&(encoded_bytes.len() as u64)).map_err(|_| ExportError::Unknown)?;
        vmo.write(&encoded_bytes, 0).map_err(|_| ExportError::Unknown)?;
        Ok(vmo)
    }
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
            Ok(()) => {
                responder.respond(()).await.unwrap();
            }
            Err(e) => {
                responder.respond_err(e).await.unwrap();
            }
        }
        println!("WriteItem response sent");
    }

    async fn export(
        &mut self,
        request: Request<store::Export>,
        responder: Responder<store::Export>,
    ) {
        println!("Export request received");

        // We need to get the VMO from the request.
        let vmo = request.payload().empty.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into();

        let result = self.export_impl(vmo);

        match result {
            Ok(output_vmo) => {
                responder.respond(output_vmo).await.unwrap();
            }
            Err(e) => {
                responder.respond_err(e).await.unwrap();
            }
        }
        println!("Export response sent");
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Add a discoverable instance of our `Store` protocol.
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_next_protocol::<Store, _>(move |_sender| {
        // Create a new in-memory key-value store. The store will live for the lifetime of the
        // connection.
        let store = HashMap::<String, Vec<u8>>::new();
        StoreServer { store }
    });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    // Serve each connection simultaneously.
    // add_fidl_next_protocol automatically handles concurrent connections via the ServiceFs.
    fs.collect::<()>().await;

    Ok(())
}
