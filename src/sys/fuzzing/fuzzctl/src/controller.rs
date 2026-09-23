// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::constants::*;
use crate::corpus;
use crate::diagnostics::Forwarder;
use crate::duration::deadline_after;
use crate::input::{Input, InputPair};
use crate::writer::{OutputSink, Writer};
use anyhow::{Context as _, Error, Result, anyhow, bail};
use flex_client::{self, ProxyHasDomain};
use flex_fuchsia_fuzzer::{self as fuzz, Artifact as FidlArtifact};
use fuchsia_async::Timer;
use futures::future::{Either, pending};
use futures::{Future, FutureExt, pin_mut, select, try_join};
use std::cell::RefCell;
use std::cmp::max;
use std::path::Path;
use zx_status as zx;

/// Represents a `fuchsia.fuzzer.Controller` connection to a fuzzer.
#[derive(Debug)]
pub struct Controller<O: OutputSink> {
    proxy: fuzz::ControllerProxy,
    forwarder: Forwarder<O>,
    min_timeout: i64,
    timeout: RefCell<Option<i64>>,
}

impl<O: OutputSink> Controller<O> {
    /// Returns a new Controller instance.
    pub fn new(proxy: fuzz::ControllerProxy, writer: &Writer<O>) -> Self {
        Self {
            proxy,
            forwarder: Forwarder::<O>::new(writer),
            min_timeout: 60 * NANOS_PER_SECOND,
            timeout: RefCell::new(None),
        }
    }

    pub fn domain(&self) -> flex_client::ClientArg {
        self.proxy.domain()
    }

    /// Registers the provided output socket with the forwarder.
    pub fn set_output<P: AsRef<Path>>(
        &mut self,
        socket: flex_client::Socket,
        output: fuzz::TestOutput,
        logs_dir: &Option<P>,
    ) -> Result<()> {
        self.forwarder.set_output(socket, output, logs_dir)
    }

    /// Sets the minimum amount of time, in nanoseconds, before a workflow can time out.
    ///
    /// If a the `max_total_time` option is set for a workflow that hangs, it will eventually
    /// timeout. This method can be used to specify the minimum duration that must elapse before a
    /// workflow is considered hung. The default of 1 minute is usually appropriate, but this method
    /// can be useful when testing.
    ///
    pub fn set_min_timeout(&mut self, min_timeout: i64) {
        self.min_timeout = min_timeout;
    }

    /// Sets various execution and error detection parameters for the fuzzer.
    ///
    /// Returns an error if:
    ///   * Communicating with the fuzzer fails
    ///   * A long-running call such as `try_one`, `fuzz`, `cleanse`, `minimize`, or `merge` is in
    ///     progress.
    ///
    pub async fn configure(&self, options: fuzz::Options) -> Result<()> {
        self.set_timeout(&options, None);
        let result =
            self.proxy.configure(&options).await.context("fuchsia.fuzzer/Controller.Configure")?;
        result.map_err(|status| {
            anyhow!("fuchsia.fuzzer/Controller.Configure returned: ZX_ERR_{}", status)
        })
    }

    /// Returns a fuzzer's current values for the various execution and error detection parameters.
    ///
    /// Returns an error if communicating with the fuzzer fails
    ///
    pub async fn get_options(&self) -> Result<fuzz::Options> {
        self.proxy
            .get_options()
            .await
            .map_err(Error::msg)
            .context("`fuchsia.fuzzer.Controller/GetOptions` failed")
    }

    /// Recalculates the timeout for a long-running workflow based on the configured maximum total
    /// time and reported elapsed time.
    ///
    /// Returns an error if communicating with the fuzzer fails
    ///
    pub async fn reset_timer(&self) -> Result<()> {
        let options = self.get_options().await?;
        let status = self.get_status().await?;
        self.set_timeout(&options, Some(status));
        Ok(())
    }

