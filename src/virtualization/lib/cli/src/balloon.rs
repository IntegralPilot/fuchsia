// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::platform::PlatformServices;
use fidl_fuchsia_virtualization::{
    BalloonControllerMarker, BalloonControllerProxy, GuestMarker, GuestStatus,
};
use guest_cli_args as arguments;
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Table, row};
use std::fmt;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct BalloonStats {
    current_pages: Option<u32>,
    requested_pages: Option<u32>,
    swap_in: Option<u64>,
    swap_out: Option<u64>,
    major_faults: Option<u64>,
    minor_faults: Option<u64>,
    hugetlb_allocs: Option<u64>,
    hugetlb_failures: Option<u64>,
    free_memory: Option<u64>,
    total_memory: Option<u64>,
    available_memory: Option<u64>,
    disk_caches: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum BalloonResult {
    Stats(BalloonStats),
    SetComplete(u32),
    NotRunning,
    NoBalloonDevice,
    Internal(String),
}

impl fmt::Display for BalloonResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BalloonResult::Stats(stats) => {
                let mut table = Table::new();
                table.set_format(*FORMAT_CLEAN);

                table.add_row(row![
                    "current-pages:",
                    stats.current_pages.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "requested-pages:",
                    stats.requested_pages.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "swap-in:",
                    stats.swap_in.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "swap-out:",
                    stats.swap_out.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "major-faults:",
                    stats.major_faults.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "minor-faults:",
                    stats.minor_faults.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "hugetlb-allocations:",
                    stats.hugetlb_allocs.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "hugetlb-failures:",
                    stats.hugetlb_failures.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "free-memory:",
                    stats.free_memory.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "total-memory:",
                    stats.total_memory.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "available-memory:",
                    stats.available_memory.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);
                table.add_row(row![
                    "disk-caches:",
                    stats.disk_caches.map_or_else(|| "UNKNOWN".to_string(), |i| i.to_string())
                ]);

                write!(f, "{}", table)
            }
            BalloonResult::SetComplete(pages) => {
                write!(f, "Resizing memory balloon to {} pages!", pages)
            }
            BalloonResult::NotRunning => write!(f, "The guest is not running"),
            BalloonResult::NoBalloonDevice => write!(f, "The guest has no balloon device"),
            BalloonResult::Internal(err) => write!(f, "Internal failure: {}", err),
        }
    }
}

// Constants from zircon/system/ulib/virtio/include/virtio/balloon.h
const VIRTIO_BALLOON_S_SWAP_IN: u16 = 0;
const VIRTIO_BALLOON_S_SWAP_OUT: u16 = 1;
const VIRTIO_BALLOON_S_MAJFLT: u16 = 2;
const VIRTIO_BALLOON_S_MINFLT: u16 = 3;
const VIRTIO_BALLOON_S_MEMFREE: u16 = 4;
const VIRTIO_BALLOON_S_MEMTOT: u16 = 5;
const VIRTIO_BALLOON_S_AVAIL: u16 = 6; // Available memory as in /proc
const VIRTIO_BALLOON_S_CACHES: u16 = 7; // Disk caches
const VIRTIO_BALLOON_S_HTLB_PGALLOC: u16 = 8; // HugeTLB page allocations
const VIRTIO_BALLOON_S_HTLB_PGFAIL: u16 = 9; // HugeTLB page allocation failures

#[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401253790)
pub async fn connect_to_balloon_controller<P: PlatformServices>(
    services: &P,
    guest_type: arguments::GuestType,
) -> Result<BalloonControllerProxy, BalloonResult> {
    let guest_manager = services
        .connect_to_manager(guest_type)
        .await
        .map_err(|err| BalloonResult::Internal(format!("failed to connect to manager: {}", err)))?;

    let guest_info = guest_manager
        .get_info()
        .await
        .map_err(|err| BalloonResult::Internal(format!("failed to get guest info: {}", err)))?;
    let status = guest_info.guest_status.expect("guest status should always be set");
    if status != GuestStatus::Starting && status != GuestStatus::Running {
        return Err(BalloonResult::NotRunning);
    }

    let (guest_endpoint, guest_server_end) = fidl::endpoints::create_proxy::<GuestMarker>();
    guest_manager
        .connect(guest_server_end)
        .await
        .map_err(|err| BalloonResult::Internal(format!("failed to send msg: {:?}", err)))?
        .map_err(|err| BalloonResult::Internal(format!("failed to connect: {:?}", err)))?;

    let (balloon_controller, balloon_server_end) =
        fidl::endpoints::create_proxy::<BalloonControllerMarker>();
    guest_endpoint
        .get_balloon_controller(balloon_server_end)
        .await
        .map_err(|err| BalloonResult::Internal(format!("failed to send msg: {:?}", err)))?
        .map_err(|_| BalloonResult::NoBalloonDevice)?;

    Ok(balloon_controller)
}

