// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context as _, Error, anyhow};
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_media::AudioRenderUsage2;
use fidl_fuchsia_media_sounds::{PlayerMarker, PlayerProxy};
use futures::lock::Mutex;
use settings_common::call_async;
use settings_common::inspect::event::ExternalEventPublisher;
use settings_common::service_context::{ExternalServiceProxy, ServiceContext};
use std::collections::HashSet;
use std::rc::Rc;
use {fidl_fuchsia_io as fio, fuchsia_async as fasync};

/// Creates a file-based sound from a resource file.
fn resource_file(name: &str) -> Result<fidl::endpoints::ClientEnd<fio::FileMarker>, Error> {
    let path = format!("/config/data/{name}");
    fuchsia_fs::file::open_in_namespace(&path, fio::PERM_READABLE)
        .with_context(|| format!("opening resource file: {path}"))?
        .into_client_end()
        .map_err(|_: fio::FileProxy| {
            anyhow!("failed to convert new Proxy to ClientEnd for resource file {path}")
        })
}

/// Establish a connection to the sound player and return the proxy representing the service.
/// Will not do anything if the sound player connection is already established.
pub(super) async fn connect_to_sound_player(
    external_publisher: ExternalEventPublisher,
    service_context_handle: Rc<ServiceContext>,
    sound_player_connection: Rc<
        Mutex<Option<ExternalServiceProxy<PlayerProxy, ExternalEventPublisher>>>,
    >,
) {
    let mut sound_player_connection_lock = sound_player_connection.lock().await;
    if sound_player_connection_lock.is_none() {
        *sound_player_connection_lock = service_context_handle
            .connect_with_publisher::<PlayerMarker, _>(external_publisher)
            .await
            .context("Connecting to fuchsia.media.sounds.Player")
            .map_err(|e| log::error!("Failed to connect to fuchsia.media.sounds.Player: {}", e))
            .ok()
    }
}

/// Plays a sound with the given `id` and `file_name` via the `sound_player_proxy`.
///
/// The `id` and `file_name` are expected to be unique and mapped 1:1 to each other. This allows
/// the sound file to be reused without having to load it again.
pub(super) async fn play_sound<'a>(
    sound_player_proxy: &ExternalServiceProxy<PlayerProxy, ExternalEventPublisher>,
    file_name: &'a str,
    id: u32,
    added_files: Rc<Mutex<HashSet<&'a str>>>,
) -> Result<(), Error> {
    // New sound, add it to the sound player set.
    if added_files.lock().await.insert(file_name) {
        let sound_file_channel = match resource_file(file_name) {
            Ok(file) => Some(file),
            Err(e) => return Err(anyhow!("[earcons] Failed to convert sound file: {}", e)),
        };
        if let Some(file_channel) = sound_file_channel {
            match call_async!(sound_player_proxy => add_sound_from_file(id, file_channel)).await {
                Ok(_) => log::debug!("[earcons] Added sound to Player: {}", file_name),
                Err(e) => {
                    return Err(anyhow!("[earcons] Unable to add sound to Player: {}", e));
                }
            };
        }
    }

    let sound_player_proxy = sound_player_proxy.clone();
    // This fasync thread is needed so that the earcons sounds can play rapidly and not wait
    // for the previous sound to finish to send another request.
    fasync::Task::local(async move {
        if let Err(e) =
            call_async!(sound_player_proxy => play_sound2(id, AudioRenderUsage2::Background)).await
        {
            log::error!("[earcons] Unable to Play sound from Player: {}", e);
        };
    })
    .detach();
    Ok(())
}
