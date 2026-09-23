// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::PublishOptions;
use diagnostics_log_types::Severity;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_logger::LogSinkMarker;
use fuchsia_async as fasync;
use fuchsia_component_client::connect::connect_to_protocol;
use fuchsia_sync::Mutex;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics::Interest;
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types::Interest;

mod filter;
mod sink;

use filter::InterestFilter;
use sink::{Sink, SinkConfig};

pub use diagnostics_log_encoding::Metatag;
pub use diagnostics_log_encoding::encode::{LogEvent, TestRecord};
pub use paste::paste;

#[cfg(test)]
use std::{
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};

/// Callback for interest listeners
pub trait OnInterestChanged {
    /// Callback for when the interest changes
    fn on_changed(&self, severity: Severity);
}

/// Options to configure a `Publisher`.
pub struct PublisherOptions<'t> {
    blocking: bool,
    pub(crate) interest: Interest,
    listen_for_interest_updates: bool,
    log_sink_client: Option<ClientEnd<LogSinkMarker>>,
    pub(crate) metatags: HashSet<Metatag>,
    pub(crate) tags: &'t [&'t str],
    wait_for_initial_interest: bool,
    pub(crate) always_log_file_line: bool,
}

impl Default for PublisherOptions<'_> {
    fn default() -> Self {
        Self {
            blocking: false,
            interest: Interest::default(),
            listen_for_interest_updates: true,
            log_sink_client: None,
            metatags: HashSet::new(),
            tags: &[],
            wait_for_initial_interest: true,
            always_log_file_line: false,
        }
    }
}

impl Default for PublishOptions<'static> {
    fn default() -> Self {
        Self {
            publisher: PublisherOptions::default(),
            install_panic_hook: true,
            panic_prefix: None,
        }
    }
}

impl PublisherOptions<'_> {
    /// Creates a `PublishOptions` with all sets either empty or set to false. This is
    /// useful when fine grain control of `Publisher` and its behavior is necessary.
    ///
    /// However, for the majority of binaries that "just want to log",
    /// `PublishOptions::default` is preferred as that brings all the default
    /// configuration that is desired in most scenarios.
    pub fn empty() -> Self {
        Self {
            blocking: false,
            interest: Interest::default(),
            listen_for_interest_updates: false,
            log_sink_client: None,
            metatags: HashSet::new(),
            tags: &[],
            wait_for_initial_interest: false,
            always_log_file_line: false,
        }
    }
}
macro_rules! publisher_options {
    ($(($name:ident, $self:ident, $($self_arg:ident),*)),*) => {
        $(
            impl<'t> $name<'t> {
                /// Whether or not to block on initial runtime interest being received before
                /// starting to emit log records using the default interest configured.
                ///
                /// It's recommended that this is set when
                /// developing to guarantee that a dynamically configured minimum severity makes it
                /// to the component before it starts emitting logs.
                ///
                /// Default: true.
                pub fn wait_for_initial_interest(mut $self, enable: bool) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.wait_for_initial_interest = enable;
                    $self
                }

                /// Whether or not to log file/line information regardless of severity.
                ///
                /// Default: false.
                pub fn log_file_line_info(mut $self, enable: bool) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.always_log_file_line = enable;
                    $self
                }

                /// When set, a `fuchsia_async::Task` will be spawned and held that will be
                /// listening for interest changes.
                ///
                /// Default: true
                pub fn listen_for_interest_updates(mut $self, enable: bool) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.listen_for_interest_updates = enable;
                    $self
                }

                /// Sets the `LogSink` that will be used.
                ///
                /// Default: the `fuchsia.logger.LogSink` available in the incoming namespace.
                pub fn use_log_sink(mut $self, client: ClientEnd<LogSinkMarker>) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.log_sink_client = Some(client);
                    $self
                }

                /// When set to true, writes to the log socket will be blocking. This is, we'll
                /// retry every time the socket buffer is full until we are able to write the log.
                ///
                /// Default: false
                pub fn blocking(mut $self, is_blocking: bool) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.blocking = is_blocking;
                    $self
                }
            }
        )*
    };
}

publisher_options!((PublisherOptions, self,), (PublishOptions, self, publisher));

