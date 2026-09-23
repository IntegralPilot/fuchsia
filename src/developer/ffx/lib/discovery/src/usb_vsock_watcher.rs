// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TargetEvent;
use fidl_fuchsia_developer_remotecontrol as fidl_rcs;
use fuchsia_async::Task;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::StreamExt;
use std::path::PathBuf;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use usb_driver_api::{ConnectError, DeviceEvent, Driver};

const IDENTIFY_VSOCK_PORT: u32 = 201;

/// Watches for devices that speak the USB VSOCK protocol.
pub struct UsbVsockWatcher {
    /// Task for the drain loop
    _drain_task: Task<()>,
}

#[derive(Debug, Error)]
enum NodeNameFetchError {
    #[error(transparent)]
    ConnectError(#[from] ConnectError),
    #[error("Identify request failed: {0:?}")]
    IdentifyHostError(fidl_rcs::IdentifyHostError),
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("RCS did not recognize Identify request")]
    Unknown,
}

impl From<fidl_rcs::IdentifyHostError> for NodeNameFetchError {
    fn from(value: fidl_rcs::IdentifyHostError) -> Self {
        NodeNameFetchError::IdentifyHostError(value)
    }
}

impl UsbVsockWatcher {
    /// Create a new [`UsbVsockWatcher`]. Starts a task which begins watching
    /// for devices immediately.
    pub(crate) fn new(socket_path: PathBuf, sender: UnboundedSender<TargetEvent>) -> Self {
        let _drain_task = Task::local(async move {
            let driver = match Driver::init(&socket_path).await {
                Ok(d) => d,
                Err(error) => {
                    log::error!(socket_path = socket_path.display().to_string().as_str(), error:?;
                                "Failed to initialize connection to USB VSOCK driver");
                    return;
                }
            };
            let stream = match driver.listen_for_devices().await {
                Ok(s) => s,
                Err(error) => {
                    log::error!(socket_path = socket_path.display().to_string().as_str(), error:?;
                                "Failed to establish USB VSOCK device event stream");
                    return;
                }
            };
            let mut stream = std::pin::pin!(stream);
            while let Some(Ok(event)) = stream.next().await {
                let nodename = if let DeviceEvent::Added { cid, serial } = &event {
                    let nodename: Result<Option<String>, NodeNameFetchError> = async {
                        let mut data = Vec::new();
                        let mut stream = driver.connect(*cid, IDENTIFY_VSOCK_PORT).await?;
                        let size = stream.read_to_end(&mut data).await?;
                        debug_assert!(data.len() == size);

                        let (header, bytes) = fidl::encoding::decode_transaction_header(&data)?;
                        let body = fidl_message::decode_response_flexible_result::<
                            fidl_rcs::IdentifyHostResponse,
                            fidl_rcs::IdentifyHostError,
                        >(header, bytes)?;

                        if let fidl_message::MaybeUnknown::Known(ident) = body {
                            Ok(ident?.nodename)
                        } else {
                            Err(NodeNameFetchError::Unknown)
                        }
                    }
                    .await;

                    match nodename {
                        Ok(n) => n,
                        Err(error) => {
                            log::warn!(error:?, cid, serial:?; "Could not contact RCS on device");
                            None
                        }
                    }
                } else {
                    None
                };
                let _ = sender.unbounded_send(TargetEvent::from_usb_event(event, nodename));
            }
        });
        UsbVsockWatcher { _drain_task }
    }
}
