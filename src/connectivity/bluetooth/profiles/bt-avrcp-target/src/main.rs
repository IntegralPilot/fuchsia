// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use futures::{try_join, FutureExt};
use std::sync::Arc;

mod avrcp_handler;
mod battery_client;
mod media;
mod types;

#[cfg(test)]
mod tests;

use crate::avrcp_handler::process_avrcp_requests;
use crate::battery_client::process_battery_client_requests;
use crate::media::media_sessions::MediaSessions;

#[fuchsia::main(logging_tags = ["bt-avrcp-tg"])]
async fn main() -> Result<(), Error> {
    // Shared state between AVRCP and MediaSession.
    // The current view of the media world.
    let media_state: Arc<MediaSessions> = Arc::new(MediaSessions::create());

    let watch_media_sessions_fut = media_state.watch();
    let avrcp_requests_fut = process_avrcp_requests(media_state.clone());
    // Power integration is optional - the AVRCP-TG component will continue even if power
    // integration is unavailable.
    let battery_client_fut = process_battery_client_requests(media_state.clone()).map(|_| Ok(()));

    let result =
        try_join!(watch_media_sessions_fut, avrcp_requests_fut, battery_client_fut).map(|_| ());
    log::info!(result:?; "finished");
    result
}