fn initialize_publishing(opts: PublishOptions<'_>) -> Result<Publisher, PublishError> {
    let publisher = Publisher::new(opts.publisher)?;
    publisher.register_logger()?;
    if opts.install_panic_hook {
        crate::install_panic_hook(opts.panic_prefix);
    }
    Ok(publisher)
}

/// Initializes logging with the given options.
///
/// IMPORTANT: this should be called at most once in a program, and must be
/// called only after an async executor has been set for the current thread,
/// otherwise it'll return errors or panic. Therefore it's recommended to never
/// call this from libraries and only do it from binaries.
pub fn initialize(opts: PublishOptions<'_>) -> Result<(), PublishError> {
    let _ = initialize_publishing(opts)?;
    Ok(())
}

/// Sets the global minimum log severity.
/// IMPORTANT: this function can panic if `initialize` wasn't called before.
pub fn set_minimum_severity(severity: impl Into<Severity>) {
    let severity: Severity = severity.into();
    log::set_max_level(severity.into());
}

struct AbortAndJoinOnDrop(
    Option<futures::future::AbortHandle>,
    Option<std::thread::JoinHandle<()>>,
);
impl Drop for AbortAndJoinOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = &mut self.0 {
            handle.abort();
        }
        self.1.take().unwrap().join().unwrap();
    }
}

/// Initializes logging with the given options.
///
/// This must be used when working in an environment where a [`fuchsia_async::Executor`] can't be
/// used.
///
/// IMPORTANT: this should be called at most once in a program, and must be
/// called only after an async executor has been set for the current thread,
/// otherwise it'll return errors or panic. Therefore it's recommended to never
/// call this from libraries and only do it from binaries.
pub fn initialize_sync(opts: PublishOptions<'_>) -> impl Drop {
    let (send, recv) = std::sync::mpsc::channel();
    let (ready_send, ready_recv) = {
        let (snd, rcv) = std::sync::mpsc::channel();
        if opts.publisher.wait_for_initial_interest { (Some(snd), Some(rcv)) } else { (None, None) }
    };
    let PublishOptions {
        publisher:
            PublisherOptions {
                blocking,
                interest,
                metatags,
                listen_for_interest_updates,
                log_sink_client,
                tags,
                wait_for_initial_interest,
                always_log_file_line,
            },
        install_panic_hook,
        panic_prefix,
    } = opts;
    let tags = tags.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let bg_thread = std::thread::spawn(move || {
        let options = PublishOptions {
            publisher: PublisherOptions {
                interest,
                metatags,
                tags: &tags.iter().map(String::as_ref).collect::<Vec<_>>(),
                listen_for_interest_updates,
                log_sink_client,
                wait_for_initial_interest,
                blocking,
                always_log_file_line,
            },
            install_panic_hook,
            panic_prefix,
        };
        let mut exec = fuchsia_async::LocalExecutor::default();
        let mut publisher = initialize_publishing(options).expect("initialize logging");
        if let Some(ready_send) = ready_send {
            ready_send.send(()).unwrap();
        }

        let interest_listening_task = publisher.take_interest_listening_task();

        if let Some(on_interest_changes) = interest_listening_task {
            let (on_interest_changes, cancel_interest) =
                futures::future::abortable(on_interest_changes);
            send.send(cancel_interest).unwrap();
            drop(send);
            exec.run_singlethreaded(on_interest_changes).ok();
        }
    });
    if let Some(ready_recv) = ready_recv {
        let _ = ready_recv.recv();
    }

    AbortAndJoinOnDrop(recv.recv().ok(), Some(bg_thread))
}

/// A `Publisher` acts as broker, implementing [`log::Log`] to receive log
/// events from a component, and then forwarding that data on to a diagnostics service.
#[derive(Clone)]
pub struct Publisher {
    inner: Arc<InnerPublisher>,
}

struct InnerPublisher {
    sink: Sink,
    filter: InterestFilter,
    interest_listening_task: Mutex<Option<fasync::Task<()>>>,
}

impl Default for Publisher {
    fn default() -> Self {
        Self::new(PublisherOptions::default()).expect("failed to create Publisher")
    }
}

