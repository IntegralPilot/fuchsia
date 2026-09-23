// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_cpu_ctrl as fcpu_ctrl;
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_component::client::Service;
use futures::{TryFutureExt, TryStreamExt};
use std::cmp::Reverse;
use zx::MonotonicDuration;

const CPU_DRIVER_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(30);

pub async fn get_cpu_ctrl_proxy(
    node_info: &str,
    total_domain_count: u8,
    perf_rank: u8,
) -> Result<fcpu_ctrl::DeviceProxy, Error> {
    if total_domain_count == 0 {
        return Err(anyhow::anyhow!("total_domain_count must be greater than 0"));
    }

    if perf_rank >= total_domain_count {
        return Err(anyhow::anyhow!("perf_rank must be less than total_domain_count"));
    }

    let mut instances = Service::open(fcpu_ctrl::ServiceMarker)
        .expect("failed to open fuchsia.hardware.cpu.ctrl service directory")
        .watch()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create watcher: {:?}", e))?;
    let mut proxies = Vec::new();
    while let Some(instance) = instances
        .try_next()
        .map_err(|e| anyhow::anyhow!("Failed to get service instance: {e:?}"))
        .on_timeout(CPU_DRIVER_TIMEOUT.after_now(), || {
            Err(anyhow::anyhow!("Timeout waiting for next service instance."))
        })
        .await?
    {
        let proxy = instance
            .connect_to_device()
            .map_err(|e| anyhow::anyhow!("Failed to connect to device: {:?}", e))?;

        let relative_perf =
            match proxy.get_relative_performance2().await {
                Ok(Ok(perf)) => perf,
                other => {
                    log::warn!(
                        other:?;
                        "get_relative_performance2 failed, falling back to get_relative_performance"
                    );
                    proxy.get_relative_performance().await?.map_err(|e| {
                        anyhow::anyhow!("GetRelativePerformance returned err: {:?}", e)
                    })? as u64
                }
            };

        let domain_id = match proxy.get_domain_id().await {
            Ok(id) => id,
            e => {
                log::warn!(
                    e:?;
                    "get_domain_id failed, using 0 as domain_id"
                );
                0
            }
        };
        log::info!(node_info:?, domain_id, relative_perf:?; "CPU device detected");
        proxies.push((domain_id, relative_perf, proxy));

        if proxies.len() == total_domain_count as usize {
            // Sort by domain ID first to produce a consistent sorting order where lower domain
            // IDs have lower perf ranks.
            proxies.sort_by_key(|r| r.0);

            // Sort by relative_perf from highest to lowest.
            proxies.sort_by_key(|r| Reverse(r.1));
            return Ok(proxies[perf_rank as usize].2.clone());
        }
    }
    Err(anyhow::anyhow!("Failed to get all devices"))
}
