// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Macros for creating Fuchsia components and tests.
//!
//! These macros work on Fuchsia, and also on host with some limitations (that are called out
//! where they exist).

// Features from those macros are expected to be implemented by exactly one function in this
// module. We strive for simple, independent, single purpose functions as building blocks to allow
// dead code elimination to have the very best chance of removing unused code that might be
// otherwise pulled in here.

#![deny(missing_docs)]

pub use fuchsia_macro::{main, test};
use libc as _;
#[doc(hidden)]
pub use log::error;
use std::future::Future;

#[cfg(fuchsia_api_level_less_than = "27")]
pub use fidl_fuchsia_diagnostics::{Interest, Severity};
#[cfg(fuchsia_api_level_at_least = "27")]
pub use fidl_fuchsia_diagnostics_types::{Interest, Severity};

#[cfg(target_os = "fuchsia")]
pub use fuchsia_async_inspect;

//
// LOGGING INITIALIZATION
//

/// Options used when initializing logging.
#[derive(Default, Clone)]
pub struct LoggingOptions<'a> {
    /// Tags with which to initialize the logging system. All logs will carry the tags configured
    /// here.
    pub tags: &'a [&'static str],

    /// Allows to configure the minimum severity of the logs being emitted. Logs of lower severity
    /// won't be emitted.
    pub interest: Interest,

    /// String to include in logged panic messages.
    pub panic_prefix: &'static str,

    /// True to always log file/line information, false to only log
    /// when severity is ERROR or above.
    pub always_log_file_line: bool,
}

#[cfg(not(target_os = "fuchsia"))]
impl<'a> From<LoggingOptions<'a>> for diagnostics_log::PublishOptions<'a> {
    fn from(logging: LoggingOptions<'a>) -> Self {
        let mut options = diagnostics_log::PublishOptions::default().tags(logging.tags);
        if let Some(severity) = logging.interest.min_severity {
            options = options.minimum_severity(severity);
        }
        options = options.panic_prefix(logging.panic_prefix);
        options
    }
}

#[cfg(target_os = "fuchsia")]
impl<'a> From<LoggingOptions<'a>> for diagnostics_log::PublishOptions<'a> {
    fn from(logging: LoggingOptions<'a>) -> Self {
        let mut options = diagnostics_log::PublishOptions::default().tags(logging.tags);
        if let Some(severity) = logging.interest.min_severity {
            options = options.minimum_severity(severity);
        }
        if logging.always_log_file_line {
            options = options.always_log_file_line();
        }
        options = options.panic_prefix(logging.panic_prefix);
        options
    }
}

/// Initialize logging
#[doc(hidden)]
pub fn init_logging_for_component_with_executor<'a, R>(
    func: impl FnOnce() -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl FnOnce() -> R + 'a {
    move || {
        diagnostics_log::initialize(logging.into()).expect("initialize_logging");
        func()
    }
}

#[doc(hidden)]
pub fn init_default_logging_for_component_with_executor<'a, R>(
    func: impl FnOnce() -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl FnOnce() -> R + 'a {
    init_logging_for_component_with_executor(func, logging)
}

#[doc(hidden)]
pub fn init_noop_logging_for_component_with_executor<'a, R>(
    func: impl FnOnce() -> R + 'a,
    _logging: LoggingOptions<'a>,
) -> impl FnOnce() -> R + 'a {
    func
}

/// Initialize logging
#[doc(hidden)]
pub fn init_logging_for_component_with_threads<'a, R>(
    func: impl FnOnce() -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl FnOnce() -> R + 'a {
    move || {
        let _guard = init_logging_with_threads(logging);
        func()
    }
}

#[doc(hidden)]
pub fn init_default_logging_for_component_with_threads<'a, R>(
    func: impl FnOnce() -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl FnOnce() -> R + 'a {
    init_logging_for_component_with_threads(func, logging)
}

#[doc(hidden)]
pub fn init_noop_logging_for_component_with_threads<'a, R>(
    func: impl FnOnce() -> R + 'a,
    _logging: LoggingOptions<'a>,
) -> impl FnOnce() -> R + 'a {
    func
}

