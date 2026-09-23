// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
use crate::TestExecutorBuilder;
use crate::{LocalExecutorBuilder, SendExecutorBuilder, TimeoutExt};
use futures::prelude::*;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_os = "fuchsia")]
use std::task::Poll;
use std::time::Duration;

// Apply the timeout from config to test
// Ideally this would be a function like Config::with_timeout, but we need to handle Send and !Send
// and it's likely better not to have to duplicate this code.
macro_rules! apply_timeout {
    ($config:expr, $test:expr) => {{
        let timeout = $config.timeout;
        let test = $test;
        move |run| {
            let test = test(run);
            async move {
                if let Some(timeout) = timeout {
                    test.on_timeout(timeout, || panic!("timeout on run {}", run)).await
                } else {
                    test.await
                }
            }
        }
    }};
}

/// Defines how to compose multiple test runs for a kind of test result.
pub trait TestResult: Sized {
    /// How to repeatedly run a test with this result in a single threaded executor.
    fn run_singlethreaded(
        test: &(dyn Sync + Fn(usize) -> Pin<Box<dyn Future<Output = Self>>>),
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self;

    /// Similarly, but use run_until_stalled
    #[cfg(target_os = "fuchsia")]
    fn run_until_stalled<
        F: 'static + Sync + Fn(usize) -> Fut,
        Fut: 'static + Future<Output = Self>,
    >(
        fake_time: bool,
        test: F,
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self;

    /// Whether the result is successful.
    fn is_ok(&self) -> bool;
}

/// Defines how to compose multiple test runs for a kind of test result in a multithreaded executor.
pub trait MultithreadedTestResult: Sized {
    /// How to repeatedly run a test with this result in a multi threaded executor.
    fn run<F: 'static + Sync + Fn(usize) -> Fut, Fut: 'static + Send + Future<Output = Self>>(
        test: F,
        threads: u8,
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self;

    /// Whether the result is successful.
    fn is_ok(&self) -> bool;
}

impl<E: Send + 'static + std::fmt::Debug> TestResult for Result<(), E> {
    fn run_singlethreaded(
        test: &(dyn Sync + Fn(usize) -> Pin<Box<dyn Future<Output = Self>>>),
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self {
        cfg.run(1, |run| {
            LocalExecutorBuilder::new()
                .allow_interrupts(options.allow_interrupts)
                .build()
                .run_singlethreaded(test(run))
        })
    }

    #[cfg(target_os = "fuchsia")]
    fn run_until_stalled<
        F: 'static + Sync + Fn(usize) -> Fut,
        Fut: 'static + Future<Output = Self>,
    >(
        fake_time: bool,
        test: F,
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self {
        let test = apply_timeout!(cfg, |run| test(run));
        cfg.run(1, |run| {
            let mut executor = TestExecutorBuilder::new()
                .fake_time(fake_time)
                .allow_interrupts(options.allow_interrupts)
                .build();
            match executor.run_until_stalled(&mut std::pin::pin!(test(run))) {
                Poll::Ready(result) => result,
                Poll::Pending => panic!(
                    "Stalled without completing. Consider using \"run_singlethreaded\", or check \
                     for a deadlock."
                ),
            }
        })
    }

    fn is_ok(&self) -> bool {
        Result::is_ok(self)
    }
}

impl<E: 'static + Send> MultithreadedTestResult for Result<(), E> {
    fn run<F: 'static + Sync + Fn(usize) -> Fut, Fut: 'static + Send + Future<Output = Self>>(
        test: F,
        threads: u8,
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self {
        let test = apply_timeout!(cfg, |run| test(run));
        // Fuchsia's SendExecutor actually uses an extra thread, but it doesn't do anything, so we
        // don't count it.
        cfg.run(threads, |run| {
            SendExecutorBuilder::new()
                .num_threads(threads)
                .allow_interrupts(options.allow_interrupts)
                .build()
                .run(test(run))
        })
    }

    fn is_ok(&self) -> bool {
        Result::is_ok(self)
    }
}

impl TestResult for () {
    fn run_singlethreaded(
        test: &(dyn Sync + Fn(usize) -> Pin<Box<dyn Future<Output = Self>>>),
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self {
        let _ = cfg.run(1, |run| {
            LocalExecutorBuilder::new()
                .allow_interrupts(options.allow_interrupts)
                .build()
                .run_singlethreaded(test(run));
            Ok::<(), ()>(())
        });
    }

    #[cfg(target_os = "fuchsia")]
    fn run_until_stalled<
        F: Sync + 'static + Fn(usize) -> Fut,
        Fut: 'static + Future<Output = Self>,
    >(
        fake_time: bool,
        test: F,
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self {
        let _ = TestResult::run_until_stalled(
            fake_time,
            move |run| {
                let test = test(run);
                async move {
                    test.await;
                    Ok::<(), ()>(())
                }
            },
            options,
            cfg,
        );
    }

    fn is_ok(&self) -> bool {
        true
    }
}

impl MultithreadedTestResult for () {
    fn run<F: 'static + Sync + Fn(usize) -> Fut, Fut: 'static + Send + Future<Output = Self>>(
        test: F,
        threads: u8,
        options: TestOptions,
        cfg: EnvironmentConfig,
    ) -> Self {
        // Fuchsia's SendExecutor actually uses an extra thread, but it doesn't do anything, so we
        // don't count it.
        let _ = cfg.run(threads, |run| {
            SendExecutorBuilder::new()
                .num_threads(threads)
                .allow_interrupts(options.allow_interrupts)
                .build()
                .run(test(run));
            Ok::<(), ()>(())
        });
    }

    fn is_ok(&self) -> bool {
        true
    }
}

/// Environment configurations for a single test run.
#[derive(Clone)]
pub struct EnvironmentConfig {
    repeat_count: usize,
    max_concurrency: usize,
    max_threads: u8,
    timeout: Option<Duration>,
}

fn env_var<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).unwrap_or_default().parse().unwrap_or(default)
}

impl EnvironmentConfig {
    fn get() -> Self {
        let repeat_count = std::cmp::max(1, env_var("FASYNC_TEST_REPEAT_COUNT", 1));
        let max_concurrency = env_var("FASYNC_TEST_MAX_CONCURRENCY", 0);
        let timeout_seconds = env_var("FASYNC_TEST_TIMEOUT_SECONDS", 0);
        let max_threads = env_var("FASYNC_TEST_MAX_THREADS", 0);
        let timeout =
            if timeout_seconds == 0 { None } else { Some(Duration::from_secs(timeout_seconds)) };
        Self { repeat_count, max_concurrency, max_threads, timeout }
    }

    fn in_parallel<E: Send>(
        &self,
        threads: u8,
        f: impl Fn() -> Result<(), E> + Sync,
    ) -> Result<(), E> {
        std::thread::scope(|s| {
            let mut join_handles = Vec::new();
            for _ in 1..threads {
                join_handles.push(s.spawn(&f));
            }
            f()?;
            for h in join_handles {
                if let Ok(result @ Err(_)) = h.join() {
                    return result;
                }
            }
            Ok(())
        })
    }

    fn run<E: Send>(
        &self,
        test_threads: u8,
        f: impl Fn(usize) -> Result<(), E> + Sync,
    ) -> Result<(), E> {
        // max_concurrency is the maximum number of runs of the same test to run in parallel, but
        // each test can run multiple threads.  max_threads is the maximum number of threads.
        let mut threads = std::cmp::min(std::cmp::max(self.repeat_count, 1), self.max_concurrency);
        if self.max_threads != 0 {
            threads =
                std::cmp::min(threads, std::cmp::max(self.max_threads / test_threads, 1) as usize);
        }
        let threads = u8::try_from(threads).unwrap_or(u8::MAX);
        let run = AtomicUsize::new(0);
        self.in_parallel(threads, || {
            loop {
                let this_run = run.fetch_add(1, Ordering::Relaxed);
                if this_run >= self.repeat_count {
                    return Ok(());
                }
                let result = f(this_run);
                if result.is_err() {
                    // Prevent any more runs from starting.
                    run.store(self.repeat_count, Ordering::Relaxed);
                    return result;
                }
            }
        })
    }
}

/// Options for running an async test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestOptions {
    /// True iff the executor should support binding Zircon interrupts.
    pub allow_interrupts: bool,
}

/// Runs a test in an executor, potentially repeatedly and concurrently.
pub fn run_singlethreaded_test<F, Fut, R>(test: F, options: TestOptions) -> R
where
    F: 'static + Sync + Fn(usize) -> Fut,
    Fut: 'static + Future<Output = R>,
    R: TestResult,
{
    TestResult::run_singlethreaded(
        &|run| test(run).boxed_local(),
        options,
        EnvironmentConfig::get(),
    )
}

/// Runs a test in an executor until it's stalled.
#[cfg(target_os = "fuchsia")]
pub fn run_until_stalled_test<F, Fut, R>(fake_time: bool, test: F, options: TestOptions) -> R
where
    F: 'static + Sync + Fn(usize) -> Fut,
    Fut: 'static + Future<Output = R>,
    R: TestResult,
{
    TestResult::run_until_stalled(fake_time, test, options, EnvironmentConfig::get())
}

/// Runs a test in an executor, potentially repeatedly and concurrently.
pub fn run_test<F, Fut, R>(test: F, threads: u8, options: TestOptions) -> R
where
    F: 'static + Sync + Fn(usize) -> Fut,
    Fut: 'static + Send + Future<Output = R>,
    R: MultithreadedTestResult,
{
    MultithreadedTestResult::run(test, threads, options, EnvironmentConfig::get())
}

#[cfg(test)]
mod tests {
    use super::{EnvironmentConfig, MultithreadedTestResult, TestOptions, TestResult};
    use futures::lock::Mutex;
    use futures::prelude::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn run_singlethreaded() {
        const REPEAT_COUNT: usize = 1000;
        const MAX_THREADS: u8 = 10;
        let pending_runs: Arc<Mutex<HashSet<_>>> =
            Arc::new(Mutex::new((0..REPEAT_COUNT).collect()));
        let pending_runs_child = pending_runs.clone();
        TestResult::run_singlethreaded(
            &move |i| {
                let pending_runs_child = pending_runs_child.clone();
                async move {
                    assert!(pending_runs_child.lock().await.remove(&i));
                }
                .boxed_local()
            },
            TestOptions::default(),
            EnvironmentConfig {
                repeat_count: REPEAT_COUNT,
                max_concurrency: 0,
                max_threads: MAX_THREADS,
                timeout: None,
            },
        );
        assert!(pending_runs.try_lock().unwrap().is_empty());
    }

    // TODO(https://fxbug.dev/42138715): should_panic tests trigger LSAN
    #[ignore]
    #[test]
    #[should_panic]
    fn run_singlethreaded_with_timeout() {
        TestResult::run_singlethreaded(
            &move |_| {
                async move {
                    futures::future::pending::<()>().await;
                }
                .boxed_local()
            },
            TestOptions::default(),
            EnvironmentConfig {
                repeat_count: 1,
                max_concurrency: 0,
                max_threads: 0,
                timeout: Some(Duration::from_millis(1)),
            },
        );
    }

    #[test]
    #[cfg(target_os = "fuchsia")]
    fn run_until_stalled() {
        const REPEAT_COUNT: usize = 1000;
        let pending_runs: Arc<Mutex<HashSet<_>>> =
            Arc::new(Mutex::new((0..REPEAT_COUNT).collect()));
        let pending_runs_child = pending_runs.clone();
        TestResult::run_until_stalled(
            false,
            move |i| {
                let pending_runs_child = pending_runs_child.clone();
                async move {
                    assert!(pending_runs_child.lock().await.remove(&i));
                }
            },
            TestOptions::default(),
            EnvironmentConfig {
                repeat_count: REPEAT_COUNT,
                max_concurrency: 1,
                max_threads: 1,
                timeout: None,
            },
        );
        assert!(pending_runs.try_lock().unwrap().is_empty());
    }

    #[test]
    fn run() {
        const REPEAT_COUNT: usize = 1000;
        const THREADS: u8 = 4;
        let pending_runs: Arc<Mutex<HashSet<_>>> =
            Arc::new(Mutex::new((0..REPEAT_COUNT).collect()));
        let pending_runs_child = pending_runs.clone();
        MultithreadedTestResult::run(
            move |i| {
                let pending_runs_child = pending_runs_child.clone();
                async move {
                    assert!(pending_runs_child.lock().await.remove(&i));
                }
            },
            THREADS,
            TestOptions::default(),
            EnvironmentConfig {
                repeat_count: REPEAT_COUNT,
                max_concurrency: 0,
                max_threads: THREADS,
                timeout: None,
            },
        );
        assert!(pending_runs.try_lock().unwrap().is_empty());
    }

    // TODO(https://fxbug.dev/42138715): should_panic tests trigger LSAN
    #[ignore]
    #[test]
    #[should_panic]
    fn run_with_timeout() {
        const THREADS: u8 = 4;
        MultithreadedTestResult::run(
            move |_| async move {
                futures::future::pending::<()>().await;
            },
            THREADS,
            TestOptions::default(),
            EnvironmentConfig {
                repeat_count: 1,
                max_concurrency: 0,
                max_threads: 0,
                timeout: Some(Duration::from_millis(1)),
            },
        );
    }

    #[test]
    fn run_singlethreaded_with_allow_interrupts() {
        TestResult::run_singlethreaded(
            &move |_| async move {}.boxed_local(),
            TestOptions { allow_interrupts: true },
            EnvironmentConfig {
                repeat_count: 1,
                max_concurrency: 0,
                max_threads: 1,
                timeout: None,
            },
        );
    }

    #[test]
    #[cfg(target_os = "fuchsia")]
    fn run_until_stalled_with_allow_interrupts() {
        TestResult::run_until_stalled(
            false,
            move |_| async move {},
            TestOptions { allow_interrupts: true },
            EnvironmentConfig {
                repeat_count: 1,
                max_concurrency: 1,
                max_threads: 1,
                timeout: None,
            },
        );
    }

    #[test]
    fn run_with_allow_interrupts() {
        const THREADS: u8 = 2;
        MultithreadedTestResult::run(
            move |_| async move {},
            THREADS,
            TestOptions { allow_interrupts: true },
            EnvironmentConfig {
                repeat_count: 1,
                max_concurrency: 0,
                max_threads: THREADS,
                timeout: None,
            },
        );
    }
}
