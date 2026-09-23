// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use diagnostics_reader::ArchiveReader;
use fidl::Socket;
use fidl_fuchsia_component::BinderMarker;
use fidl_fuchsia_tracing_controller::{
    ProvisionerMarker, SessionMarker as TracingSessionMarker, StartOptions, StopOptions,
    TraceConfig,
};
use fuchsia_async::{Socket as AsyncSocket, Task};
use fuchsia_component::client::connect_to_protocol;
use futures::{AsyncReadExt, StreamExt};
use log::info;
use parse_starnix_inspect::CoredumpReport;
use std::io::Write;

#[fuchsia::main]
async fn main() {
    info!("initializing tracing...");
    let trace_provisioner = connect_to_protocol::<ProvisionerMarker>().unwrap();
    let (tracing_socket, tracing_socket_write) = Socket::create_stream();
    let mut tracing_socket = AsyncSocket::from_socket(tracing_socket);
    let (tracing_session, server_end) = fidl::endpoints::create_proxy::<TracingSessionMarker>();
    trace_provisioner
        .initialize_tracing(
            server_end,
            &TraceConfig {
                categories: Some(vec!["starnix".to_string()]),
                // Since oneshot mode is used, set the buffer size as large
                // as possible so that trace events don't get dropped.
                buffer_size_megabytes_hint: Some(64),
                ..Default::default()
            },
            tracing_socket_write,
        )
        .unwrap();

    // Start reading from the trace socket before we launch the component to reduce the risk of
    // missing records.
    let collect_trace = Task::spawn(async move {
        info!("draining trace record socket...");
        let mut buf = Vec::new();
        tracing_socket.read_to_end(&mut buf).await.unwrap();
        info!("trace record socket drained.");
        buf
    });

    info!("starting tracing...");
    tracing_session
        .start_tracing(&StartOptions::default())
        .await
        .expect("starting tracing FIDL")
        .expect("start tracing");

    info!("running hello_world...");
    let mut event_stream = EventStream::open().await.unwrap();

    connect_to_protocol::<BinderMarker>().unwrap();
    assert_eq!(
        EventMatcher::ok()
            .moniker("kmsg_hello_world")
            .wait::<Stopped>(&mut event_stream)
            .await
            .unwrap()
            .result()
            .unwrap()
            .status,
        // Our Linux program requests a SIGABRT so there's a coredump report in inspect.
        ExitStatus::Crash(11),
    );

    info!("stopping trace...");
    let _ = tracing_session
        .stop_tracing(&StopOptions { write_results: Some(true), ..Default::default() })
        .await
        .unwrap();

    info!("dropping trace controller...");
    drop(tracing_session);

    info!("waiting for socket collection to complete...");
    let trace = collect_trace.await;

    let fxt_path = format!("/custom_artifacts/trace.fxt");
    info!("creating {fxt_path} and writing trace records...");
    let mut fxt_file = std::fs::File::create(fxt_path).unwrap();
    fxt_file.write_all(&trace[..]).unwrap();

    info!("parsing trace session...");
    let (records, warnings) = fxt::parse_full_session(&trace).unwrap();
    assert!(warnings.is_empty(), "should not encounter any trace parsing warnings");

    let kernel_inspect = ArchiveReader::inspect()
        .select_all_for_component("kernel")
        .with_minimum_schema_count(1)
        .snapshot()
        .await
        .unwrap()[0]
        .clone();
    let coredumps = CoredumpReport::extract_from_snapshot(&kernel_inspect).unwrap();
    assert_eq!(
        coredumps.len(),
        1,
        "exactly one task should have ran and crashed in the kernel, saw {coredumps:?}"
    );
    assert_eq!(coredumps[0].argv, "data/tests/linux_kmsg_hello_world");

    let hello_world_pid = coredumps[0].process_koid;
    let hello_world_tid = coredumps[0].thread_koid;

    let hello_world_trace_events = records
        .iter()
        .filter_map(|r| match r {
            fxt::TraceRecord::Event(e)
                if e.process.0 == hello_world_pid && e.thread.0 == hello_world_tid =>
            {
                Some(e)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!hello_world_trace_events.is_empty(), "must see trace events with expected pid/tid");
    for event in hello_world_trace_events {
        assert_eq!(event.category, "starnix", "starnix trace events should have starnix category");
    }

    let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();
    let hello_world_msg = loop {
        let next = logs.next().await.unwrap().unwrap();
        if next.msg() == Some("Hello, Starnix logs!") {
            break next;
        }
    };
    assert_eq!(hello_world_msg.metadata.pid.unwrap(), hello_world_pid);
    assert_eq!(hello_world_msg.metadata.tid.unwrap(), hello_world_tid);
}