/// Initialize logging
#[doc(hidden)]
pub fn init_logging_for_test_with_executor<'a, R>(
    func: impl Fn(usize) -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl Fn(usize) -> R + 'a {
    move |n| {
        diagnostics_log::initialize(logging.clone().into()).expect("initalize logging");
        func(n)
    }
}

#[doc(hidden)]
pub fn init_default_logging_for_test_with_executor<'a, R>(
    func: impl Fn(usize) -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl Fn(usize) -> R + 'a {
    #[cfg(target_os = "fuchsia")]
    {
        init_logging_for_test_with_executor(func, logging)
    }
    #[cfg(not(target_os = "fuchsia"))]
    {
        init_noop_logging_for_test_with_executor(func, logging)
    }
}

#[doc(hidden)]
pub fn init_noop_logging_for_test_with_executor<'a, R>(
    func: impl Fn(usize) -> R + 'a,
    _logging: LoggingOptions<'a>,
) -> impl Fn(usize) -> R + 'a {
    func
}

/// Initialize logging
#[doc(hidden)]
pub fn init_logging_for_test_with_threads<'a, R>(
    func: impl Fn(usize) -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl Fn(usize) -> R + 'a {
    move |n| {
        init_logging_with_threads(logging.clone());
        func(n)
    }
}

#[doc(hidden)]
pub fn init_default_logging_for_test_with_threads<'a, R>(
    func: impl Fn(usize) -> R + 'a,
    logging: LoggingOptions<'a>,
) -> impl Fn(usize) -> R + 'a {
    #[cfg(target_os = "fuchsia")]
    {
        init_logging_for_test_with_threads(func, logging)
    }
    #[cfg(not(target_os = "fuchsia"))]
    {
        // On non-Fuchsia targets, we don't initialize logging by default for tests. Host tests
        // don't have any process isolation between each other, so one test can say logging=true
        // and another in the same binary can say logging=false, and who wins will be determined
        // by execution order. In order to avoid confusion, we default to not enabling logging. If
        // a user understands the implications, they can still opt back in to enabling it.
        init_noop_logging_for_test_with_threads(func, logging)
    }
}

#[doc(hidden)]
pub fn init_noop_logging_for_test_with_threads<'a, R>(
    func: impl Fn(usize) -> R + 'a,
    _logging: LoggingOptions<'a>,
) -> impl Fn(usize) -> R + 'a {
    func
}

/// Initializes logging on a background thread, returning a guard which cancels interest listening
/// when dropped.
#[cfg(target_os = "fuchsia")]
fn init_logging_with_threads(logging: LoggingOptions<'_>) {
    diagnostics_log::initialize_sync(logging.into());
}

#[cfg(not(target_os = "fuchsia"))]
fn init_logging_with_threads(logging: LoggingOptions<'_>) {
    diagnostics_log::initialize(logging.into()).expect("initialize_logging");
}

#[cfg(target_os = "fuchsia")]
fn set_thread_role(role_name: &str) {
    if let Err(e) = fuchsia_scheduler::set_role_for_this_thread(role_name) {
        log::warn!(e:%, role_name:%; "Couldn't apply thread role.");
    }
}

//
// MAIN FUNCTION WRAPPERS
//

/// Run a non-async main function.
#[doc(hidden)]
pub fn main_not_async<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}