    // Sets a workflow timeout based on the maximum total time a fuzzer workflow is expected to run.
    fn set_timeout(&self, options: &fuzz::Options, status: Option<fuzz::Status>) {
        let elapsed = status.map(|s| s.elapsed.unwrap_or(0)).unwrap_or(0);
        if let Some(max_total_time) = options.max_total_time {
            let mut timeout_mut = self.timeout.borrow_mut();
            match max_total_time {
                0 => {
                    *timeout_mut = None;
                }
                n => {
                    *timeout_mut = Some(max(n * 2, self.min_timeout) - elapsed);
                }
            }
        }
    }

    /// Retrieves test inputs from one of the fuzzer's corpora.
    ///
    /// The compacted corpus is saved to the `corpus_dir`. Returns details on how much data was
    /// received.
    ///
    /// Returns an error if:
    ///   * Communicating with the fuzzer fails.
    ///   * One or more inputs fails to be received and saved.
    ///
    pub async fn read_corpus<P: AsRef<Path>>(
        &self,
        corpus_type: fuzz::Corpus,
        corpus_dir: P,
    ) -> Result<corpus::Stats> {
        let (client_end, server_end) =
            self.proxy.domain().create_endpoints::<fuzz::CorpusReaderMarker>();
        let stream = server_end.into_stream();
        let (_, corpus_stats) = try_join!(
            async { self.proxy.read_corpus(corpus_type, client_end).await.map_err(Error::msg) },
            async { corpus::read(stream, corpus_dir).await },
        )
        .context("`fuchsia.fuzzer.Controller/ReadCorpus` failed")?;
        Ok(corpus_stats)
    }

    /// Adds a test input to one of the fuzzer's corpora.
    ///
    /// The `test_input` may be either a single file or a directory. Returns details on how much
    /// data was sent.
    ///
    /// Returns an error if:
    ///   * Converting the input to an `Input`/`fuchsia.fuzzer.Input` pair fails.
    ///   * Communicating with the fuzzer fails
    ///   * The fuzzer returns an error, e.g. if it failed to transfer the input.
    ///
    pub async fn add_to_corpus(
        &self,
        input_pairs: Vec<InputPair>,
        corpus_type: fuzz::Corpus,
    ) -> Result<corpus::Stats> {
        let expected_num_inputs = input_pairs.len();
        let expected_total_size =
            input_pairs.iter().fold(0, |total, input_pair| total + input_pair.len());
        let mut corpus_stats = corpus::Stats { num_inputs: 0, total_size: 0 };
        for input_pair in input_pairs.into_iter() {
            let (fidl_input, input) = input_pair.as_tuple();
            let fidl_input_size = fidl_input.size;
            let (result, _) = try_join!(
                async {
                    self.proxy.add_to_corpus(corpus_type, fidl_input).await.map_err(Error::msg)
                },
                input.send(),
            )
            .context("fuchsia.fuzzer/Controller.AddToCorpus failed")?;
            if let Err(status) = result {
                bail!(
                    "fuchsia.fuzzer/Controller.AddToCorpus returned: ZX_ERR_{} \
                       after writing {} of {} files ({} of {} bytes)",
                    status,
                    corpus_stats.num_inputs,
                    expected_num_inputs,
                    corpus_stats.total_size,
                    expected_total_size
                )
            }
            corpus_stats.num_inputs += 1;
            corpus_stats.total_size += fidl_input_size;
        }
        Ok(corpus_stats)
    }

    /// Returns information about fuzzer execution.
    ///
    /// The status typically includes information such as how long the fuzzer has been running, how
    /// many edges in the call graph have been covered, how large the corpus is, etc.
    ///
    /// Refer to `fuchsia.fuzzer.Status` for precise details on the returned information.
    ///
    pub async fn get_status(&self) -> Result<fuzz::Status> {
        match self.proxy.get_status().await {
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx::Status::PEER_CLOSED =>
            {
                return Ok(fuzz::Status::default());
            }
            Err(e) => bail!("`fuchsia.fuzzer.Controller/GetStatus` failed: {:?}", e),
            Ok(fuzz_status) => Ok(fuzz_status),
        }
    }

