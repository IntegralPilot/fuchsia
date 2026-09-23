// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_component::BinderMarker;
use fidl_fuchsia_starnix_container::{ControllerMarker, ControllerSetSyscallLogFilterRequest};
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_path};
use futures::StreamExt;
use log::info;

#[fuchsia::main]
async fn main() {
    info!("Subscribing to logs...");
    let mut logs = ArchiveReader::logs().snapshot_then_subscribe().expect("subscribed to logs");

    info!("Connecting to starnix controller...");
    let controller =
        connect_to_protocol::<ControllerMarker>().expect("failed to connect to starnix controller");

    info!("Starting workload...");
    let _workload_binder =
        connect_to_protocol_at_path::<BinderMarker>("/svc/fuchsia.component.Binder.workload")
            .expect("should connect to workload binder");

    info!("Enabling syscall log filter...");
    controller
        .set_syscall_log_filter(&ControllerSetSyscallLogFilterRequest {
            process_name: Some("workload".to_string()),
            ..Default::default()
        })
        .await
        .expect("fidl error")
        .expect("set filter error");

    info!("Verifying logs...");
    while let Some(log_result) = logs.next().await {
        let log = log_result.expect("log error");
        let msg = log.msg().unwrap_or("");

        // Check if the log message is from Starnix and contains our target syscall trace.
        if msg.contains("getuid") {
            info!("Found expected log: {log:#?}");
            break;
        }
    }

    info!("Clearing syscall log filters...");
    controller.clear_syscall_log_filters().await.expect("fidl error");
    let clear_time = zx::BootInstant::get();

    info!("Verifying logs stopped...");
    let mut latch_loop_frame = false;
    let mut frames_seen = 0;

    while let Some(log_result) = logs.next().await {
        let log = log_result.expect("log error");
        let msg = log.msg().unwrap_or("");

        if msg.contains("looping") {
            // If we see a looping message that was definitely generated after we cleared the
            // filter, we can be sure that any subsequent syscalls in this (or next) loop iteration
            // should be filtered.
            if log.metadata.timestamp > clear_time {
                if !latch_loop_frame {
                    info!("Latched onto post-clear workload loop. Monitoring for failures.");
                    latch_loop_frame = true;
                }
            }
            if latch_loop_frame {
                frames_seen += 1;
                if frames_seen >= 3 {
                    break;
                }
            }
        }

        if msg.contains("getuid") {
            if latch_loop_frame {
                info!(
                    "Found syscall log AFTER a post-clear marker. Resetting frames_seen. Log: {log:#?}"
                );
                frames_seen = 0;
            } else {
                info!("Ignoring latent/racy syscall log from before confirmed clear: {log:#?}");
            }
        }
    }
}
