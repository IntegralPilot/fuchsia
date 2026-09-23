// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;

pub mod fidl_helpers;
pub mod index_convert;
pub mod legacy_ime;
pub mod text_manager;

mod keyboard;

#[fuchsia::main(logging_tags = ["text_manager"])]
async fn main() -> Result<(), Error> {
    let text_manager = text_manager::TextManager::new();
    let keyboard_service = keyboard::Service::new(text_manager.clone())
        .await
        .context("error initializing keyboard service")?;
    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service(|stream| keyboard_service.spawn_keyboard3_service(stream))
        .add_fidl_service(|stream| keyboard_service.spawn_ime_service(stream))
        // Requires clients to have `fuchsia.ui.input3.KeyEventInjector` in the sandbox.
        .add_fidl_service(|stream| keyboard_service.spawn_key_event_injector(stream))
        .add_fidl_service(|stream| keyboard_service.spawn_focus_controller(stream));

    fs.take_and_serve_directory_handle()?;
    let () = fs.collect().await;
    Ok(())
}