impl Publisher {
    /// Construct a new `Publisher` using the given options.
    ///
    /// Should be called only once.
    pub fn new(opts: PublisherOptions<'_>) -> Result<Self, PublishError> {
        let client = match opts.log_sink_client {
            Some(log_sink) => log_sink,
            None => connect_to_protocol()
                .map_err(|e| e.to_string())
                .map_err(PublishError::LogSinkConnect)?,
        };
        let sink = Sink::new(
            &client,
            SinkConfig {
                tags: opts.tags.iter().map(|s| s.to_string()).collect(),
                metatags: opts.metatags,
                retry_on_buffer_full: opts.blocking,
                always_log_file_line: opts.always_log_file_line,
            },
        )?;
        let (filter, on_change) =
            InterestFilter::new(client, opts.interest, opts.wait_for_initial_interest);
        let interest_listening_task = if opts.listen_for_interest_updates {
            Mutex::new(Some(fasync::Task::spawn(on_change)))
        } else {
            Mutex::new(None)
        };
        Ok(Self { inner: Arc::new(InnerPublisher { sink, filter, interest_listening_task }) })
    }

    // TODO(https://fxbug.dev/42150573) delete this and make Publisher private
    /// Publish the provided event for testing.
    pub fn event_for_testing(&self, record: TestRecord<'_>) {
        if self.inner.filter.enabled_for_testing(&record) {
            self.inner.sink.event_for_testing(record);
        }
    }

    /// Registers an interest listener
    pub fn set_interest_listener<T>(&self, listener: T)
    where
        T: OnInterestChanged + Send + Sync + 'static,
    {
        self.inner.filter.set_interest_listener(listener);
    }

    /// Takes the task listening for interest changes if one exists.
    fn take_interest_listening_task(&mut self) -> Option<fasync::Task<()>> {
        self.inner.interest_listening_task.lock().take()
    }

    /// Sets the global logger to this publisher. This function may only be called once in the
    /// lifetime of a program.
    pub fn register_logger(&self) -> Result<(), PublishError> {
        // SAFETY: This leaks which guarantees the publisher remains alive for the lifetime of the
        // program.
        unsafe {
            let ptr = Arc::into_raw(self.inner.clone());
            log::set_logger(&*ptr).inspect_err(|_| {
                let _ = Arc::from_raw(ptr);
            })?;
        }
        Ok(())
    }
}

impl log::Log for InnerPublisher {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        // NOTE: we handle minimum severity directly through the log max_level. So we call,
        // log::set_max_level, log::max_level where appropriate.
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.sink.record_log(record);
    }

    fn flush(&self) {}
}

impl log::Log for Publisher {
    #[inline]
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    #[inline]
    fn log(&self, record: &log::Record<'_>) {
        self.inner.log(record)
    }

    #[inline]
    fn flush(&self) {
        self.inner.flush()
    }
}

/// Errors arising while forwarding a diagnostics stream to the environment.
#[derive(Debug, Error)]
pub enum PublishError {
    /// Connection to fuchsia.logger.LogSink failed.
    #[error("failed to connect to fuchsia.logger.LogSink ({0})")]
    LogSinkConnect(String),