    /// Runs the fuzzer in a loop to generate and test new inputs.
    ///
    /// The fuzzer will continuously generate new inputs and tries them until one of four
    /// conditions are met:
    ///   * The number of inputs tested exceeds the configured number of `runs`.
    ///   * The configured amount of `max_total_time` has elapsed.
    ///   * An input triggers a fatal error, e.g. death by AddressSanitizer.
    ///   * `fuchsia.fuzzer.Controller/Stop` is called.
    ///
    /// Returns an error if:
    ///   * Either `runs` or `time` is provided but cannot be parsed to  a valid value.
    ///   * Communicating with the fuzzer fails.
    ///   * The fuzzer returns an error, e.g. it is already performing another workflow.
    ///
    pub async fn fuzz(&self) -> Result<()> {
        let response = self.proxy.fuzz().await;
        let status = check_response("Fuzz", response)?;
        check_status("Fuzz", status)
    }

    /// Tries running the fuzzer once using the given input.
    ///
    /// Returns an error if:
    ///   * Converting the input to an `Input`/`fuchsia.fuzzer.Input` pair fails.
    ///   * Communicating with the fuzzer fails.
    ///   * The fuzzer returns an error, e.g. it is already performing another workflow.
    ///
    pub async fn try_one(&self, input_pair: InputPair) -> Result<()> {
        let (fidl_input, input) = input_pair.as_tuple();
        let status = self.with_input("TryOne", self.proxy.try_one(fidl_input), input).await?;
        check_status("TryOne", status)
    }

    /// Reduces the length of an error-causing input while preserving the error.
    ///
    /// The fuzzer will bound its attempt to find shorter inputs using the given `runs` or `time`,
    /// if provided.
    ///
    /// Returns an error if:
    ///   * Either `runs` or `time` is provided but cannot be parsed to  a valid value.
    ///   * Converting the input to an `Input`/`fuchsia.fuzzer.Input` pair fails.
    ///   * Communicating with the fuzzer fails.
    ///   * The fuzzer returns an error, e.g. it is already performing another workflow.
    ///   * The minimized input fails to be received and saved.
    ///
    pub async fn minimize(&self, input_pair: InputPair) -> Result<()> {
        let (fidl_input, input) = input_pair.as_tuple();
        let status = self.with_input("Minimize", self.proxy.minimize(fidl_input), input).await?;
        match status {
            Err(zx::Status::INVALID_ARGS) => bail!("the provided input did not cause an error"),
            status => check_status("Minimize", status),
        }
    }

    /// Replaces bytes in a error-causing input with PII-safe bytes, e.g. spaces.
    ///
    /// The fuzzer will try to reproduce the error caused by the input with each byte replaced by a
    /// fixed number of "clean" candidates.
    ///
    /// Returns an error if:
    ///   * Converting the input to an `Input`/`fuchsia.fuzzer.Input` pair fails.
    ///   * Communicating with the fuzzer fails.
    ///   * The fuzzer returns an error, e.g. it is already performing another workflow.
    ///   * The cleansed input fails to be received and saved.
    ///
    pub async fn cleanse(&self, input_pair: InputPair) -> Result<()> {
        let (fidl_input, input) = input_pair.as_tuple();
        let status = self.with_input("Cleanse", self.proxy.cleanse(fidl_input), input).await?;
        match status {
            Err(zx::Status::INVALID_ARGS) => bail!("the provided input did not cause an error"),
            status => check_status("Cleanse", status),
        }
    }

    /// Removes inputs from the corpus that produce duplicate coverage.
    ///
    /// The fuzzer makes a finite number of passes over its seed and live corpora. The seed corpus
    /// is unchanged, but the fuzzer will try to find the set of shortest inputs that preserves
    /// coverage.
    ///
    /// Returns an error if:
    ///   * Communicating with the fuzzer fails.
    ///   * The fuzzer returns an error, e.g. it is already performing another workflow.
    ///   * One or more inputs fails to be received and saved.
    ///
    pub async fn merge(&self) -> Result<()> {
        let response = self.proxy.merge().await;
        let status = check_response("Merge", response)?;
        match status {
            Err(zx::Status::INVALID_ARGS) => {
                bail!("an input in the seed corpus triggered an error")
            }
            status => check_status("Merge", status),
        }
    }

