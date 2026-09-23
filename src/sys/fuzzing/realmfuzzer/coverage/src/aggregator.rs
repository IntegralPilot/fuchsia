// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::options::AsyncOptions;
use anyhow::{Context as _, Result, bail};
use fidl_fuchsia_fuzzer as fuzz;
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::{FutureExt, SinkExt, StreamExt, TryStreamExt, pin_mut, select};
use std::cell::RefCell;
use std::collections::HashMap;

/// Provides coverage data from multiple instrumented processes to the fuzzing engine.
///
/// Coverage data is produced by processes that are compiled with sanitizer coverage instrumentation
/// and linked against realmfuzzer's `target` library. It is consumed by realmfuzzer's `engine`
/// process.
///
/// This object associates the objects needed to:
///   * wait for options to be set,
///   * forward coverage data from producers to the consumer,
///   * cancel waiting for coverage when the consumer disconnects or is replaced, and
///   * replay the previous coverage data for a newly connected consumer.
///
/// This object uses interior mutability, allowing it to be passed by reference to multiple
/// concurrent futures.
///
pub struct Aggregator {
    options: AsyncOptions,
    sender: mpsc::UnboundedSender<fuzz::CoverageData>,
    receiver: Mutex<mpsc::UnboundedReceiver<fuzz::CoverageData>>,
    cancelation: RefCell<oneshot::Sender<()>>,
    cached: RefCell<HashMap<u64, Vec<fuzz::CoverageData>>>,
}

impl Aggregator {
    /// Creates an `Aggregator` suitable to be passed to [`collect_data`] and [`provide_data`].
    pub fn new() -> Self {
        let options = AsyncOptions::new();
        let (sender, receiver) = mpsc::unbounded::<fuzz::CoverageData>();
        let receiver = Mutex::new(receiver);
        let (cancelation, _) = oneshot::channel::<()>();
        let cancelation = RefCell::new(cancelation);
        let cached = RefCell::new(HashMap::new());
        Self { options, sender, receiver, cancelation, cached }
    }

    /// Implements the `fuchsia.fuzzer/CoverageDataCollector.Initialize` method.
    ///
    /// The unique target ID for the process is stored in `target_id_rc`.
    ///
    /// Returns a tuple of a target ID and the options set by the provider.
    async fn initialize(
        &self,
        eventpair: zx::EventPair,
        process: zx::Process,
    ) -> Result<(u64, fuzz::Options)> {
        let koid = process.koid().context("failed to get koid for instrumented process")?;
        let target_id = koid.raw_koid();
        let instrumented = fuzz::InstrumentedProcess { eventpair, process };
        let coverage_data =
            fuzz::CoverageData { target_id, data: fuzz::Data::Instrumented(instrumented) };
        let coverage_data_dup =
            duplicate_data(&coverage_data).context("failed to cache coverage data")?;
        self.cached.borrow_mut().entry(target_id).or_insert(Vec::new()).push(coverage_data_dup);
        let mut sender = self.sender.clone();
        sender.send(coverage_data).await.context("failed to forward coverage data")?;
        Ok((target_id, self.options.get().await))
    }

    /// Implements the `fuchsia.fuzzer/CoverageDataCollector.AddInline8bitCounters` method.
    ///
    /// Retrieves the unique target ID for the associated process from `target_id_rc`.
    ///
    async fn add_inline_8bit_counters(
        &self,
        inline_8bit_counters: zx::Vmo,
        target_id: u64,
    ) -> Result<()> {
        let coverage_data = fuzz::CoverageData {
            target_id,
            data: fuzz::Data::Inline8bitCounters(inline_8bit_counters),
        };
        let coverage_data_dup =
            duplicate_data(&coverage_data).context("failed to cache coverage data")?;
        self.cached.borrow_mut().entry(target_id).or_insert(Vec::new()).push(coverage_data_dup);
        let mut sender = self.sender.clone();
        sender.send(coverage_data).await.context("failed to forward coverage data")
    }

    /// Clears coverage data associated with the process identified by `target_id`. This should be
    /// invoked when an instrumented process disconnects.
    fn remove(&self, target_id: u64) {
        let mut cached_mut = self.cached.borrow_mut();
        cached_mut.remove(&target_id);
    }

