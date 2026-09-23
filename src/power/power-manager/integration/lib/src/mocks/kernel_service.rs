// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io::DirectoryMarker;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use fuchsia_sync::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::*;
use std::sync::Arc;
use {fidl_fuchsia_kernel as fkernel, fuchsia_async as fasync};

/// Mocks the fuchsia.kernel.Stats service to be used in integration tests.
pub struct MockKernelService {
    cpu_stats: Mutex<fkernel::CpuStats>,
}

impl MockKernelService {
    pub fn new() -> Arc<MockKernelService> {
        Arc::new(Self {
            cpu_stats: Mutex::new(fkernel::CpuStats {
                actual_num_cpus: 0 as u64,
                per_cpu_stats: Some(Vec::new()),
            }),
        })
    }

    pub async fn run(self: Arc<Self>, handles: LocalComponentHandles) -> Result<(), anyhow::Error> {
        self.run_inner(handles.outgoing_dir).await
    }

    async fn run_inner(
        self: Arc<Self>,
        outgoing_dir: ServerEnd<DirectoryMarker>,
    ) -> Result<(), anyhow::Error> {
        let mut fs = ServiceFs::new();
        fs.dir("svc").add_fidl_service(move |mut stream: fkernel::StatsRequestStream| {
            let this = self.clone();
            fasync::Task::local(async move {
                while let Some(stats_request) = stream.try_next().await.unwrap() {
                    match stats_request {
                        fkernel::StatsRequest::GetCpuStats { responder } => {
                            responder
                                .send(&(*this.cpu_stats.lock()))
                                .unwrap_or_else(|e| info!("MockKernelService send error: {:?}", e));
                        }
                        _ => info!("MockKernelService: stats request not supported!"),
                    }
                }
            })
            .detach();
        });

        fs.serve_connection(outgoing_dir).unwrap();
        fs.collect::<()>().await;

        Ok(())
    }

    pub async fn set_cpu_stats(&self, cpu_stats: fkernel::CpuStats) {
        let mut stats = self.cpu_stats.lock();
        *stats = cpu_stats;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_component::client::connect_to_protocol_at_dir_svc;

    #[fuchsia::test]
    async fn test_set_cpu_stats() {
        // Create and serve the mock service
        let (dir, outgoing_dir) = fidl::endpoints::create_proxy::<DirectoryMarker>();
        let mock = MockKernelService::new();
        let _task = fasync::Task::local(mock.clone().run_inner(outgoing_dir));

        // Connect to the mock server
        let stats_client = connect_to_protocol_at_dir_svc::<fkernel::StatsMarker>(&dir).unwrap();

        assert_eq!(
            stats_client.get_cpu_stats().await.unwrap(),
            fkernel::CpuStats { actual_num_cpus: 0 as u64, per_cpu_stats: Some(Vec::new()) }
        );

        let stats = fkernel::CpuStats {
            actual_num_cpus: 0 as u64,
            per_cpu_stats: Some(vec![fkernel::PerCpuStats {
                cpu_number: Some(1 as u32),
                flags: None,
                idle_time: Some(100),
                reschedules: None,
                context_switches: None,
                irq_preempts: None,
                yields: None,
                ints: None,
                timer_ints: None,
                timers: None,
                page_faults: None,
                exceptions: None,
                syscalls: None,
                reschedule_ipis: None,
                generic_ipis: None,
                ..Default::default()
            }]),
        };
        mock.set_cpu_stats(stats).await;
        assert_eq!(
            stats_client.get_cpu_stats().await.unwrap().per_cpu_stats.unwrap()[0]
                .idle_time
                .unwrap(),
            100
        );
    }
}
