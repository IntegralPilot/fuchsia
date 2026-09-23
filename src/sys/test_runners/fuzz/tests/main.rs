// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, Result, bail};
use diagnostics_data::LogsData;
use fidl::endpoints::{create_proxy, create_request_stream};
use fidl_fuchsia_fuzzer::{self as fuzz, Result_ as FuzzResult};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::{AsyncReadExt, AsyncWriteExt, TryStreamExt, join};
use serde_json::Deserializer;
use std::cell::RefCell;
use std::rc::Rc;

const FUZZER_URL: &str = "fuchsia-pkg://fuchsia.com/fuzz-test-runner-tests#meta/fuzzer.cm";
const BUF_SIZE: u32 = 32768;

// Connects to the fuzz-manager, and then connects a controller to the fuzzer.
async fn setup() -> (fuzz::ManagerProxy, fuzz::ControllerProxy, fasync::Task<()>) {
    let fuzz_manager =
        connect_to_protocol::<fuzz::ManagerMarker>().expect("failed to connect fuzz-manager");
    let (controller, log_task) = connect(&fuzz_manager).await;
    let options = fuzz::Options { seed: Some(1), ..Default::default() };
    let response = controller
        .configure(&options)
        .await
        .unwrap_or_else(|e| panic!("{}: {:?}", controller_name("Configure"), e));
    assert!(response.is_ok());
    (fuzz_manager, controller, log_task)
}

// Connects a controller to the fuzzer using the fuzz-manager.
async fn connect(fuzz_manager: &fuzz::ManagerProxy) -> (fuzz::ControllerProxy, fasync::Task<()>) {
    let (controller, server_end) = create_proxy::<fuzz::ControllerMarker>();
    let status = fuzz_manager
        .connect(FUZZER_URL, server_end)
        .await
        .unwrap_or_else(|e| panic!("{}: {:?}", manager_name("Connect"), e));
    assert_eq!(status, Ok(()));

    // Forward syslog. This isn't strictly necessary for testing, but is very useful when debugging.
    let (rx, tx) = zx::Socket::create_stream();
    let status = fuzz_manager
        .get_output(FUZZER_URL, fuzz::TestOutput::Syslog, tx)
        .await
        .unwrap_or_else(|e| panic!("{}: {:?}", manager_name("GetOutput"), e));
    assert_eq!(status, Ok(()));
    let log_task = fasync::Task::spawn(async move {
        let mut socket = fasync::Socket::from_socket(rx);
        let mut buf: [u8; BUF_SIZE as usize] = [0; BUF_SIZE as usize];
        let mut logs = Vec::new();
        loop {
            let len = socket.read(&mut buf).await.expect("failed to read syslog socket");
            if len == 0 {
                break;
            }
            let line = std::str::from_utf8(&buf[..len]).expect("failed to convert syslog to UTF-8");
            let deserializer = Deserializer::from_str(line);
            let iter = deserializer.into_iter::<LogsData>();
            for deserialized in iter {
                let data = match deserialized {
                    Ok(data) => data,
                    Err(_) => continue,
                };
                let msg = data.msg().unwrap_or("<missing>");
                logs.push(format!("[{}] {}", data.metadata.severity, msg));
            }
        }
        eprintln!("{}", logs.join("\n"));
    });
    (controller, log_task)
}

// Constructs a `fuzz::Input` that can be sent over FIDL, and the socket to write into it.
fn make_fidl_input(input: &str) -> Result<(fasync::Socket, fuzz::Input)> {
    let (rx, tx) = zx::Socket::create_stream();
    let fidl_input = fuzz::Input { socket: rx, size: input.len() as u64 };
    let tx = fasync::Socket::from_socket(tx);
    Ok((tx, fidl_input))
}

// Receives data from the socket in the |input|.
async fn recv_input(input: fuzz::Input) -> Result<Vec<u8>> {
    let mut buf = vec![0; input.size as usize];
    let mut rx = fasync::Socket::from_socket(input.socket);
    rx.read_exact(&mut buf).await.context("Async socket read failed")?;
    Ok(buf)
}

