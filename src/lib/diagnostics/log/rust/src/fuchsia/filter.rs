// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::OnInterestChanged;
use diagnostics_log_encoding::encode::TestRecord;
use diagnostics_log_types::Severity;
use fidl_fuchsia_logger::LogSinkProxy;
use fuchsia_sync::Mutex;

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics as fdiagnostics;
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types as fdiagnostics;

pub(crate) struct InterestFilter {
    default_severity: Severity,
    listener: Mutex<Option<Box<dyn OnInterestChanged + Send + Sync + 'static>>>,
}

impl InterestFilter {
    /// Returns a new `InterestFilter` with the default interest.
    ///
    /// NOTE: This does not update the global maximum log level because some callers don't want
    /// that. Calling `update_interest` *will* update the global maximum log level.
    pub fn new(default_interest: fdiagnostics::Interest) -> Self {
        Self {
            default_severity: default_interest.min_severity.map_or(Severity::Info, Severity::from),
            listener: Mutex::default(),
        }
    }

    /// Sets the interest listener.
    pub fn set_interest_listener<T>(&self, listener: T)
    where
        T: OnInterestChanged + Send + Sync + 'static,
    {
        let mut listener_guard = self.listener.lock();
        *listener_guard = Some(Box::new(listener));
    }

    /// Listen for interest updates.
    pub async fn listen_for_interest_updates(&self, proxy: LogSinkProxy) {
        while let Ok(Ok(interest)) = proxy.wait_for_interest_change().await {
            self.update_interest(interest);
        }
    }

    /// Updates the global interest.
    pub fn update_interest(&self, interest: fdiagnostics::Interest) {
        let new_min_severity = interest.min_severity.map_or(self.default_severity, Severity::from);
        log::set_max_level(new_min_severity.into());
        let callback_guard = self.listener.lock();
        if let Some(callback) = &*callback_guard {
            callback.on_changed(new_min_severity);
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
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_logger::{LogSinkMarker, LogSinkRequest, LogSinkRequestStream};
    use futures::channel::mpsc;
    use futures::{StreamExt, TryStreamExt};
    use log::{debug, error, info, trace, warn};
    use std::sync::Arc;

    struct SeverityTracker {
        _filter: Arc<InterestFilter>,
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
        let filter = Arc::new(InterestFilter::new(fdiagnostics::Interest::default()));
        filter.update_interest(fdiagnostics::Interest::default());
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
        let (proxy, mut requests) = create_proxy_and_stream::<LogSinkMarker>();
        let filter = Arc::new(InterestFilter::new(fdiagnostics::Interest {
            min_severity: Some(fdiagnostics::Severity::Warn),
            ..Default::default()
        }));
        let (send, mut recv) = mpsc::unbounded();
        filter.set_interest_listener(InterestChangedListener(send));
        let _on_changes_task = fuchsia_async::Task::spawn({
            let filter = filter.clone();
            async move { filter.listen_for_interest_updates(proxy).await }
        });
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
    async fn log_frontend_tracks_severity() {
        // Manually set to a known value.
        log::set_max_level(log::LevelFilter::Off);

        let (proxy, mut requests) = create_proxy_and_stream::<LogSinkMarker>();
        let filter = Arc::new(InterestFilter::new(fdiagnostics::Interest {
            min_severity: Some(fdiagnostics::Severity::Warn),
            ..Default::default()
        }));
        // The filter shouldn't set the global level until it receives an interest update.
        assert_eq!(log::max_level(), log::LevelFilter::Off);
        assert_eq!(filter.default_severity, Severity::Warn);

        let (send, mut recv) = mpsc::unbounded();
        filter.set_interest_listener(InterestChangedListener(send));
        let _on_changes_task = fuchsia_async::Task::spawn(async move {
            filter.listen_for_interest_updates(proxy).await;
        });

        send_interest_change(&mut requests, Some(Severity::Trace)).await;
        recv.next().await.unwrap();
        assert_eq!(log::max_level(), log::LevelFilter::Trace);

        send_interest_change(&mut requests, Some(Severity::Info)).await;
        recv.next().await.unwrap();
        assert_eq!(log::max_level(), log::LevelFilter::Info);
    }
}
