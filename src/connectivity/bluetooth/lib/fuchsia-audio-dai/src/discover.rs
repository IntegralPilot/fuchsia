// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_io as fio;
use std::path::Path;

use crate::DigitalAudioInterface;

/// The member name within each service directory that provides the DAI connector.
const DAI_CONNECTOR_MEMBER: &str = "dai_connector";
const DAI_SERVICE_DIR: &str = "/svc/fuchsia.hardware.audio.DaiConnectorService";

/// Finds any DAI devices, connects to any that are available and provides access to them.
pub async fn find_devices() -> Result<Vec<DigitalAudioInterface>, Error> {
    // Connect to the component's environment.
    let directory_proxy =
        fuchsia_fs::directory::open_in_namespace(DAI_SERVICE_DIR, fuchsia_fs::PERM_READABLE)?;
    find_devices_internal(directory_proxy).await
}

async fn find_devices_internal(
    directory_proxy: fio::DirectoryProxy,
) -> Result<Vec<DigitalAudioInterface>, Error> {
    let files = fuchsia_fs::directory::readdir(&directory_proxy).await?;

    let devices = files
        .iter()
        .map(|file| {
            let path = Path::new(DAI_SERVICE_DIR).join(&file.name).join(DAI_CONNECTOR_MEMBER);
            DigitalAudioInterface::new(&path)
        })
        .collect();

    Ok(devices)
}

#[cfg(test)]
mod tests {
    use fidl_fuchsia_hardware_audio as fhaudio;
    use fuchsia_component_test::{
        Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Route,
    };
    use futures::channel::mpsc;
    use futures::{SinkExt, StreamExt};
    use realmbuilder_mock_helpers::mock_svc;

    use super::*;
    use crate::test::mock_dai_service_with_io_devices;

    #[fuchsia::test]
    async fn test_env_dir_is_not_found() {
        let _ = find_devices().await.expect_err("find devices okay");
    }

    async fn mock_client(
        handles: LocalComponentHandles,
        mut sender: mpsc::Sender<()>,
    ) -> Result<(), Error> {
        let service_dir = handles.open_service::<fhaudio::DaiConnectorServiceMarker>()?;
        let devices = find_devices_internal(service_dir).await.expect("should find devices");
        assert_eq!(devices.len(), 2);
        let _ = sender.send(()).await.unwrap();
        Ok(())
    }

    #[fuchsia::test]
    async fn devices_found_from_env() {
        let (device_sender, mut device_receiver) = mpsc::channel(0);
        let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");

        // Add a mock that provides the service with one input and output device.
        let mock_svc = builder
            .add_local_child(
                "mock-svc",
                move |handles: LocalComponentHandles| {
                    Box::pin(mock_svc(
                        handles,
                        mock_dai_service_with_io_devices("input1", "output1"),
                    ))
                },
                ChildOptions::new().eager(),
            )
            .await
            .expect("Failed adding mock service provider to topology");

        // Add a mock that represents a client trying to discover DAI devices.
        let mock_client = builder
            .add_local_child(
                "mock-client",
                move |handles: LocalComponentHandles| {
                    let s = device_sender.clone();
                    Box::pin(mock_client(handles, s.clone()))
                },
                ChildOptions::new().eager(),
            )
            .await
            .expect("Failed adding mock client to topology");

        // Give client access to service
        builder
            .add_route(
                Route::new()
                    .capability(Capability::service::<fhaudio::DaiConnectorServiceMarker>())
                    .from(&mock_svc)
                    .to(&mock_client),
            )
            .await
            .expect("Failed adding route for dai device service");

        let _instance = builder.build().await.unwrap();

        let _ = device_receiver.next().await.expect("should receive devices");
    }
}
