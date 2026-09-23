// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::Device;
use crate::environment::Environment;
use crate::matcher::Matcher;
use crate::{matcher, service};
use anyhow::{Error, format_err};
use fs_management::format::DiskFormat;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::lock::Mutex;
use std::sync::Arc;

fn size_as_string(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;
    if bytes >= GIB {
        let gibibytes = bytes / GIB;
        format!("{gibibytes}G")
    } else if bytes >= MIB {
        let mebibytes = bytes / MIB;
        format!("{mebibytes}M")
    } else if bytes >= KIB {
        let kibibytes = bytes / KIB;
        format!("{kibibytes}K")
    } else {
        format!("{bytes}B")
    }
}

pub struct Manager {
    matcher: matcher::Matchers,
    environment: Arc<Mutex<dyn Environment>>,
}

impl Manager {
    pub fn new(
        config: &fshost_config::Config,
        environment: Arc<Mutex<dyn Environment>>,
        extra_matchers: Vec<Box<dyn Matcher>>,
    ) -> Self {
        Manager {
            matcher: matcher::Matchers::new_with_extra_matchers(config, extra_matchers),
            environment,
        }
    }

    /// The main loop of fshost. Watch for new devices, match them against filesystems we expect,
    /// and then launch them appropriately.
    pub async fn device_handler(
        &mut self,
        device_stream: impl futures::Stream<Item = Box<dyn Device>>,
        mut shutdown_rx: mpsc::Receiver<service::FshostShutdownResponder>,
    ) -> Result<service::FshostShutdownResponder, Error> {
        let mut device_stream = Box::pin(device_stream).fuse();
        let mut block_index = 0;
        loop {
            let name = format!("{:03}", block_index);
            block_index += 1;
            // Wait for the next device to come in, or the shutdown signal to arrive.
            let mut device = futures::select! {
                responder = shutdown_rx.next() => {
                    let responder = responder
                        .ok_or_else(|| format_err!("shutdown signal stream ended unexpectedly"))?;
                    return Ok(responder);
                },
                maybe_device = device_stream.next() => {
                    if let Some(device) = maybe_device {
                        device
                    } else {
                        anyhow::bail!("block watcher returned none unexpectedly");
                    }
                },
            };

            let mut environment = self.environment.lock().await;
            let content_format = device.content_format().await.unwrap_or(DiskFormat::Unknown);
            let label = device.partition_label().await.ok().map(|s| s.to_string());
            let type_guid =
                device.partition_type().await.ok().map(|guid| uuid::Uuid::from_bytes(guid.clone()));
            let size = device
                .get_block_info()
                .await
                .ok()
                .map(|info| size_as_string(info.block_count * info.block_size as u64))
                .unwrap_or_else(|| "?".to_string());
            log::info!(
                source:% = device.source(),
                path:% = device.path(),
                content_format:?,
                label:?,
                type_guid:?,
                size:%;
                "Matching device"
            );
            // Publish devices we find as needed.
            // If we get an error from this just log a warning and continue.
            if let Err(error) = environment.publish_device_to_debug_block(device.as_ref(), &name) {
                log::warn!(error:?; "Failed to publish block device");
            }

            let device_path = device.path().to_string();
            match self.matcher.match_device(device, &mut *environment).await {
                Ok(true) => (),
                Ok(false) => log::trace!(path:% = device_path; "ignored"),
                Err(error) => {
                    log::error!(
                        path:% = device_path,
                        error:?;
                        "Failed to match device",
                    );
                }
            };
        }
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        self.environment.lock().await.shutdown().await
    }
}