    /// Implements the `fuchsia.fuzzer.CoverageDataProvider/SetOptions` method.
    fn set_options(&self, options: fuzz::Options) {
        self.options.set(options);
    }

    /// Implements the `fuchsia.fuzzer.CoverageDataProvider/WatchCoverageData` method.
    ///
    /// As a "hanging get" method, the behavior of this method differs between its first and
    /// subseqeuent calls. On the first call, it immediately returns the current coverage data.
    /// Subsequently, if no new data has become available since the previous call, it will wait for
    /// the coverage data to change before returning an update. Whether a call is the first one or
    /// not is tracked using `first_rc`, which should initially be set to `true`.
    ///
    async fn watch_coverage_data(
        &self,
        first_rc: &RefCell<bool>,
    ) -> Result<Vec<fuzz::CoverageData>> {
        // Typically, only one client at a time is expected. If a second call is made concurrently
        // with another, it should take over from the first. It achieves this by replacing the
        // cancelation channel before trying to acquire the lock on the receiver:
        //
        // 1. Replacing the cancelation interrupts the first call if it is waiting for more coverage
        //    data. By installing a new `cancel_sender`, the previous one is dropped, awakening the
        //    future generated by the first call. Once awoken that future detects the drop and
        //    returns without blocking.
        // 2. Acquiring the lock ensures the second call does not start until the first has
        //    finished.
        let (cancel_sender, cancel_receiver) = oneshot::channel::<()>();
        self.cancelation.replace(cancel_sender);
        let mut receiver = self.receiver.lock().await;
        let mut batch = Vec::new();

        // If this is the first call to `watch_coverage_data` for a particular connection, ignore
        // any in-flight data. Instead, replay cached data, if any. This allows reconnecting engines
        // to get handles to persistent components.
        let mut first = first_rc.borrow_mut();
        if *first {
            while let Ok(Some(_)) = receiver.try_next() {}
            for target_data in self.cached.borrow().values() {
                for coverage_data in target_data.iter() {
                    let coverage_data_dup = duplicate_data(&coverage_data)
                        .context("failed to duplicate coverage data")?;
                    batch.push(coverage_data_dup);
                }
            }
            *first = false;
        } else {
            let cancel_fut = cancel_receiver.fuse();
            pin_mut!(cancel_fut);
            loop {
                match receiver.try_next() {
                    Ok(Some(coverage_data)) => {
                        batch.push(coverage_data);
                        continue;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        // This indicates that there are no messages available, but the channel is
                        // not yet closed. Return available updates.
                        if !batch.is_empty() {
                            break;
                        }
                    }
                };
                // Wait for an update or for the call to be canceled.
                let receive_fut = receiver.next().fuse();
                pin_mut!(receive_fut);
                let received = select! {
                    received = receive_fut => received,
                    _ = cancel_fut => None,
                };
                match received {
                    Some(coverage_data) => {
                        batch.push(coverage_data);
                    }
                    None => break,
                };
            }
        }
        Ok(batch)
    }
}

/// Makes a copy of coverage data.
///
/// The returned coverage data will contain duplicates of the handles in the original.
///
fn duplicate_data(coverage: &fuzz::CoverageData) -> Result<fuzz::CoverageData> {
    let target_id = coverage.target_id;
    match &coverage.data {
        fuzz::Data::Instrumented(instrumented) => {
            let eventpair = instrumented
                .eventpair
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .context("failed to duplicate eventpair")?;
            let process = instrumented
                .process
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .context("failed to duplicate process")?;
            let instrumented = fuzz::InstrumentedProcess { eventpair, process };
            Ok(fuzz::CoverageData { target_id, data: fuzz::Data::Instrumented(instrumented) })
        }
        fuzz::Data::Inline8bitCounters(inline_8bit_counters) => {
            let inline_8bit_counters = inline_8bit_counters
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .context("failed to duplicate VMO")?;
            Ok(fuzz::CoverageData {
                target_id,
                data: fuzz::Data::Inline8bitCounters(inline_8bit_counters),
            })
        }
        fuzz::DataUnknown!() => bail!("Unknown coverage type: {:?}", coverage.data),
    }
}

