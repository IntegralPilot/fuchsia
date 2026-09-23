// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::OnInterestChanged;
use diagnostics_log_encoding::encode::TestRecord;
use diagnostics_log_types::Severity;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_logger::{LogSinkMarker, LogSinkProxy, LogSinkSynchronousProxy};
use fuchsia_sync::Mutex;
use std::future::Future;
use std::sync::Arc;

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics as fdiagnostics;
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types as fdiagnostics;

pub(crate) struct InterestFilter {
    listener: Arc<Mutex<Option<Box<dyn OnInterestChanged + Send + Sync + 'static>>>>,
}

impl InterestFilter {
    /// Constructs a new `InterestFilter` and a future which should be polled to listen
    /// to changes in the LogSink's interest.
    pub fn new(
        client: ClientEnd<LogSinkMarker>,
        interest: fdiagnostics::Interest,
        wait_for_initial_interest: bool,
    ) -> (Self, impl Future<Output = ()>) {
        let default_severity = interest.min_severity.map(Severity::from).unwrap_or(Severity::Info);
        let min_severity = if wait_for_initial_interest {
            let sync_proxy = zx::Unowned::<LogSinkSynchronousProxy>::new(client.channel());
            match sync_proxy.wait_for_interest_change(zx::MonotonicInstant::INFINITE) {
                Ok(Ok(initial_interest)) => {
                    initial_interest.min_severity.map(Severity::from).unwrap_or(default_severity)
                }
                _ => default_severity,
            }
        } else {
            default_severity
        };

        log::set_max_level(min_severity.into());

        let listener = Arc::new(Mutex::new(None));
        let filter = Self { listener: listener.clone() };
        (filter, Self::listen_to_interest_changes(listener, default_severity, client.into_proxy()))
    }

    /// Sets the interest listener.
    pub fn set_interest_listener<T>(&self, listener: T)
    where
        T: OnInterestChanged + Send + Sync + 'static,
    {
        let mut listener_guard = self.listener.lock();
        *listener_guard = Some(Box::new(listener));
    }

    async fn listen_to_interest_changes(
        listener: Arc<Mutex<Option<Box<dyn OnInterestChanged + Send + Sync>>>>,
        default_severity: Severity,
        proxy: LogSinkProxy,
    ) {
        while let Ok(Ok(interest)) = proxy.wait_for_interest_change().await {
            let new_min_severity =
                interest.min_severity.map(Severity::from).unwrap_or(default_severity);
            log::set_max_level(new_min_severity.into());
            let callback_guard = listener.lock();
            if let Some(callback) = &*callback_guard {
                callback.on_changed(new_min_severity);
            }
        }
    }

    pub fn enabled_for_testing(&self, record: &TestRecord<'_>) -> bool {
        let min_severity = Severity::try_from(log::max_level()).map(|s| s as u8).unwrap_or(u8::MAX);
        min_severity <= record.severity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_request_stream;
    use fidl_fuchsia_logger::{LogSinkMarker, LogSinkRequest, LogSinkRequestStream};
    use futures::channel::mpsc;
    use futures::{StreamExt, TryStreamExt};
    use log::{debug, error, info, trace, warn};

    struct SeverityTracker {
        _filter: InterestFilter,
        severity_counts: Arc<Mutex<SeverityCount>>,
    }

    impl log::Log for SeverityTracker {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            let mut count = self.severity_counts.lock();
            let to_increment = match record.level() {
                log::Level::Trace => &mut count.trace,
                log::Level::Debug => &mut count.debug,
                log::Level::Info => &mut count.info,
                log::Level::Warn => &mut count.warn,
                log::Level::Error => &mut count.error,
            };
            *to_increment += 1;
        }

        fn flush(&self) {}
    }

    #[derive(Debug, Default, Eq, PartialEq)]
    struct SeverityCount {
        trace: u64,
        debug: u64,
        info: u64,
        warn: u64,
        error: u64,
    }

    struct InterestChangedListener(mpsc::UnboundedSender<()>);

    impl OnInterestChanged for InterestChangedListener {
        fn on_changed(&self, _: crate::Severity) {
            self.0.unbounded_send(()).unwrap();
        }
    }

    #[fuchsia::test(logging = false)]
    async fn default_filter_is_info_when_unspecified() {
        let (client, _requests) = create_request_stream::<LogSinkMarker>();
        let (filter, _on_changes) =
            InterestFilter::new(client, fdiagnostics::Interest::default(), false);
        let observed = Arc::new(Mutex::new(SeverityCount::default()));
        log::set_boxed_logger(Box::new(SeverityTracker {
            severity_counts: observed.clone(),
            _filter: filter,
        }))
        .unwrap();
        let mut expected = SeverityCount::default();

        error!("oops");
        expected.error += 1;
        assert_eq!(&*observed.lock(), &expected);

        warn!("maybe");
        expected.warn += 1;
        assert_eq!(&*observed.lock(), &expected);

        info!("ok");
        expected.info += 1;
        assert_eq!(&*observed.lock(), &expected);

        debug!("hint");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");

        trace!("spew");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");
    }