// Gets an artifact after a long-running workflow.
async fn watch_artifact(
    controller: &fuzz::ControllerProxy,
) -> Result<(FuzzResult, Option<Vec<u8>>)> {
    let artifact = controller.watch_artifact().await.context(controller_name("WatchArtifact"))?;
    if let Some(e) = artifact.error {
        bail!("workflow returned error: {:?}", e)
    }
    let fuzz_result = artifact.result.unwrap_or(FuzzResult::NoErrors);
    let input = match artifact.input {
        Some(input) => {
            let input = recv_input(input).await?;
            Some(input)
        }
        None => None,
    };
    Ok((fuzz_result, input))
}

// Simple |fuchsia.fuzzer.CorpusReader| implementation.
async fn serve_corpus_reader(stream: fuzz::CorpusReaderRequestStream) -> Result<Vec<String>> {
    // let corpus: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let corpus = Rc::new(RefCell::new(Vec::new()));
    stream
        .try_for_each(|request| async {
            // let corpus_for_request = corpus.clone();
            match request {
                fuzz::CorpusReaderRequest::Next { test_input, responder } => {
                    let status = match recv_input(test_input).await {
                        Ok(received) => {
                            // let mut corpus = corpus_for_request.lock().await;
                            let as_str = std::str::from_utf8(&received).unwrap().to_string();
                            if !as_str.is_empty() {
                                let mut corpus_mut = corpus.borrow_mut();
                                corpus_mut.push(as_str);
                            }
                            zx::sys::ZX_OK
                        }
                        Err(_) => zx::Status::INTERNAL.into_raw(),
                    };
                    responder.send(status)
                }
            }
        })
        .await
        .context("CorpusReader failed")?;
    let corpus = corpus.borrow();
    Ok(corpus.clone())
}

// Handles status updates published by the fuzzer.
async fn subscribe_to_updates(
    stream: fuzz::MonitorRequestStream,
) -> Result<Vec<fuzz::UpdateReason>> {
    let reasons = Rc::new(RefCell::new(Vec::new()));
    stream
        .try_for_each(|request| {
            futures::future::ready(match request {
                fuzz::MonitorRequest::Update { reason, status: _, responder } => {
                    {
                        let mut reasons = reasons.borrow_mut();
                        reasons.push(reason);
                    }
                    responder.send()
                }
            })
        })
        .await
        .expect("Monitor error");
    let reasons = reasons.borrow();
    Ok(reasons.clone())
}

// Helper function return a `Ok(_)` or `Err(_)` depending on whether the arguments are equal or not,
// respectively.
fn compare<T: PartialEq>(x: &T, y: T) -> Result<()> {
    if x.ne(&y) {
        bail!("not equal");
    }
    Ok(())
}

// Returns the name of the fuzz-manager FIDL method.
fn manager_name(method: &str) -> String {
    format!("fuchsia.fuzzer.Manager/{}", method)
}

// Returns the name of the fuzzer controller FIDL method.
fn controller_name(method: &str) -> String {
    format!("fuchsia.fuzzer.Controller/{}", method)
}

// Stops the fuzzer and dumps the log if the test resulted in an error.
async fn teardown(
    fuzz_manager: fuzz::ManagerProxy,
    test_result: Result<()>,
    log_task: fasync::Task<()>,
) -> Result<()> {
    let stop_result = stop(&fuzz_manager).await.map_err(Error::msg);
    log_task.await;
    test_result.and(stop_result)
}

// Stops the fuzzer.
async fn stop(fuzz_manager: &fuzz::ManagerProxy) -> Result<(), zx::Status> {
    match fuzz_manager.stop(FUZZER_URL).await {
        Ok(result) => result.map_err(zx::Status::err_from_raw),
        Err(e) => {
            eprintln!("fuchsia.fuzzer/Manager.Stop: {}", e);
            Err(zx::Status::INTERNAL)
        }
    }
}