/// Collects coverage data from instrumented processes.
///
/// This method implements the `fuchsia.fuzzer.CoverageDataCollector` protocol. Coverage data
/// provided to it by clients is saved and forwarded via the given `aggregator`.
///
pub async fn collect_data(
    stream: fuzz::CoverageDataCollectorRequestStream,
    aggregator: &Aggregator,
) -> Result<()> {
    let target_id_rc = &RefCell::new(0);
    stream
        .map(|result| {
            result.context("failed to receive request: fuchsia.fuzzer/CoverageDataCollector")
        })
        .try_for_each(|request| async move {
            match request {
                fuzz::CoverageDataCollectorRequest::Initialize {
                    eventpair,
                    process,
                    responder,
                } => {
                    let (target_id, response) = aggregator
                        .initialize(eventpair, process)
                        .await
                        .context("failed to initialize")?;
                    *target_id_rc.borrow_mut() = target_id;
                    responder
                        .send(&response)
                        .context("failed to send response: CoverageDataCollector.Initialize")?;
                }
                fuzz::CoverageDataCollectorRequest::AddInline8bitCounters {
                    inline_8bit_counters,
                    responder,
                } => {
                    aggregator
                        .add_inline_8bit_counters(inline_8bit_counters, *target_id_rc.borrow())
                        .await
                        .context("failed to add LLVM module")?;
                    responder.send().context(
                        "failed to send response: CoverageDataCollector.AddInline8bitCounters",
                    )?;
                }
            }
            Ok(())
        })
        .await?;
    aggregator.remove(*target_id_rc.borrow());
    Ok(())
}