fn handle_balloon_set(balloon_controller: BalloonControllerProxy, num_pages: u32) -> BalloonResult {
    if let Err(err) = balloon_controller.request_num_pages(num_pages) {
        BalloonResult::Internal(format!("failed to request pages: {:?}", err))
    } else {
        BalloonResult::SetComplete(num_pages)
    }
}

async fn handle_balloon_stats(balloon_controller: BalloonControllerProxy) -> BalloonResult {
    let result = balloon_controller.get_balloon_size().await;
    let Ok((current_num_pages, requested_num_pages)) = result else {
        return BalloonResult::Internal(format!("failed to send msg: {:?}", result.unwrap_err()));
    };

    let result = balloon_controller.get_mem_stats().await;
    let Ok((status, mem_stats)) = result else {
        return BalloonResult::Internal(format!("failed to send msg: {:?}", result.unwrap_err()));
    };

    // The device isn't in a good state to query stats. Trying again may succeed.
    if mem_stats.is_none() {
        return BalloonResult::Internal(format!("failed to query stats: {}", status));
    }

    let mut stats = BalloonStats {
        current_pages: Some(current_num_pages),
        requested_pages: Some(requested_num_pages),
        ..BalloonStats::default()
    };

    for stat in mem_stats.unwrap() {
        match stat.tag {
            VIRTIO_BALLOON_S_SWAP_IN => stats.swap_in = Some(stat.val),
            VIRTIO_BALLOON_S_SWAP_OUT => stats.swap_out = Some(stat.val),
            VIRTIO_BALLOON_S_MAJFLT => stats.major_faults = Some(stat.val),
            VIRTIO_BALLOON_S_MINFLT => stats.minor_faults = Some(stat.val),
            VIRTIO_BALLOON_S_MEMFREE => stats.free_memory = Some(stat.val),
            VIRTIO_BALLOON_S_MEMTOT => stats.total_memory = Some(stat.val),
            VIRTIO_BALLOON_S_AVAIL => stats.available_memory = Some(stat.val),
            VIRTIO_BALLOON_S_CACHES => stats.disk_caches = Some(stat.val),
            VIRTIO_BALLOON_S_HTLB_PGALLOC => stats.hugetlb_allocs = Some(stat.val),
            VIRTIO_BALLOON_S_HTLB_PGFAIL => stats.hugetlb_failures = Some(stat.val),
            tag => println!("unrecognized tag: {}", tag),
        }
    }

    BalloonResult::Stats(stats)
}

