// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::PublishOptions;
use diagnostics_log_types::Severity;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_logger::{
    LogSinkEvent, LogSinkMarker, LogSinkOnInitRequest, LogSinkProxy, LogSinkSynchronousProxy,
};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::stream::StreamExt;
use std::borrow::Borrow;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics::Interest;
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types::Interest;

use fuchsia_component_client::connect::connect_to_protocol;
#[cfg(not(feature = "no_startup_handle"))]
use fuchsia_runtime::{HandleInfo, HandleType};

mod filter;
mod sink;

use filter::InterestFilter;
use sink::{BufferedSink, IoBufferSink, Sink, SinkConfig};

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
#[derive(Default)]
pub struct PublisherOptions<'t> {
    pub(crate) interest: Interest,
    listen_for_interest_updates: bool,
    log_sink_client: Option<ClientEnd<LogSinkMarker>>,
    pub(crate) metatags: HashSet<Metatag>,
    pub(crate) tags: &'t [&'t str],
    pub(crate) always_log_file_line: bool,
    register_global_logger: bool,
}

impl Default for PublishOptions<'static> {
    fn default() -> Self {
        Self {
            publisher: PublisherOptions {
                // Default to registering the global logger and listening for interest updates for
                // `PublishOptions` because it's used by the `initialize...` functions which are
                // typically called at program start time.
                listen_for_interest_updates: true,
                register_global_logger: true,
                ..PublisherOptions::default()
            },
            install_panic_hook: true,
            panic_prefix: None,
        }
    }
}

macro_rules! publisher_options {
    ($(($name:ident, $self:ident, $($self_arg:ident),*)),*) => {
        $(
            impl<'t> $name<'t> {
                /// Whether or not to log file/line information regardless of severity.
                ///
                /// Default: false.
                pub fn log_file_line_info(mut $self, enable: bool) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.always_log_file_line = enable;
                    $self
                }

                /// When set, a `fuchsia_async::Task` will be spawned and held that will be
                /// listening for interest changes. This option can only be set if
                /// `register_global_logger` is set.
                ///
                /// Default: true for `PublishOptions`, false for `PublisherOptions`.
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

                /// When set to true, the publisher will be registered as the global logger. This
                /// can only be done once.
                ///
                /// Default: true for `PublishOptions`, false for `PublisherOptions`.
                pub fn register_global_logger(mut $self, value: bool) -> Self {
                    let this = &mut $self$(.$self_arg)*;
                    this.register_global_logger = value;
                    $self
                }
            }
        )*
    };
}

publisher_options!((PublisherOptions, self,), (PublishOptions, self, publisher));

/// Initializes logging with the given options.
///
/// IMPORTANT: this should be called at most once in a program, and must be
/// called only after an async executor has been set for the current thread,
/// otherwise it'll return errors or panic. Therefore it's recommended to never
/// call this from libraries and only do it from binaries.
// Ideally this would be an async function, but fixing that is a bit of a Yak shave.
pub fn initialize(opts: PublishOptions<'_>) -> Result<(), PublishError> {
    let result = Publisher::new_sync_with_async_listener(opts.publisher);
    if matches!(result, Err(PublishError::MissingOnInit)) {
        // NOTE: We ignore missing OnInit errors as these can happen on products where the log sink
        // connection isn't routed. If this is a mistake, then there will be warning messages from
        // Component Manager regarding failed routing.
        return Ok(());
    }
    result?;
    if opts.install_panic_hook {
        crate::install_panic_hook(opts.panic_prefix);
    }
    Ok(())
}