#[fuchsia::test]
async fn test_reconnect() -> Result<()> {
    let fuzz_manager =
        connect_to_protocol::<fuzz::ManagerMarker>().expect("failed to connect to fuzz-manager");

    // Set an option.
    let (controller1, log_task1) = connect(&fuzz_manager).await;
    let test1 = || async move {
        let options = fuzz::Options { dictionary_level: Some(13), ..Default::default() };
        let response =
            controller1.configure(&options).await.context(controller_name("Configure"))?;
        response.map_err(Error::msg)
    };
    let test_result1 = test1().await;

    // Now reconnect and use the option to verify the instance is the same.
    let (controller2, log_task2) = connect(&fuzz_manager).await;
    let test2 = || async move {
        let options = controller2.get_options().await.context(controller_name("GetOptions"))?;
        compare(&options.dictionary_level, Some(13))
    };
    let test_result2 = test2().await;

    // Stop the fuzzer and join the logs.
    let stop_result = stop(&fuzz_manager).await.map_err(Error::msg);
    if let Err(e) = test_result1.and(test_result2).and(stop_result) {
        let _ = join!(log_task1, log_task2);
        return Err(e);
    }
    Ok(())
}

#[fuchsia::test]
async fn test_stop() -> Result<()> {
    let (fuzz_manager, _controller, log_task) = setup().await;

    // Stop.
    stop(&fuzz_manager).await.map_err(Error::msg)?;

    // Stop when already stopped.
    let response = stop(&fuzz_manager).await;
    assert_eq!(response, Err(zx::Status::NOT_FOUND));

    log_task.await;
    Ok(())
}

#[fuchsia::test]
async fn test_configure() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let options = fuzz::Options { max_input_size: Some(1024), ..Default::default() };
        let response =
            controller.configure(&options).await.context(controller_name("Configure"))?;
        response.map_err(Error::msg)?;

        let options = controller.get_options().await.context(controller_name("GetOptions"))?;
        compare(&options.max_input_size, Some(1024))
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_fuzz_until_crash() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let response = controller.fuzz().await.context(controller_name("Fuzz"))?;
        response.map_err(Error::msg)?;
        let (fuzz_result, input) = watch_artifact(&controller).await?;
        compare(&fuzz_result, FuzzResult::Crash)?;
        compare(&input, Some(b"CRASH".to_vec()))
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_fuzz_until_runs() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let options = fuzz::Options {
            seed: Some(1),
            runs: Some(100),
            max_input_size: Some(4),
            ..Default::default()
        };
        let response =
            controller.configure(&options).await.context(controller_name("Configure"))?;
        response.map_err(Error::msg)?;
        let (client_end, stream) = create_request_stream::<fuzz::MonitorMarker>();
        controller.add_monitor(client_end).await.context(controller_name("AddMonitor"))?;
        let results = join!(controller.fuzz(), subscribe_to_updates(stream));
        let response = results.0.context(controller_name("Fuzz"))?;
        response.map_err(Error::msg)?;
        let (fuzz_result, _input) = watch_artifact(&controller).await?;
        compare(&fuzz_result, FuzzResult::NoErrors)?;
        let reasons = results.1.context("failed to get updates")?;
        compare(&reasons.first(), Some(&fuzz::UpdateReason::Init))?;
        compare(&reasons.last(), Some(&fuzz::UpdateReason::Done))
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_try_one_no_errors() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let input = "NoErrors";
        let (mut tx, fidl_input) = make_fidl_input(input).context("failed to make FIDL input")?;
        let results = join!(tx.write_all(input.as_bytes()), controller.try_one(fidl_input));
        results.0.context("failed to send input")?;
        let response = results.1.context(controller_name("TryOne"))?;
        response.map_err(Error::msg)?;
        let (fuzz_result, _input) = watch_artifact(&controller).await?;
        compare(&fuzz_result, FuzzResult::NoErrors)
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_try_one_crash() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let input = "CRASH";
        let (mut tx, fidl_input) = make_fidl_input(input).context("failed to make FIDL input")?;
        let results = join!(tx.write_all(input.as_bytes()), controller.try_one(fidl_input));
        results.0.context("failed to send input")?;
        let response = results.1.context(controller_name("TryOne"))?;
        response.map_err(Error::msg)?;
        let (fuzz_result, _input) = watch_artifact(&controller).await?;
        compare(&fuzz_result, FuzzResult::Crash)
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_minimize() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let input = "THIS CRASH CAN BE MINIMIZED";
        let (mut tx, fidl_input) = make_fidl_input(input).context("failed to make FIDL input")?;
        let results = join!(tx.write_all(input.as_bytes()), controller.minimize(fidl_input));
        results.0.context("failed to send input")?;
        let response = results.1.context("FIDL failure")?;
        response.map_err(Error::msg)?;
        let (fuzz_result, input) = watch_artifact(&controller).await?;
        compare(&fuzz_result, FuzzResult::Minimized)?;
        compare(&input, Some(b"CRASH".to_vec()))
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_cleanse() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let input = "AAAACRASHAAAA";
        let (mut tx, fidl_input) = make_fidl_input(input).context("failed to make FIDL input")?;
        let results = join!(tx.write_all(input.as_bytes()), controller.cleanse(fidl_input));
        results.0.context("failed to send input")?;
        let response = results.1.context("FIDL failure")?;
        response.map_err(Error::msg)?;
        let (fuzz_result, input) = watch_artifact(&controller).await?;
        compare(&fuzz_result, FuzzResult::Cleansed)?;
        compare(&input, Some(b"    CRASH    ".to_vec()))
    };
    teardown(fuzz_manager, test().await, log_task).await
}