pub async fn handle_balloon<P: PlatformServices>(
    services: &P,
    args: &arguments::balloon_args::BalloonArgs,
) -> BalloonResult {
    match &args.balloon_cmd {
        arguments::balloon_args::BalloonCommands::Set(args) => {
            let controller = match connect_to_balloon_controller(services, args.guest_type).await {
                Ok(controller) => controller,
                Err(result) => {
                    return result;
                }
            };

            handle_balloon_set(controller, args.num_pages)
        }
        arguments::balloon_args::BalloonCommands::Stats(args) => {
            let controller = match connect_to_balloon_controller(services, args.guest_type).await {
                Ok(controller) => controller,
                Err(result) => {
                    return result;
                }
            };

            handle_balloon_stats(controller).await
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::{ControlHandle, RequestStream, create_proxy_and_stream};
    use fidl_fuchsia_virtualization::MemStat;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use zx_status;

    #[fuchsia::test(allow_stalls = false)]
    async fn balloon_valid_page_num_returns_ok() {
        let (proxy, mut stream) = create_proxy_and_stream::<BalloonControllerMarker>();
        let expected_string = "Resizing memory balloon to 0 pages!";

        let result = handle_balloon_set(proxy, 0);
        let _ = stream
            .next()
            .await
            .expect("Failed to read from stream")
            .expect("Failed to parse request")
            .into_request_num_pages()
            .expect("Unexpected call to Balloon Controller");

        assert_eq!(result.to_string(), expected_string);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn balloon_stats_server_shut_down_returns_err() {
        let (proxy, mut stream) = create_proxy_and_stream::<BalloonControllerMarker>();
        let _task = fasync::Task::spawn(async move {
            let _ = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_get_balloon_size()
                .expect("Unexpected call to Balloon Controller");
            stream.control_handle().shutdown();
        });

        let result = handle_balloon_stats(proxy).await;
        assert_eq!(
            std::mem::discriminant(&result),
            std::mem::discriminant(&BalloonResult::Internal(String::new()))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn balloon_stats_empty_input_returns_err() {
        let (proxy, mut stream) = create_proxy_and_stream::<BalloonControllerMarker>();

        let _task = fasync::Task::spawn(async move {
            let get_balloon_size_responder = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_get_balloon_size()
                .expect("Unexpected call to Balloon Controller");
            get_balloon_size_responder.send(0, 0).expect("Failed to send request to proxy");

            let get_mem_stats_responder = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_get_mem_stats()
                .expect("Unexpected call to Balloon Controller");
            get_mem_stats_responder
                .send(zx_status::Status::INTERNAL.into_raw(), None)
                .expect("Failed to send request to proxy");
        });

        let result = handle_balloon_stats(proxy).await;
        assert_eq!(
            std::mem::discriminant(&result),
            std::mem::discriminant(&BalloonResult::Internal(String::new()))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn balloon_stats_valid_input_returns_valid_string() {
        let test_stats = [
            MemStat { tag: VIRTIO_BALLOON_S_SWAP_IN, val: 2 },
            MemStat { tag: VIRTIO_BALLOON_S_SWAP_OUT, val: 3 },
            MemStat { tag: VIRTIO_BALLOON_S_MAJFLT, val: 4 },
            MemStat { tag: VIRTIO_BALLOON_S_MINFLT, val: 5 },
            MemStat { tag: VIRTIO_BALLOON_S_MEMFREE, val: 6 },
            MemStat { tag: VIRTIO_BALLOON_S_MEMTOT, val: 7 },
            MemStat { tag: VIRTIO_BALLOON_S_AVAIL, val: 8 },
            MemStat { tag: VIRTIO_BALLOON_S_CACHES, val: 9 },
            MemStat { tag: VIRTIO_BALLOON_S_HTLB_PGALLOC, val: 10 },
            MemStat { tag: VIRTIO_BALLOON_S_HTLB_PGFAIL, val: 11 },
        ];

        let current_num_pages = 6;
        let requested_num_pages = 8;
        let (proxy, mut stream) = create_proxy_and_stream::<BalloonControllerMarker>();
        let _task = fasync::Task::spawn(async move {
            let get_balloon_size_responder = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_get_balloon_size()
                .expect("Unexpected call to Balloon Controller");
            get_balloon_size_responder
                .send(current_num_pages, requested_num_pages)
                .expect("Failed to send request to proxy");

            let get_mem_stats_responder = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_get_mem_stats()
                .expect("Unexpected call to Balloon Controller");
            get_mem_stats_responder
                .send(0, Some(&test_stats))
                .expect("Failed to send request to proxy");
        });

        let result = handle_balloon_stats(proxy).await;
        assert_eq!(
            result.to_string(),
            concat!(
                " current-pages:        6 \n",
                " requested-pages:      8 \n",
                " swap-in:              2 \n",
                " swap-out:             3 \n",
                " major-faults:         4 \n",
                " minor-faults:         5 \n",
                " hugetlb-allocations:  10 \n",
                " hugetlb-failures:     11 \n",
                " free-memory:          6 \n",
                " total-memory:         7 \n",
                " available-memory:     8 \n",
                " disk-caches:          9 \n",
            )
        );
    }
}
