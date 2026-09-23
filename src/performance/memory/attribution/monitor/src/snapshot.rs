// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use attribution_processing::AttributionData;
use attribution_processing::digest::BucketDefinition;
use fidl_fuchsia_memory_attribution_plugin as fplugin;
use fuchsia_trace::duration;
use futures::AsyncWriteExt;
use stalls::MemoryStallMetrics;
use stalls::refaults::RefaultProvider;
use traces::CATEGORY_MEMORY_CAPTURE;
use zstd::stream::raw::CParameter::ChecksumFlag;

/// AttributionSnapshot holds and serves a snapshot of the memory of a Fuchsia system, to be sent
/// to a ffx command on a host.
pub struct AttributionSnapshot(fplugin::Snapshot);

impl AttributionSnapshot {
    pub fn new(
        attribution_data: AttributionData,
        kernel_statistics: fplugin::KernelStatistics,
        memory_stalls: MemoryStallMetrics,
        refault_provider: impl RefaultProvider,
        bucket_definitions: &[BucketDefinition],
    ) -> AttributionSnapshot {
        AttributionSnapshot(fplugin::Snapshot {
            attributions: Some(
                attribution_data.attributions.into_iter().map(|a| a.into()).collect(),
            ),
            principals: Some(
                attribution_data.principals_vec.into_iter().map(|p| p.into()).collect(),
            ),
            resources: Some(attribution_data.resources_vec.into_iter().map(|r| r.into()).collect()),
            resource_names: Some(
                attribution_data.resource_names.iter().map(|n| *n.buffer()).collect(),
            ),
            kernel_statistics: Some(kernel_statistics.into()),
            performance_metrics: Some(fplugin::PerformanceImpactMetrics {
                some_memory_stalls_ns: memory_stalls.some.as_nanos().try_into().ok(),
                full_memory_stalls_ns: memory_stalls.full.as_nanos().try_into().ok(),
                page_refaults: Some(refault_provider.get_count()),
                ..Default::default()
            }),
            bucket_definitions: Some(
                bucket_definitions
                    .iter()
                    .map(|b| fplugin::BucketDefinition {
                        name: Some(b.name.clone()),
                        process: b.process.as_ref().map(|r| r.as_str().into()),
                        vmo: b.vmo.as_ref().map(|r| r.as_str().into()),
                        principal: b.principal.as_ref().map(|r| r.as_str().into()),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        })
    }

    pub async fn serve(self, socket: zx::Socket) -> anyhow::Result<()> {
        duration!(CATEGORY_MEMORY_CAPTURE, "serve:snapshort");
        let data: Vec<u8> = {
            duration!(CATEGORY_MEMORY_CAPTURE, "persist:serve:snapshot");
            fidl::persist(&self.0).unwrap()
        };
        std::mem::drop(self);

        let compressed_data = {
            duration!(CATEGORY_MEMORY_CAPTURE, "compress:serve:snapshot", "uncompressed"=>data.len());
            // zstd does not support async write to a socket.
            // Use bulk compression instead, to avoid stopping the event loop with blocking writes.
            let mut compressor = zstd::bulk::Compressor::new(3)?;
            compressor.context_mut().set_parameter(ChecksumFlag(true)).map_err(|code| {
                anyhow!("Failed to enable checksum: {}", zstd::zstd_safe::get_error_name(code))
            })?;
            compressor.compress(&data)?
        };
        std::mem::drop(data);

        {
            duration!(CATEGORY_MEMORY_CAPTURE, "send:serve:snapshot", "compressed"=>compressed_data.len());
            fidl::AsyncSocket::from_socket(socket).write_all(&compressed_data).await?;
        }
        Ok(())
    }
}

impl Drop for AttributionSnapshot {
    fn drop(&mut self) {
        fuchsia_async::Task::local(async {
            let _ = scudo::mallopt(scudo::M_PURGE_ALL, 0);
        })
        .detach();
    }
}

#[cfg(test)]
pub mod tests {
    // use super::*;
    use crate::snapshot::{AttributionSnapshot, fplugin};

    #[fuchsia::test]
    async fn test_serve_socket_gets_closed() {
        let (s1, s2) = zx::Socket::create_stream();
        let serve_task = fuchsia_async::Task::spawn(
            AttributionSnapshot(fplugin::Snapshot { ..Default::default() }).serve(s1),
        );
        // Close the socket without reading it til the, end.
        s2.half_close().unwrap();
        // Verify that the process does not panic and returns an error.
        assert!(serve_task.await.is_err());
    }
}