/// Provide coverage data to the fuzzing engine.
///
/// This method implements the `fuchsia.fuzzer/CoverageDataProvider` protocol. Clients can use it to
/// to retrieve coverage data from the given `aggregator`.
///
pub async fn provide_data(
    stream: fuzz::CoverageDataProviderRequestStream,
    aggregator: &Aggregator,
) -> Result<()> {
    let first_rc = &RefCell::new(true);
    stream
        .map(|result| {
            result.context("failed to receive request: fuchsia.fuzzer/CoverageDataProvider")
        })
        .try_for_each(|request| async move {
            match request {
                fuzz::CoverageDataProviderRequest::SetOptions { options, .. } => {
                    aggregator.set_options(options);
                }
                fuzz::CoverageDataProviderRequest::WatchCoverageData { responder } => {
                    let response = aggregator
                        .watch_coverage_data(first_rc)
                        .await
                        .context("failed to watch coverage data")?;

                    // Ignore failures to send the FIDL response. Clients at some point will
                    // disconnect while waiting for coverage data.
                    let _ = responder.send(response);
                }
            }
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::{Aggregator, collect_data, provide_data};
    use anyhow::{Context as _, Result, bail};
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_fuzzer as fuzz;
    use fuchsia_async as fasync;
    use fuchsia_runtime::process_self;
    use futures::{FutureExt, pin_mut, select, try_join};
    use zx::Peered;

    // Test fixtures.

    // Connect and get the options. Wait for a signal and then add the given number of modules.
    async fn producer(proxy: &fuzz::CoverageDataCollectorProxy, num_modules: usize) -> Result<()> {
        let (local, eventpair) = zx::EventPair::create();
        let process = process_self()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .context("failed to get process")?;
        let options = proxy
            .initialize(eventpair, process)
            .await
            .context("CoverageDataCollector.Initialize")?;
        assert_eq!(options.runs, Some(1000));
        fasync::OnSignals::new(&local, zx::Signals::USER_0)
            .await
            .context("failed to receive signal")?;
        for _ in 0..num_modules {
            let inline_8bit_counters = zx::Vmo::create(32).context("failed to create VMO")?;
            inline_8bit_counters
                .write("hello, world!".as_bytes(), 0)
                .context("failed to write to VMO")?;
            proxy
                .add_inline8bit_counters(inline_8bit_counters)
                .await
                .context("CoverageDataCollector.AddInline8bitCounters")?;
        }
        Ok(())
    }

    // Set the options, and then wait until the given number of modules have been added, signaling
    // any producers as they connect.
    async fn consumer(proxy: &fuzz::CoverageDataProviderProxy, num_modules: usize) -> Result<()> {
        let options = fuzz::Options { runs: Some(1000), ..Default::default() };
        proxy.set_options(&options).context("CoverageDataProvider.SetOptions")?;
        let mut modules_added = 0;
        let koid = process_self().koid().context("failed to get process koid")?;
        while modules_added < num_modules {
            let batch = proxy
                .watch_coverage_data()
                .await
                .context("CoverageDataProvider.WatchCoverageData")?;
            for coverage in batch.into_iter() {
                assert_eq!(koid.raw_koid(), coverage.target_id);
                match &coverage.data {
                    fuzz::Data::Instrumented(instrumented) => {
                        let _ = instrumented
                            .eventpair
                            .signal_peer(zx::Signals::NONE, zx::Signals::USER_0);
                    }
                    fuzz::Data::Inline8bitCounters(inline_8bit_counters) => {
                        let mut data: [u8; 13] = [0; 13];
                        inline_8bit_counters
                            .read(&mut data, 0)
                            .context("failed to read from VMO")?;
                        assert_eq!(data, "hello, world!".as_bytes());
                        modules_added += 1;
                    }
                    fuzz::DataUnknown!() => bail!("Unknown coverage type: {:?}", coverage.data),
                }
            }
        }
        Ok(())
    }

    // Unit tests.

    #[fuchsia::test]
    async fn test_coverage_data_collector_initialize() -> Result<()> {
        let (proxy, stream) = create_proxy_and_stream::<fuzz::CoverageDataCollectorMarker>();

        // Send a fake instrumented process and get the options.
        let initialize = async move {
            let (_, eventpair) = zx::EventPair::create();
            let process = process_self()
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .context("failed to get process")?;
            let options = proxy
                .initialize(eventpair, process)
                .await
                .context("CoverageDataCollector.Initialize")?;
            assert_eq!(options.runs, Some(100));
            assert_eq!(options.max_total_time, Some(2000000));
            assert_eq!(options.seed, Some(3));
            Ok(())
        };

        // Set the options.
        let aggregator = Aggregator::new();
        aggregator.set_options(fuzz::Options {
            runs: Some(100),
            max_total_time: Some(2000000),
            seed: Some(3),
            ..Default::default()
        });
        try_join!(initialize, collect_data(stream, &aggregator))?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_coverage_data_collector_add_inline_8bit_counters() -> Result<()> {
        let (proxy, stream) = create_proxy_and_stream::<fuzz::CoverageDataCollectorMarker>();

        // Send a fake inline 8 bit counters.
        let add_inline_8bit_counters = async move {
            let inline_8bit_counters = zx::Vmo::create(32).context("failed to create VMO")?;
            proxy
                .add_inline8bit_counters(inline_8bit_counters)
                .await
                .context("CoverageDataCollector.AddInline8bitCounters")?;
            Ok(())
        };

        // Just check that it can be sent without error.
        let aggregator = Aggregator::new();
        try_join!(add_inline_8bit_counters, collect_data(stream, &aggregator))?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_coverage_data_provider_set_options() -> Result<()> {
        let (proxy, stream) = create_proxy_and_stream::<fuzz::CoverageDataProviderMarker>();

        // Set the options.
        let set_options = async move {
            let options = fuzz::Options {
                seed: Some(4),
                max_input_size: Some(500),
                mutation_depth: Some(6),
                ..Default::default()
            };
            proxy.set_options(&options).context("CoverageDataProvider.SetOptions")?;
            Ok(())
        };

        let aggregator = Aggregator::new();
        try_join!(set_options, provide_data(stream, &aggregator))?;

        // Check that they were set.
        let (_, eventpair) = zx::EventPair::create();
        let process = process_self()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .context("failed to get process")?;
        let (_, options) =
            aggregator.initialize(eventpair, process).await.context("failed to get options")?;
        assert_eq!(options.seed, Some(4));
        assert_eq!(options.max_input_size, Some(500));
        assert_eq!(options.mutation_depth, Some(6));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_coverage_data_provider_watch_coverage_data() -> Result<()> {
        let (proxy, stream) = create_proxy_and_stream::<fuzz::CoverageDataProviderMarker>();

        // The initial "hanging-get" returns an empty vector immediately.
        let watch_coverage_data = async move {
            let coverage_data = proxy
                .watch_coverage_data()
                .await
                .context("CoverageDataProvider.WatchCoverageData")?;
            assert!(coverage_data.is_empty());
            Ok(())
        };

        let aggregator = Aggregator::new();
        try_join!(watch_coverage_data, provide_data(stream, &aggregator))?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_coverage_single_producer_and_consumer() -> Result<()> {
        let aggregator = Aggregator::new();
        let (proxy1, stream1) = create_proxy_and_stream::<fuzz::CoverageDataCollectorMarker>();
        let (proxy2, stream2) = create_proxy_and_stream::<fuzz::CoverageDataProviderMarker>();

        // Wrap the futures in `async move` to ensure the streams are dropped on completion.
        let produce_data = async move { producer(&proxy1, 2).await };
        let consume_data = async move { consumer(&proxy2, 2).await };

        try_join!(
            produce_data,
            collect_data(stream1, &aggregator),
            provide_data(stream2, &aggregator),
            consume_data,
        )?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_coverage_multiple_producers_single_consumer() -> Result<()> {
        let aggregator = Aggregator::new();
        let (proxy1, stream1) = create_proxy_and_stream::<fuzz::CoverageDataCollectorMarker>();
        let (proxy2, stream2) = create_proxy_and_stream::<fuzz::CoverageDataCollectorMarker>();
        let (proxy3, stream3) = create_proxy_and_stream::<fuzz::CoverageDataProviderMarker>();

        // Wrap the futures in `async move` to ensure the streams are dropped on completion.
        let produce_data1 = async move { producer(&proxy1, 1).await };
        let produce_data2 = async move { producer(&proxy2, 3).await };
        let consume_data3 = async move { consumer(&proxy3, 4).await };

        try_join!(
            produce_data1,
            produce_data2,
            collect_data(stream1, &aggregator),
            collect_data(stream2, &aggregator),
            provide_data(stream3, &aggregator),
            consume_data3,
        )?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_coverage_multiple_producers_serial_consumers() -> Result<()> {
        let aggregator = Aggregator::new();
        let (proxy1, stream1) = create_proxy_and_stream::<fuzz::CoverageDataCollectorMarker>();
        let (proxy2, stream2) = create_proxy_and_stream::<fuzz::CoverageDataProviderMarker>();

        // Wrap the last future in `async move` to ensure its stream is dropped on completion.
        let produce_fut = producer(&proxy1, 2).fuse();
        let collect_fut = collect_data(stream1, &aggregator).fuse();
        let provide_fut = provide_data(stream2, &aggregator).fuse();
        let consume_fut = async move { consumer(&proxy2, 2).await };
        let consume_fut = consume_fut.fuse();
        pin_mut!(produce_fut, collect_fut, provide_fut, consume_fut);

        // Run the `provide_data` and `consume_data` futures to completion.
        let mut completed = 0;
        while completed < 2 {
            select! {
                result = produce_fut => result,
                result = collect_fut => result,
                result = provide_fut => result,
                result = consume_fut => result,
            }?;
            completed += 1;
        }

        // Now reconnect. The consumer should get the same results from the connected producers.
        let (proxy3, stream3) = create_proxy_and_stream::<fuzz::CoverageDataProviderMarker>();
        let provide_fut = provide_data(stream3, &aggregator).fuse();
        let consume_fut = async move { consumer(&proxy3, 2).await };
        let consume_fut = consume_fut.fuse();
        pin_mut!(provide_fut, consume_fut);
        while completed < 4 {
            select! {
                result = produce_fut => result.context("a"),
                result = collect_fut => result.context("b"),
                result = provide_fut => result.context("c"),
                result = consume_fut => result.context("d"),
            }?;
            completed += 1;
        }
        Ok(())
    }
}
