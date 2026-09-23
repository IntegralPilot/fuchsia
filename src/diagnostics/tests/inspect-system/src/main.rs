// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod diagnostics;
mod metrics;

use anyhow::{Error, bail};
use diagnostics::RequestId;
use diagnostics_reader::{ArchiveReader, RetryConfig};
use fasync::MonotonicDuration;
use fidl_fuchsia_diagnostics::{
    ArchiveAccessorMarker, ArchiveAccessorProxy, BatchIteratorMarker, ClientSelectorConfiguration,
    DataType, Format, SelectorArgument, StreamMode, StreamParameters,
};
use fuchsia_component::server::ServiceFs;
use fuchsiaperf::Unit;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{AsyncWriteExt, SinkExt, StreamExt};
use log::{debug, error, info};
use metrics::{MetricSet, MetricTypeHint};
use rust_measure_tape_for_case::Measurable as _;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Instant;
use zx::sys::ZX_CHANNEL_MAX_MSG_BYTES;
use {fidl_fuchsia_test as ftest, fuchsia_async as fasync};

// Ensure that only a single operation is running at a time.
// Running multiple tests in parallel will give noisy benchmark results.
static OPERATION_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const NUM_TRIALS: usize = 25;

#[fuchsia::main(logging_tags=["inspect-system-test"], logging_minimum_severity="info")]
async fn main() -> Result<(), Error> {
    info!("Starting inspect system test runner");

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(async move {
            suite_connection_handler(stream).await;
        })
        .detach();
    });

    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;

    Ok(())
}

// Implements the Suite protocol for this test.
async fn suite_connection_handler(mut stream: ftest::SuiteRequestStream) {
    let id = RequestId::new();
    info!("{} New suite stream", id);

    let (mut task_tx, task_rx) = mpsc::unbounded::<fasync::Task<()>>();
    let _task_handler = fasync::Task::spawn(async move {
        task_rx.for_each_concurrent(None, |task| task).await;
    });

    while let Some(Ok(val)) = stream.next().await {
        let rid = id.new_request();
        match val {
            ftest::SuiteRequest::GetTests { iterator, .. } => {
                info!("{} Starting GetTests", rid);
                task_tx
                    .send(fasync::Task::spawn(async move {
                        let mut stream = iterator.into_stream();
                        let resp = match get_test_cases(rid).await {
                            Ok(v) => v,
                            Err(e) => {
                                error!("{} Failed to get test cases: {:?}", rid, e);
                                return;
                            }
                        };
                        let mut remaining_cases = &resp[..];
                        while let Some(Ok(req)) = stream.next().await {
                            match req {
                                ftest::CaseIteratorRequest::GetNext { responder } => {
                                    let mut bytes = 32; // overhead for header + vector
                                    let mut case_count = 0;
                                    for case in remaining_cases {
                                        bytes += case.measure().num_bytes;
                                        if bytes > ZX_CHANNEL_MAX_MSG_BYTES as usize {
                                            break;
                                        }
                                        case_count += 1;
                                    }
                                    responder.send(&remaining_cases[..case_count]).ok();
                                    remaining_cases = &remaining_cases[case_count..];
                                }
                            }
                        }
                    }))
                    .await
                    .ok();
            }
            ftest::SuiteRequest::Run { tests, listener, .. } => {
                info!("{} Starting Run", rid);
                let proxy = listener.into_proxy();
                task_tx
                    .send(fasync::Task::spawn(async move {
                        if let Err(e) = handle_run(rid, tests, proxy).await {
                            error!("{} failed to complete run: {:?}", rid, e);
                        }
                        info!("{} Done Run", rid);
                    }))
                    .await
                    .ok();
            }
        }
        info!("{} Done request", rid);
    }
}

// Handles operations related to a single test run, including reporting results over the RunListener.
async fn handle_run(
    rid: RequestId,
    tests: Vec<ftest::Invocation>,
    listener: ftest::RunListenerProxy,
) -> Result<(), Error> {
    for test in tests {
        let name = test.name.as_ref().ok_or_else(|| anyhow::format_err!("Missing name"))?.clone();
        debug!("{} Queuing for {}", rid, name);
        let _lock = OPERATION_MUTEX.lock().await;
        debug!("{} Starting {}", rid, name);

        let (stdout_tx, stdout_rx) = zx::Socket::create_stream();
        let (stderr_tx, stderr_rx) = zx::Socket::create_stream();
        let (case_proxy, server_end) = fidl::endpoints::create_proxy::<ftest::CaseListenerMarker>();
        listener
            .on_test_case_started(
                &test,
                ftest::StdHandles {
                    out: Some(stdout_rx),
                    err: Some(stderr_rx),
                    ..Default::default()
                },
                server_end,
            )
            .ok();

        let status = match handle_invocation(&name, stdout_tx).await {
            Ok(_) => ftest::Status::Passed,
            Err(e) => {
                let mut stderr_tx = fasync::Socket::from_socket(stderr_tx);
                stderr_tx.write_all(format!("Test failed: {e:?}").as_bytes()).await.ok();
                ftest::Status::Failed
            }
        };
        case_proxy.finished(&ftest::Result_ { status: Some(status), ..Default::default() }).ok();
    }

    listener.on_finished().ok();

    Ok(())
}