/// Sets the global minimum log severity.
/// IMPORTANT: this function can panic if `initialize` wasn't called before.
pub fn set_minimum_severity(severity: impl Into<Severity>) {
    let severity: Severity = severity.into();
    log::set_max_level(severity.into());
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
pub fn initialize_sync(opts: PublishOptions<'_>) {
    match Publisher::new_sync(opts.publisher) {
        Ok(_) => {}
        Err(PublishError::MissingOnInit) => {
            // NOTE: We ignore missing OnInit errors as these can happen on products where the log
            // sink connection isn't routed. If this is a mistake, then there will be warning
            // messages from Component Manager regarding failed routing.
            return;
        }
        Err(e) => panic!("Unable to initialize logging: {e:?}"),
    }
    if opts.install_panic_hook {
        crate::install_panic_hook(opts.panic_prefix);
    }
}

/// A `Publisher` acts as broker, implementing [`log::Log`] to receive log
/// events from a component, and then forwarding that data on to a diagnostics service.
#[derive(Clone)]
pub struct Publisher {
    inner: Arc<InnerPublisher>,
}

struct InnerPublisher {
    sink: IoBufferSink,
    filter: InterestFilter,
}

impl Publisher {
    fn new(opts: PublisherOptions<'_>, iob: zx::Iob) -> Self {
        Self {
            inner: Arc::new(InnerPublisher {
                sink: IoBufferSink::new(
                    iob,
                    SinkConfig {
                        tags: opts.tags.iter().map(|s| s.to_string()).collect(),
                        metatags: opts.metatags,
                        always_log_file_line: opts.always_log_file_line,
                    },
                ),
                filter: InterestFilter::new(opts.interest),
            }),
        }
    }

    /// Returns a new `Publisher`. This will connect synchronously and, if configured, run a
    /// listener in a separate thread.
    pub fn new_sync(opts: PublisherOptions<'_>) -> Result<Self, PublishError> {
        let listen_for_interest_updates = opts.listen_for_interest_updates;
        let (publisher, client) = Self::new_sync_no_listener(opts)?;
        if listen_for_interest_updates {
            let publisher = publisher.clone();
            std::thread::spawn(move || {
                fuchsia_async::LocalExecutor::default()
                    .run_singlethreaded(publisher.listen_for_interest_updates(client.into_proxy()));
            });
        }
        Ok(publisher)
    }

    /// Returns a new `Publisher`. This will connect synchronously and, if configured, run a
    /// listener in an async task. Prefer to use `new_async`.
    pub fn new_sync_with_async_listener(opts: PublisherOptions<'_>) -> Result<Self, PublishError> {
        let listen_for_interest_updates = opts.listen_for_interest_updates;
        let (publisher, client) = Self::new_sync_no_listener(opts)?;
        if listen_for_interest_updates {
            fasync::Task::spawn(publisher.clone().listen_for_interest_updates(client.into_proxy()))
                .detach();
        }
        Ok(publisher)
    }

    /// Returns a new `Publisher`, but doesn't listen for interest updates. This will connect
    /// synchronously.
    fn new_sync_no_listener(
        mut opts: PublisherOptions<'_>,
    ) -> Result<(Self, ClientEnd<LogSinkMarker>), PublishError> {
        let PublisherOptions { listen_for_interest_updates, register_global_logger, .. } = opts;

        if listen_for_interest_updates && !register_global_logger {
            // We can only support listening for interest updates if we are registering a global
            // logger. This is because if we don't register, the initial interest is dropped.
            return Err(PublishError::UnsupportedOption);
        }

        let mut get_client = || match opts.log_sink_client.take() {
            Some(log_sink) => Ok(log_sink),
            None => {
                #[cfg(not(feature = "no_startup_handle"))]
                {
                    let log_sink = fuchsia_runtime::take_startup_handle(HandleInfo::new(
                        HandleType::LogSink,
                        0,
                    ));
                    if let Some(log_sink) = log_sink {
                        let log_sink = fidl::Channel::from(log_sink);
                        return Ok(ClientEnd::<LogSinkMarker>::from(log_sink));
                    }
                }
                // If startup handle is unavailable fallback to /svc.
                connect_to_protocol()
                    .map_err(|e| e.to_string())
                    .map_err(PublishError::LogSinkConnect)
            }
        };
        let client = (get_client)()?;

        let proxy = zx::Unowned::<LogSinkSynchronousProxy>::new(client.channel());
        let Ok(LogSinkEvent::OnInit {
            payload: LogSinkOnInitRequest { buffer: Some(iob), interest, .. },
        }) = proxy.wait_for_event(zx::MonotonicInstant::INFINITE)
        else {
            return Err(PublishError::MissingOnInit);
        };

        let publisher = Self::new(opts, iob);

        if register_global_logger {
            publisher.register_logger(if listen_for_interest_updates { interest } else { None })?;
        }

        Ok((publisher, client))
    }

    /// Returns a new `Publisher`. This will connect asynchronously and, if configured, run a
    /// listener in an async task.
    pub async fn new_async(mut opts: PublisherOptions<'_>) -> Result<Self, PublishError> {
        let PublisherOptions { listen_for_interest_updates, register_global_logger, .. } = opts;

        if listen_for_interest_updates && !register_global_logger {
            // We can only support listening for interest updates if we are registering a global
            // logger. This is because if we don't register, the initial interest is dropped.
            return Err(PublishError::UnsupportedOption);
        }

        let mut get_client = || match opts.log_sink_client.take() {
            Some(log_sink) => Ok(log_sink.into_proxy()),
            None => {
                #[cfg(not(feature = "no_startup_handle"))]
                {
                    let log_sink = fuchsia_runtime::take_startup_handle(HandleInfo::new(
                        HandleType::LogSink,
                        0,
                    ));
                    if let Some(log_sink) = log_sink {
                        let log_sink = fidl::Channel::from(log_sink);
                        let log_sink = ClientEnd::<LogSinkMarker>::from(log_sink);
                        return Ok(log_sink.into_proxy());
                    }
                }
                // If startup handle is unavailable fallback to /svc.
                connect_to_protocol()
                    .map_err(|e| e.to_string())
                    .map_err(PublishError::LogSinkConnect)
            }
        };
        let proxy = (get_client)()?;

        let Some(Ok(LogSinkEvent::OnInit {
            payload: LogSinkOnInitRequest { buffer: Some(iob), interest, .. },
        })) = proxy.take_event_stream().next().await
        else {
            return Err(PublishError::MissingOnInit);
        };

        let publisher = Self::new(opts, iob);

        if register_global_logger {
            publisher.register_logger(if listen_for_interest_updates { interest } else { None })?;
            fasync::Task::spawn(publisher.clone().listen_for_interest_updates(proxy)).detach();
        }

        Ok(publisher)
    }

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

    /// Sets the global logger to this publisher. This function may only be called once in the
    /// lifetime of a program.
    pub fn register_logger(&self, interest: Option<Interest>) -> Result<(), PublishError> {
        self.inner.filter.update_interest(interest.unwrap_or_default());
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

    /// Listens for interest updates. Callers must maintain a clone to keep the publisher alive;
    /// this function will downgrade to a weak reference.
    async fn listen_for_interest_updates(self, proxy: LogSinkProxy) {
        self.inner.filter.listen_for_interest_updates(proxy).await;
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

impl Borrow<InterestFilter> for InnerPublisher {
    fn borrow(&self) -> &InterestFilter {
        &self.filter
    }
}

/// Initializes logging, but buffers logs until the connection is established. This is required for
/// things like Component Manager, which would otherwise deadlock when starting. This carries some
/// overhead, so should be avoided unless required.
pub fn initialize_buffered(opts: PublishOptions<'_>) -> Result<(), PublishError> {
    BufferedPublisher::new(opts.publisher)?;
    if opts.install_panic_hook {
        crate::install_panic_hook(opts.panic_prefix);
    }
    Ok(())
}

/// A buffered publisher will buffer log messages until the IOBuffer is received. If this is
/// registered as the global logger, then messages will be logged at the default level until an
/// updated level is received from Archivist.
pub struct BufferedPublisher {
    sink: BufferedSink,
    filter: InterestFilter,
    interest_listening_task: Mutex<Option<fasync::Task<()>>>,
}

impl BufferedPublisher {
    /// Returns a publisher that will buffer messages until the IOBuffer is received. An async
    /// executor must be established.
    pub fn new(opts: PublisherOptions<'_>) -> Result<Arc<Self>, PublishError> {
        if opts.listen_for_interest_updates && !opts.register_global_logger {
            // We can only support listening for interest updates if we are registering a global
            // logger. This is because if we don't register, the initial interest is dropped.
            return Err(PublishError::UnsupportedOption);
        }

        let get_client = || match opts.log_sink_client {
            Some(log_sink) => Ok(log_sink),
            None => {
                #[cfg(not(feature = "no_startup_handle"))]
                {
                    let log_sink = fuchsia_runtime::take_startup_handle(HandleInfo::new(
                        HandleType::LogSink,
                        0,
                    ));
                    if let Some(log_sink) = log_sink {
                        let log_sink = fidl::Channel::from(log_sink);
                        return Ok(ClientEnd::<LogSinkMarker>::from(log_sink));
                    }
                }
                // If startup handle is unavailable fallback to /svc.
                connect_to_protocol()
                    .map_err(|e| e.to_string())
                    .map_err(PublishError::LogSinkConnect)
            }
        };
        let client = (get_client)()?;

        let this = Arc::new(Self {
            sink: BufferedSink::new(SinkConfig {
                tags: opts.tags.iter().map(|s| s.to_string()).collect(),
                metatags: opts.metatags,
                always_log_file_line: opts.always_log_file_line,
            }),
            filter: InterestFilter::new(opts.interest),
            interest_listening_task: Mutex::default(),
        });

        if opts.register_global_logger {
            // SAFETY: This leaks which guarantees the publisher remains alive for the lifetime
            // of the program. This leaks even when there is an error (which shouldn't happen so
            // we don't worry about it).
            unsafe {
                log::set_logger(&*Arc::into_raw(this.clone()))?;
            }
        }

        // Whilst we are waiting for the OnInit event, we hold a strong reference to the publisher
        // which will prevent the publisher from being dropped and ensure that buffered log messages
        // are sent.
        let this_clone = this.clone();
        *this_clone.interest_listening_task.lock() = Some(fasync::Task::spawn(async move {
            let proxy = client.into_proxy();

            let Some(Ok(LogSinkEvent::OnInit {
                payload: LogSinkOnInitRequest { buffer: Some(buffer), interest, .. },
            })) = proxy.take_event_stream().next().await
            else {
                // There's not a lot we can do here: we haven't received the event we expected
                // and there's no way we can log the issue.
                return;
            };

            // Ignore the interest sent in the OnInit request if `listen_for_interest_updates`
            // is false; it is assumed that the caller wants the interest specified in the
            // options to stick.
            this.filter.update_interest(
                (if opts.listen_for_interest_updates { interest } else { None })
                    .unwrap_or_default(),
            );

            this.sink.set_buffer(buffer);

            if opts.listen_for_interest_updates {
                this.filter.listen_for_interest_updates(proxy).await;
            }
        }));

        Ok(this_clone)
    }
}

impl log::Log for BufferedPublisher {
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

impl Borrow<InterestFilter> for BufferedPublisher {
    fn borrow(&self) -> &InterestFilter {
        &self.filter
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

    /// Unsupported publish option.
    #[error("unsupported option")]
    UnsupportedOption,

    /// The channel was closed with no OnInit event.
    #[error("did not receive the OnInit event")]
    MissingOnInit,
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
    use assert_matches::assert_matches;
    use diagnostics_reader::ArchiveReader;
    use fidl_fuchsia_diagnostics_crasher::{CrasherMarker, CrasherProxy};
    use fuchsia_async::TimeoutExt;
    use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
    use futures::{FutureExt, StreamExt, future};
    use log::{debug, error, info};
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
        let _publisher = Publisher::new_async(PublisherOptions {
            tags: &["verify_setting_minimum_log_severity"],
            register_global_logger: true,
            ..PublisherOptions::default()
        })
        .await
        .expect("initialized log");

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
        let (logs, mut errors) = reader.snapshot_then_subscribe().unwrap().split_streams();

        let total_threads = 10;

        for i in 0..total_threads {
            std::thread::spawn(move || {
                log::info!(thread=i; "log from thread {}", i);
            });
        }

        let mut filtered = 0;
        let mut results = logs
            .filter(|data| {
                future::ready(
                    if data.tags().unwrap().iter().any(|t| t == "log_macro_logs_are_recorded") {
                        true
                    } else {
                        filtered += 1;
                        false
                    },
                )
            })
            .take(total_threads);

        let mut seen = vec![];
        while let Some(log) = results
            .next()
            .on_timeout(Duration::from_secs(30), || {
                error!("Timed out!");
                None
            })
            .await
        {
            let hierarchy = log.payload_keys().unwrap();
            assert_eq!(hierarchy.properties.len(), 1);
            assert_eq!(hierarchy.properties[0].name(), "thread");
            let thread_id = hierarchy.properties[0].uint().unwrap();
            seen.push(thread_id as usize);
            assert_eq!(log.msg().unwrap(), format!("log from thread {thread_id}"));
        }

        assert_matches!(errors.next().now_or_never(), None);

        seen.sort();
        assert_eq!(seen, (0..total_threads).collect::<Vec<_>>(), "filtered={filtered}");
    }
}