    async fn send_interest_change(stream: &mut LogSinkRequestStream, severity: Option<Severity>) {
        match stream.try_next().await {
            Ok(Some(LogSinkRequest::WaitForInterestChange { responder })) => {
                responder
                    .send(Ok(&fdiagnostics::Interest {
                        min_severity: severity.map(fdiagnostics::Severity::from),
                        ..Default::default()
                    }))
                    .expect("send response");
            }
            other => panic!("Expected WaitForInterestChange but got {:?}", other),
        }
    }

    #[fuchsia::test(logging = false)]
    async fn default_filter_on_interest_changed() {
        let (client, mut requests) = create_request_stream::<LogSinkMarker>();
        let (filter, on_changes) = InterestFilter::new(
            client,
            fdiagnostics::Interest {
                min_severity: Some(fdiagnostics::Severity::Warn),
                ..Default::default()
            },
            false,
        );
        let (send, mut recv) = mpsc::unbounded();
        filter.set_interest_listener(InterestChangedListener(send));
        let _on_changes_task = fuchsia_async::Task::spawn(on_changes);
        let observed = Arc::new(Mutex::new(SeverityCount::default()));
        log::set_boxed_logger(Box::new(SeverityTracker {
            severity_counts: observed.clone(),
            _filter: filter,
        }))
        .expect("set logger");

        // After overriding to info, filtering is at info level. The mpsc channel is used to
        // get a signal as to when the filter has processed the update.
        send_interest_change(&mut requests, Some(Severity::Info)).await;
        recv.next().await.unwrap();

        let mut expected = SeverityCount::default();
        error!("oops");
        expected.error += 1;
        assert_eq!(&*observed.lock(), &expected);

        warn!("maybe");
        expected.warn += 1;
        assert_eq!(&*observed.lock(), &expected);

        info!("ok");
        expected.info += 1;
        assert_eq!(&*observed.lock(), &expected);

        debug!("hint");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");

        trace!("spew");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");

        // After resetting to default, filtering is at warn level.
        send_interest_change(&mut requests, None).await;
        recv.next().await.unwrap();

        error!("oops");
        expected.error += 1;
        assert_eq!(&*observed.lock(), &expected);

        warn!("maybe");
        expected.warn += 1;
        assert_eq!(&*observed.lock(), &expected);

        info!("ok");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");

        debug!("hint");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");

        trace!("spew");
        assert_eq!(&*observed.lock(), &expected, "should not increment counters");
    }

    #[fuchsia::test(logging = false)]
    async fn wait_for_initial_interest() {
        let (client, mut requests) = create_request_stream::<LogSinkMarker>();
        let t = std::thread::spawn(move || {
            // Unused, but its existence is needed by AsyncChannel.
            let _executor = fuchsia_async::LocalExecutor::default();
            let (filter, _on_changes) =
                InterestFilter::new(client, fdiagnostics::Interest::default(), true);
            filter
        });
        if let Some(Ok(request)) = requests.next().await {
            match request {
                LogSinkRequest::WaitForInterestChange { responder } => {
                    responder
                        .send(Ok(&fdiagnostics::Interest {
                            min_severity: Some(fdiagnostics::Severity::Trace),
                            ..Default::default()
                        }))
                        .expect("sent initial interest");
                }
                other => panic!("Got unexpected: {:?}", other),
            };
        }
        let _filter = t.join().unwrap();
        assert_eq!(log::max_level(), log::Level::Trace);
    }

    #[fuchsia::test(logging = false)]
    async fn log_frontend_tracks_severity() {
        // Manually set to a known value.
        log::set_max_level(log::LevelFilter::Off);

        let (client, mut requests) = create_request_stream::<LogSinkMarker>();
        let (filter, on_changes) = InterestFilter::new(
            client,
            fdiagnostics::Interest {
                min_severity: Some(fdiagnostics::Severity::Warn),
                ..Default::default()
            },
            false,
        );
        // Log frontend tracks the default min_severity.
        assert_eq!(log::max_level(), log::LevelFilter::Warn);

        let (send, mut recv) = mpsc::unbounded();
        filter.set_interest_listener(InterestChangedListener(send));
        let _on_changes_task = fuchsia_async::Task::spawn(on_changes);

        send_interest_change(&mut requests, Some(Severity::Trace)).await;
        recv.next().await.unwrap();
        assert_eq!(log::max_level(), log::LevelFilter::Trace);

        send_interest_change(&mut requests, Some(Severity::Info)).await;
        recv.next().await.unwrap();
        assert_eq!(log::max_level(), log::LevelFilter::Info);
    }
}
