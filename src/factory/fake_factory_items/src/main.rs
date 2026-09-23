// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod config;
mod fake_factory_items_server;

use anyhow::Error;
use config::Config;
use fake_factory_items_server::{FakeFactoryItemsServer, spawn_fake_factory_items_server};
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::RwLock;
use futures::prelude::*;
use std::sync::Arc;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let config = Config::load().unwrap();
    let config_map = Arc::new(RwLock::new(config.into()));

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        let server = FakeFactoryItemsServer::new(Arc::clone(&config_map));
        spawn_fake_factory_items_server(server, stream);
    });
    fs.take_and_serve_directory_handle()?;

    fs.collect::<()>().await;
    Ok(())
}