    // Runs the given `fidl_fut` along with a future to send an `input`.
    async fn with_input<F>(
        &self,
        name: &str,
        fidl_fut: F,
        input: Input,
    ) -> Result<Result<(), zx::Status>>
    where
        F: Future<Output = Result<Result<(), i32>, fidl::Error>>,
    {
        let fidl_fut = fidl_fut.fuse();
        let send_fut = input.send().fuse();
        let timer_fut = match deadline_after(*self.timeout.borrow()) {
            Some(deadline) => Either::Left(Timer::new(deadline)),
            None => Either::Right(pending()),
        };
        let timer_fut = timer_fut.fuse();
        pin_mut!(fidl_fut, send_fut, timer_fut);
        let mut remaining = 2;
        let mut status = Ok(());
        // If `fidl_fut` completes with e.g. `Ok(zx::Status::CANCELED)`, drop
        // the `send_fut` and `forward_fut` futures.
        while remaining > 0 && status.is_ok() {
            select! {
                response = fidl_fut => {
                    status = check_response(name, response)?;
                    remaining -= 1;
                }
                result = send_fut => {
                    result?;
                    remaining -= 1;
                }
                _ = timer_fut => {
                    bail!("workflow timed out");
                }
            };
        }
        Ok(status)
    }
    /// Waits for the results of a long-running workflow.
    ///
    /// The `fuchsia.fuzzer.Controller/WatchArtifact` method uses a
    /// ["hanging get" pattern](https://fuchsia.dev/fuchsia-src/development/api/fidl#hanging-get).
    /// The first call will return whatever the current artifact is for the fuzzer; subsequent calls
    /// will block until the artifact changes. The implementation below may retry the FIDL method to
    /// ensure it only returns `Ok(None)` on channel close.
    ///
    pub async fn watch_artifact(&self) -> Result<FidlArtifact> {
        let watch_fut = || async move {
            loop {
                let artifact = self.proxy.watch_artifact().await?;
                if artifact != FidlArtifact::default() {
                    return Ok(artifact);
                }
            }
        };
        let watch_fut = watch_fut().fuse();
        let forward_fut = self.forwarder.forward_all().fuse();
        let timer_fut = match deadline_after(*self.timeout.borrow()) {
            Some(deadline) => Either::Left(Timer::new(deadline)),
            None => Either::Right(pending()),
        };
        let timer_fut = timer_fut.fuse();
        pin_mut!(watch_fut, forward_fut, timer_fut);
        let mut remaining = 2;
        let mut fidl_artifact = FidlArtifact::default();
        // If `fidl_fut` completes with e.g. `Ok(zx::Status::CANCELED)`, drop
        // the `send_fut` and `forward_fut` futures.
        while remaining > 0 {
            select! {
                result = watch_fut => {
                    fidl_artifact = match result {
                        Ok(fidl_artifact) => {
                            if let Some(e) = fidl_artifact.error {
                                bail!("workflow returned an error: ZX_ERR_{}", e);
                            }
                            fidl_artifact
                        }
                        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                            if epitaph == zx::Status::PEER_CLOSED =>
                        {
                            FidlArtifact {
                                error: Some(zx::Status::CANCELED.into_raw()),
                                ..Default::default()
                            }
                        }
                        Err(e) => bail!("fuchsia.fuzzer/Controller.WatchArtifact: {:?}", e),
                    };
                    remaining -= 1;
                }
                result = forward_fut => {
                    result?;
                    remaining -= 1;
                }
                _ = timer_fut => {
                    bail!("workflow timed out");
                }
            };
        }
        Ok(fidl_artifact)
    }
}

// Checks a FIDL response for generic errors.
fn check_response(
    name: &str,
    response: Result<Result<(), i32>, fidl::Error>,
) -> Result<Result<(), zx::Status>> {
    match response {
        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx::Status::PEER_CLOSED =>
        {
            Ok(Ok(()))
        }
        Err(e) => bail!("`fuchsia.fuzzer.Controller/{}` failed: {:?}", name, e),
        Ok(Err(raw)) => Ok(Err(zx::Status::err_from_raw(raw))),
        Ok(Ok(())) => Ok(Ok(())),
    }
}