// Handles a single invocation of a test, which consists of repeatedly
// querying the Archivist for a specific component's Inspect data and
// reporting statistics on latency and size.
async fn handle_invocation(moniker: &str, stdout: zx::Socket) -> Result<(), Error> {
    let mut stdout = fasync::Socket::from_socket(stdout);

    stdout.write_all(format!("Processing {moniker}\n").as_bytes()).await.ok();
    let proxy: ArchiveAccessorProxy = fuchsia_component::client::connect_to_protocol_at_path::<
        ArchiveAccessorMarker,
    >("/svc/fuchsia.diagnostics.RealArchiveAccessor")?;

    stdout.write_all(b"Reading all selectors and warming up\n").await.ok();

    let mut selectors = vec![];
    for value in ArchiveReader::inspect()
        .add_selector(format!("{moniker}:root"))
        .retry(RetryConfig::never())
        .with_archive(proxy)
        .with_timeout(MonotonicDuration::from_seconds(15))
        .snapshot()
        .await?
    {
        if let Some(payload) = value.payload {
            for (path, maybe_prop) in payload.property_iter() {
                if let Some(prop) = maybe_prop {
                    selectors.push(format!("{}:{}:{}", moniker, path.join("/"), prop.name()));
                }
            }
        }
    }

    // Metrics to collect:
    // - Total time to get snapshot
    const TOTAL_TIME: &str = "Total - Time";
    // - Total size of data in snapshot
    const TOTAL_SIZE: &str = "Total - Size";
    // - Number of batches returned
    const BATCH_COUNT: &str = "Batch - Count";
    // - Time to get each batch
    const BATCH_TIME: &str = "Batch - Time";
    // - Number of entries in each batch
    const BATCH_ENTRIES: &str = "Batch - Entries";
    // - Size of each entry
    const ENTRY_SIZE: &str = "Entry - Size";

    stdout.write_all(format!("Got {} selectors\n", selectors.len()).as_bytes()).await.ok();

    let proxy: ArchiveAccessorProxy = fuchsia_component::client::connect_to_protocol_at_path::<
        ArchiveAccessorMarker,
    >("/svc/fuchsia.diagnostics.RealArchiveAccessor")?;

    let mut metrics = MetricSet::default();
    metrics.set_type_hints(
        [
            (TOTAL_TIME, MetricTypeHint { is_integral: false, unit: Unit::Milliseconds }),
            (TOTAL_SIZE, MetricTypeHint { is_integral: true, unit: Unit::Bytes }),
            (BATCH_TIME, MetricTypeHint { is_integral: false, unit: Unit::Milliseconds }),
            (ENTRY_SIZE, MetricTypeHint { is_integral: true, unit: Unit::Bytes }),
        ]
        .into_iter(),
    );

    for _ in 0..NUM_TRIALS {
        let (iterator, result_stream) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
        let time_start = Instant::now();

        proxy.stream_diagnostics(
            &StreamParameters {
                data_type: Some(DataType::Inspect),
                stream_mode: Some(StreamMode::Snapshot),
                format: Some(Format::Json),
                client_selector_configuration: Some(ClientSelectorConfiguration::Selectors(vec![
                    SelectorArgument::RawSelector(format!("{moniker}:root")),
                ])),
                ..Default::default()
            },
            result_stream,
        )?;

        let mut total_size = 0;
        let mut batch_count = 0;

        loop {
            let time_batch_start = Instant::now();
            match iterator.get_next().await? {
                Ok(contents) => {
                    if contents.is_empty() {
                        break;
                    }
                    batch_count += 1;
                    metrics.add_measurement(BATCH_ENTRIES, contents.len() as f64);
                    metrics.add_measurement(
                        BATCH_TIME,
                        Instant::now().duration_since(time_batch_start).as_secs_f64() * 1000.0,
                    );

                    for content in contents.into_iter() {
                        let buffer = match content {
                            fidl_fuchsia_diagnostics::FormattedContent::Json(buffer) => buffer,
                            _ => {
                                bail!("Unknown format returned for batch");
                            }
                        };
                        total_size += buffer.size;
                        metrics.add_measurement(ENTRY_SIZE, buffer.size as f64);
                    }
                }
                Err(e) => {
                    bail!("Failed to read from Archivist: {:?}", e);
                }
            }
        }

        metrics.add_measurement(BATCH_COUNT, batch_count as f64);
        metrics.add_measurement(TOTAL_SIZE, total_size as f64);
        metrics.add_measurement(
            TOTAL_TIME,
            Instant::now().duration_since(time_start).as_secs_f64() * 1000.0,
        );
    }

    stdout
        .write_all(format!("Completed ReadAll:\n{}", metrics.format_text()).as_bytes())
        .await
        .ok();

    let formatted_moniker = moniker.replace("/", "::");

    let directory = PathBuf::from("custom_artifacts").join(&formatted_moniker);
    std::fs::create_dir_all(&directory)?;
    let mut file = File::create(directory.join("readall.fuchsiaperf.fyi.json"))?;
    metrics.write_fuchsiaperf(&formatted_moniker, &file)?;
    file.flush()?;

    Ok(())
}

// Reads all data from the Archivist and formats it as test case names.
async fn get_test_cases(rid: RequestId) -> Result<Vec<ftest::Case>, Error> {
    debug!("{} Queuing get names", rid);
    let _lock = OPERATION_MUTEX.lock().await;
    debug!("{} Starting get names", rid);

    let proxy: ArchiveAccessorProxy = fuchsia_component::client::connect_to_protocol_at_path::<
        ArchiveAccessorMarker,
    >("/svc/fuchsia.diagnostics.RealArchiveAccessor")?;

    let mut names_seen = HashSet::new();
    let mut ret = vec![];

    for value in ArchiveReader::inspect()
        .retry(RetryConfig::never())
        .with_archive(proxy)
        .with_timeout(MonotonicDuration::from_seconds(60))
        .snapshot()
        .await?
    {
        if names_seen.insert(value.moniker.clone()) {
            ret.push(ftest::Case {
                name: Some(value.moniker.to_string()),
                enabled: Some(true),
                ..Default::default()
            });
        }
    }

    info!("{} Got {} names", rid, ret.len());

    Ok(ret)
}
