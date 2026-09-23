// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_google_nanohub::{
    DisplayDeviceGetDisplaySelectResponse, DisplayDeviceRequest, DisplayMode, DisplaySelect,
    DisplayServiceRequest, DisplayState, DisplaySyncInfo,
};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use std::sync::Arc;

pub type DisplayRequests = Arc<Mutex<Vec<String>>>;

pub async fn mock_display_server(
    handles: LocalComponentHandles,
    requests: DisplayRequests,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(DisplayServiceRequest::Mcudisplay);
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(0, |DisplayServiceRequest::Mcudisplay(stream)| {
        let requests = requests.clone();
        async move {
            stream
                .try_for_each(|request| {
                    let requests = requests.clone();
                    async move {
                        match request {
                            DisplayDeviceRequest::GetDisplayState { responder } => {
                                requests.lock().push("GetDisplayState".into());
                                let response = DisplayState {
                                    mode: Some(DisplayMode::On),
                                    ..Default::default()
                                };
                                responder.send(Ok(&response))?;
                            }
                            DisplayDeviceRequest::GetDisplayInfo { responder } => {
                                requests.lock().push("GetDisplayInfo".into());
                                let response = DisplaySyncInfo {
                                    display_mode: Some(DisplayMode::On),
                                    panel_mode: Some(1),
                                    normal_brightness: Some(2),
                                    always_on_display_brightness: Some(3),
                                    ..Default::default()
                                };
                                responder.send(Ok(&response))?;
                            }
                            DisplayDeviceRequest::GetDisplaySelect { responder } => {
                                requests.lock().push("GetDisplaySelect".into());
                                let response = DisplayDeviceGetDisplaySelectResponse {
                                    display_select: Some(DisplaySelect::Mcu),
                                    ..Default::default()
                                };
                                responder.send(Ok(&response))?;
                            }
                            DisplayDeviceRequest::SetDisplaySelect { payload: _, responder } => {
                                requests.lock().push("SetDisplaySelect".into());
                                responder.send(Ok(()))?;
                            }
                        }
                        Ok(())
                    }
                })
                .await
                .unwrap_or_else(|e| eprintln!("Error encountered: {:?}", e))
        }
    })
    .await;

    Ok(())
}