// Checks the result from a FIDL response for common errors.
fn check_status(name: &str, status: Result<(), zx::Status>) -> Result<()> {
    match status {
        Ok(()) => Ok(()),
        Err(zx::Status::BAD_STATE) => bail!("another long-running workflow is in progress"),
        Err(status) => bail!("`fuchsia.fuzzer.Controller/{}` returned: ZX_ERR_{}", name, status),
    }
}

#[cfg(test)]
mod tests {
    use crate::util::digest_path;
    use anyhow::{Context as _, Result};
    use flex_fuchsia_fuzzer::{self as fuzz, Result_ as FuzzResult};
    use fuchsia_async as fasync;
    use fuchsia_fuzzctl::{Controller, Input, InputPair};
    use fuchsia_fuzzctl_test::{FakeController, Test, create_task, serve_controller, verify_saved};
    use zx_status as zx;

    // Creates a test setup suitable for unit testing `Controller`.
    fn perform_test_setup(
        test: &Test,
    ) -> Result<(FakeController, fuzz::ControllerProxy, fasync::Task<()>)> {
        let fake = test.controller();
        let (proxy, stream) = test.domain().create_proxy_and_stream::<fuzz::ControllerMarker>();
        let task = create_task(serve_controller(stream, test.clone()), test.writer());
        Ok((fake, proxy, task))
    }

    #[fuchsia::test]
    async fn test_configure() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        // Modify all the options that start with 'd'.
        let expected = fuzz::Options {
            dictionary_level: Some(1),
            detect_exits: Some(true),
            detect_leaks: Some(false),
            death_exitcode: Some(2),
            debug: Some(true),
            ..Default::default()
        };
        controller.configure(expected.clone()).await?;
        let actual = fake.get_options();
        assert_eq!(actual.dictionary_level, expected.dictionary_level);
        assert_eq!(actual.detect_exits, expected.detect_exits);
        assert_eq!(actual.detect_leaks, expected.detect_leaks);
        assert_eq!(actual.death_exitcode, expected.death_exitcode);
        assert_eq!(actual.debug, expected.debug);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_options() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        // Modify all the options that start with 'm'.
        let expected = fuzz::Options {
            max_total_time: Some(20000),
            max_input_size: Some(2000),
            mutation_depth: Some(20),
            malloc_limit: Some(200),
            malloc_exitcode: Some(2),
            ..Default::default()
        };
        fake.set_options(expected.clone());
        let actual = controller.get_options().await?;
        assert_eq!(actual.max_total_time, expected.max_total_time);
        assert_eq!(actual.max_input_size, expected.max_input_size);
        assert_eq!(actual.mutation_depth, expected.mutation_depth);
        assert_eq!(actual.malloc_limit, expected.malloc_limit);
        assert_eq!(actual.malloc_exitcode, expected.malloc_exitcode);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_read_corpus() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        let seed_dir = test.create_dir("seed")?;
        fake.set_input_to_send(b"foo");
        let stats = controller.read_corpus(fuzz::Corpus::Seed, &seed_dir).await?;
        assert_eq!(fake.get_corpus_type(), fuzz::Corpus::Seed);
        assert_eq!(stats.num_inputs, 1);
        assert_eq!(stats.total_size, 3);
        let path = digest_path(&seed_dir, None, b"foo");
        verify_saved(&path, b"foo")?;

        let live_dir = test.create_dir("live")?;
        fake.set_input_to_send(b"barbaz");
        let stats = controller.read_corpus(fuzz::Corpus::Live, &live_dir).await?;
        assert_eq!(fake.get_corpus_type(), fuzz::Corpus::Live);
        assert_eq!(stats.num_inputs, 1);
        assert_eq!(stats.total_size, 6);
        let path = digest_path(&live_dir, None, b"barbaz");
        verify_saved(&path, b"barbaz")?;

