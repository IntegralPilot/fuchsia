// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_google_nanohub::{DeviceRequest, ServiceRequest};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use futures::prelude::*;

const FAKE_FIRMWARE_NAME: &str = "test_firmware_name";

pub async fn mock_nanohub_server(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(ServiceRequest::Nanohub);
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(0, |ServiceRequest::Nanohub(stream)| async move {
        stream
            .try_for_each(|request| async move {
                match request {
                    DeviceRequest::GetFirmwareName { responder } => {
                        responder.send(FAKE_FIRMWARE_NAME)?;
                    }
                    other => {
                        panic!("at the disco: {other:?}")
                    }
                }
                Ok(())
            })
            .await
            .unwrap_or_else(|e| eprintln!("Error encountered: {:?}", e))
    })
    .await;

    Ok(())
}
