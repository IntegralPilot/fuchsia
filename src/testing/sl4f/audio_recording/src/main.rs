// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use anyhow::Error;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::stream::StreamExt;
use log::{error, info};

mod audio_facade;
mod input_worker;
mod output_worker;
mod server;
mod util;

use crate::audio_facade::AudioFacade;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    info!("Starting audio recording service");
    let mut fs = ServiceFs::new_local();

    let audio_facade: Arc<AudioFacade> = Arc::new(AudioFacade::new().await?);

    let audio_facade_clone = audio_facade.clone();
    fs.dir("svc").add_fidl_service(move |stream| {
        info!("Starting injection stream");
        let audio_facade_clone = audio_facade_clone.clone();
        fasync::Task::spawn(async move {
            server::handle_injection_request(audio_facade_clone, stream)
                .await
                .unwrap_or_else(|e| error!("Error handling injection channel: {:?}", e))
        })
        .detach();
    });

    fs.dir("svc").add_fidl_service(move |stream| {
        info!("Starting capture stream");
        let audio_facade_clone = audio_facade.clone();
        fasync::Task::spawn(async move {
            server::handle_capture_request(audio_facade_clone, stream)
                .await
                .unwrap_or_else(|e| error!("Error handling recording channel: {:?}", e))
        })
        .detach();
    });

    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;

    Ok(())
}