        Ok(())
    }

    #[fuchsia::test]
    async fn test_add_to_corpus() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        let input_pairs: Vec<InputPair> = vec![b"foo".to_vec(), b"bar".to_vec(), b"baz".to_vec()]
            .into_iter()
            .map(|data| InputPair::try_from_data(&test.domain(), data).unwrap())
            .collect();
        let stats = controller.add_to_corpus(input_pairs, fuzz::Corpus::Seed).await?;
        assert_eq!(fake.get_corpus_type(), fuzz::Corpus::Seed);
        assert_eq!(stats.num_inputs, 3);
        assert_eq!(stats.total_size, 9);

        let input_pairs: Vec<InputPair> =
            vec![b"qux".to_vec(), b"quux".to_vec(), b"corge".to_vec()]
                .into_iter()
                .map(|data| InputPair::try_from_data(&test.domain(), data).unwrap())
                .collect();
        let stats = controller.add_to_corpus(input_pairs, fuzz::Corpus::Live).await?;
        assert_eq!(fake.get_corpus_type(), fuzz::Corpus::Live);
        assert_eq!(stats.num_inputs, 3);
        assert_eq!(stats.total_size, 12);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_status() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        let expected = fuzz::Status {
            running: Some(true),
            runs: Some(1),
            elapsed: Some(2),
            covered_pcs: Some(3),
            covered_features: Some(4),
            corpus_num_inputs: Some(5),
            corpus_total_size: Some(6),
            process_stats: None,
            ..Default::default()
        };
        fake.set_status(expected.clone());
        let actual = controller.get_status().await?;
        assert_eq!(actual, expected);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_try_one() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        let input_pair = InputPair::try_from_data(&test.domain(), b"foo".to_vec())?;
        controller.try_one(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::NoErrors));

        fake.set_result(Ok(FuzzResult::Crash));
        let input_pair = InputPair::try_from_data(&test.domain(), b"bar".to_vec())?;
        controller.try_one(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::Crash));

        fake.cancel();
        let input_pair = InputPair::try_from_data(&test.domain(), b"baz".to_vec())?;
        controller.try_one(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, Some(zx::Status::CANCELED.into_raw()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_fuzz() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        let options = fuzz::Options { runs: Some(10), ..Default::default() };
        controller.configure(options).await?;
        controller.fuzz().await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::NoErrors));

        fake.set_result(Ok(FuzzResult::Death));
        fake.set_input_to_send(b"foo");
        controller.fuzz().await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::Death));

        let fidl_input = artifact.input.context("invalid FIDL artifact")?;
        let input = Input::try_receive(fidl_input).await?;
        assert_eq!(input.data, b"foo");

        fake.cancel();
        controller.fuzz().await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, Some(zx::Status::CANCELED.into_raw()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_minimize() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        fake.set_input_to_send(b"foo");
        let input_pair = InputPair::try_from_data(&test.domain(), b"foofoofoo".to_vec())?;
        controller.minimize(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::Minimized));

        let fidl_input = artifact.input.context("invalid FIDL artifact")?;
        let input = Input::try_receive(fidl_input).await?;
        assert_eq!(input.data, b"foo");

        fake.cancel();
        let input_pair = InputPair::try_from_data(&test.domain(), b"bar".to_vec())?;
        controller.minimize(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, Some(zx::Status::CANCELED.into_raw()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_cleanse() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        fake.set_input_to_send(b"   bar   ");
        let input_pair = InputPair::try_from_data(&test.domain(), b"foobarbaz".to_vec())?;
        controller.cleanse(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::Cleansed));

        let fidl_input = artifact.input.context("invalid FIDL artifact")?;
        let input = Input::try_receive(fidl_input).await?;
        assert_eq!(input.data, b"   bar   ");

        fake.cancel();
        let input_pair = InputPair::try_from_data(&test.domain(), b"foobarbaz".to_vec())?;
        controller.cleanse(input_pair).await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, Some(zx::Status::CANCELED.into_raw()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge() -> Result<()> {
        let test = Test::try_new()?;
        let (fake, proxy, _task) = perform_test_setup(&test)?;
        let controller = Controller::new(proxy, test.writer());

        controller.merge().await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, None);
        assert_eq!(artifact.result, Some(FuzzResult::Merged));

        fake.cancel();
        controller.merge().await?;
        let artifact = controller.watch_artifact().await?;
        assert_eq!(artifact.error, Some(zx::Status::CANCELED.into_raw()));

        Ok(())
    }
}