    /// Couldn't create a new socket.
    #[error("failed to create a socket for logging")]
    MakeSocket(#[source] zx::Status),

    /// An issue with the LogSink channel or socket prevented us from sending it to the `LogSink`.
    #[error("failed to send a socket to the LogSink")]
    SendSocket(#[source] fidl::Error),

    /// Installing a Logger.
    #[error("failed to install the loger")]
    InitLogForward(#[from] log::SetLoggerError),
}

#[cfg(test)]
static CURRENT_TIME_NANOS: AtomicI64 = AtomicI64::new(Duration::from_secs(10).as_nanos() as i64);

/// Increments the test clock.
#[cfg(test)]
pub fn increment_clock(duration: Duration) {
    CURRENT_TIME_NANOS.fetch_add(duration.as_nanos() as i64, Ordering::SeqCst);
}

/// Gets the current monotonic time in nanoseconds.
#[doc(hidden)]
pub fn get_now() -> i64 {
    #[cfg(not(test))]
    return zx::MonotonicInstant::get().into_nanos();

    #[cfg(test)]
    CURRENT_TIME_NANOS.load(Ordering::Relaxed)
}

/// Logs every N seconds using an Atomic variable
/// to keep track of the time. This will have a higher
/// performance impact on ARM compared to regular logging due to the use
/// of an atomic.
#[macro_export]
macro_rules! log_every_n_seconds {
    ($seconds:expr, $severity:expr, $($arg:tt)*) => {
        use std::{time::Duration, sync::atomic::{Ordering, AtomicI64}};
        use $crate::{paste, fuchsia::get_now};

        let now = get_now();

        static LAST_LOG_TIMESTAMP: AtomicI64 = AtomicI64::new(0);
        if now - LAST_LOG_TIMESTAMP.load(Ordering::Acquire) >= Duration::from_secs($seconds).as_nanos() as i64 {
            paste! {
                log::[< $severity:lower >]!($($arg)*);
            }
            LAST_LOG_TIMESTAMP.store(now, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_reader::ArchiveReader;
    use fidl_fuchsia_diagnostics_crasher::{CrasherMarker, CrasherProxy};
    use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
    use futures::{StreamExt, future};
    use log::{debug, info};
    use moniker::ExtendedMoniker;

    #[fuchsia::test]
    async fn panic_integration_test() {
        let builder = RealmBuilder::new().await.unwrap();
        let puppet = builder
            .add_child("rust-crasher", "#meta/crasher.cm", ChildOptions::new())
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<CrasherMarker>())
                    .from(&puppet)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        let realm = builder.build().await.unwrap();
        let child_name = realm.root.child_name();
        let reader = ArchiveReader::logs();
        let (logs, _) = reader.snapshot_then_subscribe().unwrap().split_streams();
        let proxy: CrasherProxy = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
        let target_moniker =
            ExtendedMoniker::parse_str(&format!("realm_builder:{}/rust-crasher", child_name))
                .unwrap();
        proxy.crash("This is a test panic.").await.unwrap();

        let result =
            logs.filter(|data| future::ready(target_moniker == data.moniker)).next().await.unwrap();
        assert_eq!(result.line_number(), Some(29).as_ref());
        assert_eq!(
            result.file_path(),
            Some("src/lib/diagnostics/log/rust/rust-crasher/src/main.rs")
        );
        assert!(
            result
                .payload_keys()
                .unwrap()
                .get_property("info")
                .unwrap()
                .to_string()
                .contains("This is a test panic.")
        );
    }

    #[fuchsia::test(logging = false)]
    async fn verify_setting_minimum_log_severity() {
        let reader = ArchiveReader::logs();
        let (logs, _) = reader.snapshot_then_subscribe().unwrap().split_streams();
        let publisher = Publisher::new(PublisherOptions {
            tags: &["verify_setting_minimum_log_severity"],
            ..PublisherOptions::empty()
        })
        .expect("initialized log");
        log::set_boxed_logger(Box::new(publisher)).unwrap();

        info!("I'm an info log");
        debug!("I'm a debug log and won't show up");

        set_minimum_severity(Severity::Debug);
        debug!("I'm a debug log and I show up");

        let results = logs
            .filter(|data| {
                future::ready(
                    data.tags().unwrap().iter().any(|t| t == "verify_setting_minimum_log_severity"),
                )
            })
            .take(2)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(results[0].msg().unwrap(), "I'm an info log");
        assert_eq!(results[1].msg().unwrap(), "I'm a debug log and I show up");
    }

    #[fuchsia::test]
    async fn log_macro_logs_are_recorded() {
        let reader = ArchiveReader::logs();
        let (logs, _) = reader.snapshot_then_subscribe().unwrap().split_streams();

        let total_threads = 10;

        for i in 0..total_threads {
            std::thread::spawn(move || {
                log::info!(thread=i; "log from thread {}", i);
            });
        }

        let mut results = logs
            .filter(|data| {
                future::ready(
                    data.tags().unwrap().iter().any(|t| t == "log_macro_logs_are_recorded"),
                )
            })
            .take(total_threads);

        let mut seen = vec![];
        while let Some(log) = results.next().await {
            let hierarchy = log.payload_keys().unwrap();
            assert_eq!(hierarchy.properties.len(), 1);
            assert_eq!(hierarchy.properties[0].name(), "thread");
            let thread_id = hierarchy.properties[0].uint().unwrap();
            seen.push(thread_id as usize);
            assert_eq!(log.msg().unwrap(), format!("log from thread {thread_id}"));
        }
        seen.sort();
        assert_eq!(seen, (0..total_threads).collect::<Vec<_>>());
    }
}
