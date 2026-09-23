// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_system_device::MagmaSystemDevice;
use crate::traits::Device;
use anyhow::Context;
use fidl_fuchsia_gpu_magma as fidl_magma;
use futures::TryStreamExt;
use std::sync::Arc;

/// This handles the CombinedDevice FIDL requests by forwarding them to the system device.
#[derive(Clone)]
pub struct MagmaCombinedDeviceServer {
    pub system_device: Arc<MagmaSystemDevice>,
}

impl Drop for MagmaCombinedDeviceServer {
    fn drop(&mut self) {
        log::debug!("Dropping MagmaCombinedDeviceServer");
        self.system_device.stop_all_connections();
    }
}

impl MagmaCombinedDeviceServer {
    pub fn new(system_device: MagmaSystemDevice) -> Arc<Self> {
        Arc::new(MagmaCombinedDeviceServer { system_device: Arc::new(system_device) })
    }

    pub async fn run(
        self: Arc<Self>,
        client_type: crate::traits::MagmaClientType,
        mut stream: fidl_magma::CombinedDeviceRequestStream,
    ) -> anyhow::Result<()> {
        while let Some(request) = stream.try_next().await.context("Stream error")? {
            match request {
                fidl_magma::CombinedDeviceRequest::Query { query_id, responder } => {
                    let device = &self.system_device;
                    let (vmo, result) = device
                        .query(query_id.into_primitive() as u64)
                        .map_err(|status| anyhow::anyhow!("Query failed: {}", status))?;

                    let response = match vmo {
                        Some(vmo_handle) => {
                            fidl_magma::DeviceQueryResponse::BufferResult(vmo_handle)
                        }
                        None => fidl_magma::DeviceQueryResponse::SimpleResult(result),
                    };
                    responder.send(Ok(response)).context("Failed to send")?;
                }

                fidl_magma::CombinedDeviceRequest::Connect2 {
                    client_id,
                    primary_channel,
                    notification_channel,
                    ..
                } => {
                    self.system_device.clone().start_connection_thread(
                        client_id,
                        client_type,
                        primary_channel,
                        notification_channel,
                    );
                }

                fidl_magma::CombinedDeviceRequest::DumpState { dump_type, .. } => {
                    self.system_device.dump_status(dump_type);
                }

                fidl_magma::CombinedDeviceRequest::GetIcdList { responder } => {
                    let icd_list = self
                        .system_device
                        .get_icd_list()
                        .map_err(|status| anyhow::anyhow!("GetIcdList failed: {}", status))?;

                    let fidl_icd_list: Vec<_> = icd_list
                        .into_iter()
                        .map(|item| fidl_magma::IcdInfo {
                            component_url: Some(item.url),
                            flags: Some(fidl_magma::IcdFlags::from_bits_truncate(
                                item.support_flags as u32,
                            )),
                            ..fidl_magma::IcdInfo::default()
                        })
                        .collect();
                    responder.send(&fidl_icd_list).context("Failed to send GetIcdList")?;
                }
            }
        }
        Ok(())
    }
}