/// Run a non-async main function, applying `role_name` as the thread role.
#[doc(hidden)]
pub fn main_not_async_with_role<F, R>(f: F, _role_name: &'static str) -> R
where
    F: FnOnce() -> R,
{
    #[cfg(target_os = "fuchsia")]
    set_thread_role(_role_name);
    f()
}

/// Run an async main function with a single threaded executor.
#[doc(hidden)]
#[cfg(target_os = "fuchsia")]
pub fn main_singlethreaded<F, Fut, R>(
    f: F,
    instrument: Option<std::sync::Arc<dyn fuchsia_async::instrument::TaskInstrument>>,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + 'static,
{
    fuchsia_async::LocalExecutorBuilder::new()
        .instrument(instrument)
        .build()
        .run_singlethreaded(f())
}

/// Run an async main function with a single threaded executor.
#[doc(hidden)]
#[cfg(not(target_os = "fuchsia"))]
pub fn main_singlethreaded<F, Fut, R>(f: F, _instrument: Option<()>) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + 'static,
{
    fuchsia_async::LocalExecutorBuilder::new().build().run_singlethreaded(f())
}

/// Run an async main function with a single threaded executor, applying `role_name`.
#[doc(hidden)]
#[cfg(target_os = "fuchsia")]
pub fn main_singlethreaded_with_role<F, Fut, R>(
    f: F,
    _role_name: &'static str,
    instrument: Option<std::sync::Arc<dyn fuchsia_async::instrument::TaskInstrument>>,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + 'static,
{
    #[cfg(target_os = "fuchsia")]
    set_thread_role(_role_name);
    fuchsia_async::LocalExecutorBuilder::new()
        .instrument(instrument)
        .build()
        .run_singlethreaded(f())
}

/// Run an async main function with a single threaded executor, applying `role_name`.
#[doc(hidden)]
#[cfg(not(target_os = "fuchsia"))]
pub fn main_singlethreaded_with_role<F, Fut, R>(
    f: F,
    _role_name: &'static str,
    _instrument: Option<()>,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + 'static,
{
    fuchsia_async::LocalExecutorBuilder::new().build().run_singlethreaded(f())
}

/// Run an async main function with a multi threaded executor (containing `num_threads`).
#[doc(hidden)]
#[cfg(target_os = "fuchsia")]
pub fn main_multithreaded<F, Fut, R>(
    f: F,
    num_threads: u8,
    instrument: Option<std::sync::Arc<dyn fuchsia_async::instrument::TaskInstrument>>,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    fuchsia_async::SendExecutorBuilder::new()
        .num_threads(num_threads)
        .instrument(instrument)
        .build()
        .run(f())
}

/// Run an async main function with a multi threaded executor (containing `num_threads`).
#[doc(hidden)]
#[cfg(not(target_os = "fuchsia"))]
pub fn main_multithreaded<F, Fut, R>(f: F, num_threads: u8, _instrument: Option<()>) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    fuchsia_async::SendExecutorBuilder::new().num_threads(num_threads).build().run(f())
}

/// Run an async main function with a multi threaded executor (containing `num_threads`) and apply
/// `role_name` to all of the threads.
#[doc(hidden)]
#[cfg(target_os = "fuchsia")]
pub fn main_multithreaded_with_role<F, Fut, R>(
    f: F,
    num_threads: u8,
    _role_name: &'static str,
    instrument: Option<std::sync::Arc<dyn fuchsia_async::instrument::TaskInstrument>>,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    #[cfg(target_os = "fuchsia")]
    set_thread_role(_role_name);

    let builder =
        fuchsia_async::SendExecutorBuilder::new().num_threads(num_threads).instrument(instrument);

    #[cfg(target_os = "fuchsia")]
    let builder = builder.worker_init(move || set_thread_role(_role_name));

    builder.build().run(f())
}

/// Run an async main function with a multi threaded executor (containing `num_threads`) and apply
/// `role_name` to all of the threads.
#[doc(hidden)]
#[cfg(not(target_os = "fuchsia"))]
pub fn main_multithreaded_with_role<F, Fut, R>(
    f: F,
    num_threads: u8,
    _role_name: &'static str,
    _instrument: Option<()>,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let builder = fuchsia_async::SendExecutorBuilder::new().num_threads(num_threads);

    #[cfg(target_os = "fuchsia")]
    let builder = builder.worker_init(move || set_thread_role(_role_name));

    builder.build().run(f())
}

//
// TEST FUNCTION WRAPPERS
//

/// Run a non-async test function.
#[doc(hidden)]
pub fn test_not_async<F, R>(f: F) -> R
where
    F: FnOnce(usize) -> R,
    R: fuchsia_async::test_support::TestResult,
{
    let result = f(0);
    if result.is_ok() {
        install_lsan_hook();
    }
    result
}

/// Options for running an async test in fuchsia.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestOptions {
    /// True iff the executor should support binding Zircon interrupts.
    pub allow_interrupts: bool,
}

impl Default for TestOptions {
    fn default() -> Self {
        Self { allow_interrupts: false }
    }
}

impl From<TestOptions> for fuchsia_async::test_support::TestOptions {
    fn from(options: TestOptions) -> Self {
        Self { allow_interrupts: options.allow_interrupts }
    }
}

/// Run an async test function with a single threaded executor.
#[doc(hidden)]
pub fn test_singlethreaded<F, Fut, R>(f: F, options: TestOptions) -> R
where
    F: Fn(usize) -> Fut + Sync + 'static,
    Fut: Future<Output = R> + 'static,
    R: fuchsia_async::test_support::TestResult,
{
    let result = fuchsia_async::test_support::run_singlethreaded_test(f, options.into());
    if result.is_ok() {
        install_lsan_hook();
    }
    result
}

/// Run an async test function with a multi threaded executor (containing `num_threads`).
#[doc(hidden)]
pub fn test_multithreaded<F, Fut, R>(f: F, num_threads: u8, options: TestOptions) -> R
where
    F: Fn(usize) -> Fut + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: fuchsia_async::test_support::MultithreadedTestResult,
{
    let result = fuchsia_async::test_support::run_test(f, num_threads, options.into());
    if result.is_ok() {
        install_lsan_hook();
    }
    result
}

/// Run an async test function until it stalls. The executor will also use fake time.
#[doc(hidden)]
#[cfg(target_os = "fuchsia")]
pub fn test_until_stalled<F, Fut, R>(f: F, options: TestOptions) -> R
where
    F: 'static + Sync + Fn(usize) -> Fut,
    Fut: 'static + Future<Output = R>,
    R: fuchsia_async::test_support::TestResult,
{
    let result = fuchsia_async::test_support::run_until_stalled_test(true, f, options.into());
    if result.is_ok() {
        install_lsan_hook();
    }
    result
}

//
// FUNCTION ARGUMENT ADAPTERS
//

/// Take a main function `f` that takes an argument and return a function that takes none but calls
/// `f` with the arguments parsed via argh.
#[doc(hidden)]
pub fn adapt_to_parse_arguments<A, R>(f: impl FnOnce(A) -> R) -> impl FnOnce() -> R
where
    A: argh::TopLevelCommand,
{
    move || f(argh::from_env())
}

/// Take a test function `f` that takes no parameters and return a function that takes the run
/// number as required by our test runners.
#[doc(hidden)]
pub fn adapt_to_take_test_run_number<R>(f: impl Fn() -> R) -> impl Fn(usize) -> R {
    move |_| f()
}

//
// LEAK SANITIZER SUPPORT
//

// Note that the variant is named variant_asan (for AddressSanitizer), but the specific sanitizer we
// are targeting is lsan (LeakSanitizer), which is enabled as part of the asan variant.

#[doc(hidden)]
#[cfg(not(any(feature = "variant_asan", feature = "variant_hwasan")))]
pub fn disable_lsan_for_should_panic() {}

#[doc(hidden)]
#[cfg(any(feature = "variant_asan", feature = "variant_hwasan"))]
pub fn disable_lsan_for_should_panic() {
    unsafe extern "C" {
        fn __lsan_disable();
    }
    unsafe {
        __lsan_disable();
    }
}

#[cfg(not(any(feature = "variant_asan", feature = "variant_hwasan")))]
fn install_lsan_hook() {}

#[cfg(any(feature = "variant_asan", feature = "variant_hwasan"))]
fn install_lsan_hook() {
    unsafe extern "C" {
        fn __lsan_do_leak_check();
    }
    // Wrap the call because atexit requires a safe function pointer.
    extern "C" fn lsan_do_leak_check() {
        unsafe { __lsan_do_leak_check() }
    }
    // Wait until atexit hooks are called, in case there is more cleanup left to do.
    let err = unsafe { libc::atexit(lsan_do_leak_check) };
    if err != 0 {
        panic!("Failed to install atexit hook for LeakSanitizer: atexit returned {err}");
    }
}