#[fuchsia::test]
async fn test_merge() -> Result<()> {
    let (fuzz_manager, controller, log_task) = setup().await;
    let test = || async move {
        let seed_inputs = vec!["C", "AS"];
        for input in seed_inputs.iter() {
            let (mut tx, fidl_input) =
                make_fidl_input(input).context("failed to make FIDL input")?;
            let results = join!(
                tx.write_all(input.as_bytes()),
                controller.add_to_corpus(fuzz::Corpus::Seed, fidl_input)
            );
            results.0.context("failed to send input")?;
            let result = results.1.context(controller_name("AddToCorpus"))?;
            result.map_err(Error::msg)?;
        }

        let live_inputs = vec!["CR", "CRY", "CASH", "CRASS", "CRAM", "CRAMS", "SCRAM"];
        for input in live_inputs {
            let (mut tx, fidl_input) =
                make_fidl_input(input).context("failed to make FIDL input")?;
            let results = join!(
                tx.write_all(input.as_bytes()),
                controller.add_to_corpus(fuzz::Corpus::Live, fidl_input)
            );
            results.0.context("failed to send input")?;
            let result = results.1.context(controller_name("AddToCorpus"))?;
            result.map_err(Error::msg)?;
        }

        let response = controller.merge().await.context(controller_name("Merge"))?;
        response.map_err(Error::msg)?;

        // Get the seed corpus.
        let (client_end, stream) = create_request_stream::<fuzz::CorpusReaderMarker>();
        let results = join!(
            controller.read_corpus(fuzz::Corpus::Seed, client_end),
            serve_corpus_reader(stream),
        );
        results.0.context(controller_name("ReadCorpus (seed)"))?;
        let actual_seed = results.1.context("failed to serve seed corpus reader")?;
        compare(&actual_seed, seed_inputs.iter().map(|s| s.to_string()).collect())?;

        // Get the live corpus.
        let (client_end, stream) = create_request_stream::<fuzz::CorpusReaderMarker>();
        let results = join!(
            controller.read_corpus(fuzz::Corpus::Live, client_end),
            serve_corpus_reader(stream),
        );
        results.0.context(controller_name("ReadCorpus (live)"))?;
        let actual_live = results.1.context("failed to serve live corpus reader")?;

        // The fake runner "measures" inputs by setting the number of features to matching prefix
        // length. The merge algorithm sorts inputs by shortest first, then most features, and it keeps
        // any inputs that add new features.
        compare(&actual_live, vec!["CR", "CRAM", "CRASS"].iter().map(|s| s.to_string()).collect())
    };
    teardown(fuzz_manager, test().await, log_task).await
}
