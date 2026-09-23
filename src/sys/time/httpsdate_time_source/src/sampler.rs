// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Config;
use crate::bound::Bound;
use crate::datatypes::{HttpsSample, Poll};
use anyhow::format_err;
use async_trait::async_trait;
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_runtime::{BootDurationExt, UtcDuration, UtcDurationExt, UtcInstant};

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use httpdate_hyper::{HttpsDateError, HttpsDateErrorType, NetworkTimeClient};
use hyper::Uri;
use log::warn;

const NANOS_IN_SECONDS: i64 = 1_000_000_000;

#[async_trait]
/// An `HttpsDateClient` can make requests against a given uri to retrieve a UTC time.
pub trait HttpsDateClient {
    /// Poll |uri| once to obtain the current UTC time. The time is quantized to a second due to
    /// the format of the HTTP date header.
    ///
    /// The timeout is on the monotonic timeline, to ensure no timer storms after a wakeup.
    async fn request_utc(
        &mut self,
        uri: &Uri,
        https_timeout: zx::BootDuration,
    ) -> Result<UtcInstant, HttpsDateError>;
}

#[async_trait]
impl HttpsDateClient for NetworkTimeClient {
    async fn request_utc(
        &mut self,
        uri: &Uri,
        https_timeout: zx::BootDuration,
    ) -> Result<UtcInstant, HttpsDateError> {
        let utc = self
            .get_network_time(uri.clone())
            .on_timeout(fasync::BootInstant::after(https_timeout), || {
                Err(HttpsDateError::new(HttpsDateErrorType::NetworkError)
                    .with_source(format_err!("Timed out after {:?}", https_timeout)))
            })
            .await?;
        Ok(UtcInstant::from_nanos(utc.timestamp_nanos_opt().expect("the timestamp should exist")))
    }
}

/// An `HttpsSampler` produces `HttpsSample`s by polling an HTTP server, possibly by combining
/// the results of multiple polls.
#[async_trait]
pub trait HttpsSampler {
    /// Produce a single `HttpsSample` by polling |num_polls| times. Returns an Ok result
    /// containing a Future as soon as the first poll succeeds. The Future yields a sample. This
    /// signature enables the caller to act on a success as soon as possible without waiting for
    /// all polls to complete.
    async fn produce_sample(
        &self,
        num_polls: usize,
    ) -> Result<BoxFuture<'_, HttpsSample>, HttpsDateError>;
}

/// The default implementation of `HttpsSampler` that uses an `HttpsDateClient` to poll a server.
pub struct HttpsSamplerImpl<'a, C: HttpsDateClient> {
    /// Client used to poll servers for time.
    client: Mutex<C>,
    /// URI called to obtain time.
    uri: Uri,
    /// HttpsDate config.
    config: &'a Config,
}

impl<'a> HttpsSamplerImpl<'a, NetworkTimeClient> {
    /// Create a new `HttpsSamplerImpl` that makes requests against `uri` to poll time.
    pub fn new(uri: Uri, config: &'a Config) -> Self {
        Self::new_with_client(uri, NetworkTimeClient::new(), config)
    }
}

impl<'a, C: HttpsDateClient + Send> HttpsSamplerImpl<'a, C> {
    fn new_with_client(uri: Uri, client: C, config: &'a Config) -> Self {
        Self { client: Mutex::new(client), uri, config }
    }
}

#[async_trait]
impl<C: HttpsDateClient + Send> HttpsSampler for HttpsSamplerImpl<'_, C> {
    async fn produce_sample(
        &self,
        num_polls: usize,
    ) -> Result<BoxFuture<'_, HttpsSample>, HttpsDateError> {
        // Don't measure offset on the initial poll, as setting up TLS connections causes this poll
        // to take longer.
        let (mut bound, first_poll) = self.poll_server().await?;
        let mut polls = vec![first_poll];

        let sample_fut = async move {
            for _poll_idx in 1..num_polls {
                let ideal_next_poll_time = ideal_next_poll_time(
                    &bound,
                    polls.iter().map(|poll| &poll.round_trip_time),
                    self.config.first_rtt_time_factor,
                );
                fasync::Timer::new(ideal_next_poll_time).await;

                // For subsequent polls ignore errors. This allows producing a degraded sample
                // instead of outright failing as long as one poll succeeds.
                if let Ok((new_bound, new_poll)) = self.poll_server().await {
                    bound = match bound.combine(&new_bound) {
                        Some(combined) => combined,
                        None => {
                            // Bounds might fail to combine if e.g. the device went to sleep and
                            // monotonic time was not updated. We assume the most recent poll is
                            // most accurate and discard accumulated information.
                            // TODO(b/365667080): report this event to Cobalt
                            polls.clear();
                            warn!("Unable to combine time bound, time may have moved.");
                            new_bound
                        }
                    };
                    polls.push(new_poll);
                }
            }

            HttpsSample {
                utc: bound.center(),
                reference: bound.reference,
                standard_deviation: bound.size() * self.config.standard_deviation_bound_percentage
                    / 100,
                final_bound_size: bound.size(),
                polls,
            }
        };

        Ok(sample_fut.boxed())
    }
}

impl<C: HttpsDateClient + Send> HttpsSamplerImpl<'_, C> {
    /// Poll the server once to produce a fresh bound on the UTC time. Returns a bound and the
    /// observed round trip time.
    async fn poll_server(&self) -> Result<(Bound, Poll), HttpsDateError> {
        let boot_before = zx::BootInstant::get();
        let reported_utc =
            self.client.lock().await.request_utc(&self.uri, self.config.https_timeout).await?;
        let boot_after = zx::BootInstant::get();
        let round_trip_time = boot_after - boot_before;
        let monotonic_center = boot_before + round_trip_time / 2;
        // We assume here that the time reported by an HTTP server is truncated down to the second.
        // Thus the actual time on the server is in the range [reported_utc, reported_utc + 1).
        // Network latency adds additional uncertainty and is accounted for by expanding the bound
        // in either direction by half the observed round trip time.
        let bound = Bound {
            reference: monotonic_center,
            utc_min: reported_utc - round_trip_time.to_utc_lossy() / 2,
            utc_max: reported_utc
                + UtcDuration::from_seconds(1)
                + round_trip_time.to_utc_lossy() / 2,
        };
        let poll = Poll { round_trip_time };
        Ok((bound, poll))
    }
}

/// Given a bound and observed round trip times, estimates the ideal time at which
/// to poll the server.
fn ideal_next_poll_time<'a, I>(
    bound: &Bound,
    mut observed_rtt: I,
    first_rtt_time_factor: u16,
) -> zx::BootInstant
where
    I: Iterator<Item = &'a zx::BootDuration> + ExactSizeIterator,
{
    // Estimate the ideal monotonic time we'd like the server to check time.
    // ideal_server_check_time is a monotonic time at which bound's projection is centered
    // around a whole number of UTC seconds (utc_min = n - k, utc_max = n + k) where n is a
    // whole number of seconds.
    // Ignoring network latency, the bound produced by polling the server at
    // ideal_server_check_time must be [n-1, n) or [n, n + 1). In either case combining with
    // the original bound results in a bound half the size of the original.
    let ideal_server_check_time = bound.reference + zx::BootDuration::from_seconds(1)
        - time_subs(bound.center()).to_boot_lossy();

    // Since there is actually network latency, try to guess what it'll be and start polling
    // early so the server checks at the ideal time. The first poll takes longer than subsequent
    // polls due to TLS handshaking, so we make a best effort to account for that when the first
    // poll is the only one available. Otherwise, we discard the first poll rtt.
    let rtt_guess = match observed_rtt.len() {
        0 => return zx::BootInstant::get(),
        1 => *observed_rtt.next().unwrap() / first_rtt_time_factor,
        _ => avg(observed_rtt.skip(1)),
    };
    let ideal_poll_start_time = ideal_server_check_time - rtt_guess / 2;

    // Adjust the time in case it has already passed.
    let now = zx::BootInstant::get();
    if now < ideal_poll_start_time {
        ideal_poll_start_time
    } else {
        ideal_poll_start_time
            + zx::BootDuration::from_seconds(1)
            + seconds(now - ideal_poll_start_time)
    }
}

/// Calculates the average of a set of small durations. May overflow for large durations.
fn avg<'a, I, T>(durations: I) -> zx::Duration<T>
where
    I: ExactSizeIterator + Iterator<Item = &'a zx::Duration<T>>,
    T: zx::Timeline + Copy + 'a,
{
    let count = durations.len() as i64;
    let raw_nanos = durations.into_iter().map(|d| d.into_nanos()).sum::<i64>();
    let result = raw_nanos / count;
    zx::Duration::<T>::from_nanos(result)
}

/// Returns the whole second component of a zx::BootDuration.
fn seconds<T>(duration: zx::Duration<T>) -> zx::Duration<T>
where
    T: zx::Timeline + std::marker::Copy,
{
    duration - subs(duration)
}

/// Returns the subsecond component of a zx::BootDuration.
fn subs<T>(duration: zx::Duration<T>) -> zx::Duration<T>
where
    T: zx::Timeline,
{
    zx::Duration::<T>::from_nanos(duration.into_nanos() % NANOS_IN_SECONDS)
}

/// Returns the subsecond component of an Instant.
fn time_subs<T>(time: zx::Instant<T>) -> zx::Duration<T>
where
    T: zx::Timeline,
{
    zx::Duration::<T>::from_nanos(time.into_nanos() % NANOS_IN_SECONDS)
}

#[cfg(test)]
pub use fake::FakeSampler;
#[cfg(test)]
mod fake {
    use super::*;
    use futures::Future;
    use futures::channel::oneshot;
    use futures::future::pending;
    use std::collections::VecDeque;

    /// An |HttpsSampler| which responds with premade responses and signals when the responses
    /// have been given out.
    pub struct FakeSampler {
        /// Queue of responses.
        enqueued_responses: Mutex<VecDeque<Result<HttpsSample, HttpsDateError>>>,
        /// Channel used to signal exhaustion of the enqueued responses.
        completion_notifier: Mutex<Option<oneshot::Sender<()>>>,
        /// List of `produce_sample` request arguments received.
        received_request_num_polls: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl HttpsSampler for FakeSampler {
        async fn produce_sample(
            &self,
            num_polls: usize,
        ) -> Result<BoxFuture<'_, HttpsSample>, HttpsDateError> {
            match self.enqueued_responses.lock().await.pop_front() {
                Some(result) => {
                    self.received_request_num_polls.lock().await.push(num_polls);
                    result.map(|sample| futures::future::ready(sample).boxed())
                }
                None => {
                    self.completion_notifier.lock().await.take().unwrap().send(()).unwrap();
                    pending().await
                }
            }
        }
    }

    #[async_trait]
    impl<T: AsRef<FakeSampler> + Send + Sync> HttpsSampler for T {
        async fn produce_sample(
            &self,
            num_polls: usize,
        ) -> Result<BoxFuture<'_, HttpsSample>, HttpsDateError> {
            self.as_ref().produce_sample(num_polls).await
        }
    }

    impl FakeSampler {
        /// Create a test client and a future that resolves when all the contents
        /// of |responses| have been consumed.
        pub fn with_responses(
            responses: impl IntoIterator<Item = Result<HttpsSample, HttpsDateError>>,
        ) -> (Self, impl Future) {
            let (sender, receiver) = oneshot::channel();
            let client = FakeSampler {
                enqueued_responses: Mutex::new(VecDeque::from_iter(responses)),
                completion_notifier: Mutex::new(Some(sender)),
                received_request_num_polls: Mutex::new(vec![]),
            };
            (client, receiver)
        }

        /// Assert that calls to produce_sample were made with the expected num_polls arguments.
        pub async fn assert_produce_sample_requests(&self, expected: &[usize]) {
            assert_eq!(self.received_request_num_polls.lock().await.as_slice(), expected);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::TryFutureExt;
    use std::collections::{HashMap, VecDeque};
    use std::sync::LazyLock;

    static TEST_URI: LazyLock<hyper::Uri> = LazyLock::new(|| "https://localhost/".parse().unwrap());

    const ONE_SECOND: zx::BootDuration = zx::BootDuration::from_seconds(1);
    const RTT_TIMES_ZERO_LATENCY: [zx::BootDuration; 2] =
        [zx::BootDuration::from_nanos(0), zx::BootDuration::from_nanos(0)];
    const RTT_TIMES_100_MS_LATENCY: [zx::BootDuration; 4] = [
        zx::BootDuration::from_millis(500), // ignored
        zx::BootDuration::from_millis(50),
        zx::BootDuration::from_millis(100),
        zx::BootDuration::from_millis(150),
    ];
    const DURATION_50_MS: zx::BootDuration = zx::BootDuration::from_millis(50);

    const TEST_UTC_OFFSET: zx::BootDuration = zx::BootDuration::from_hours(72);

    /// An |HttpsDateClient| which responds with fake (quantized) UTC times at specified offsets
    /// from the monotonic time.
    struct TestClient {
        enqueued_offsets: VecDeque<Result<zx::BootDuration, HttpsDateError>>,
    }

    fn make_test_config() -> Config {
        Config {
            use_fake_sampler: false,
            https_timeout: zx::BootDuration::from_seconds(10),
            standard_deviation_bound_percentage: 30,
            first_rtt_time_factor: 5,
            use_pull_api: false,
            sample_config_by_urgency: HashMap::new(),
        }
    }

    #[async_trait]
    impl HttpsDateClient for TestClient {
        async fn request_utc(
            &mut self,
            _uri: &Uri,
            _https_timeout: zx::BootDuration,
        ) -> Result<UtcInstant, HttpsDateError> {
            let offset = self.enqueued_offsets.pop_front().unwrap()?;
            // This is intentionally a UTC timestamp manufactured from boot instant
            // and offset.  This is only correct in tests.
            let unquantized_time =
                UtcInstant::from_nanos((zx::BootInstant::get() + offset).into_nanos());
            Ok(unquantized_time - time_subs(unquantized_time))
        }
    }

    impl TestClient {
        /// Create a test client that calculates responses with the provided offsets.
        fn with_offset_responses(
            offsets: impl IntoIterator<Item = Result<zx::BootDuration, HttpsDateError>>,
        ) -> Self {
            TestClient { enqueued_offsets: VecDeque::from_iter(offsets) }
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_produce_sample_one_poll() {
        let config = make_test_config();
        let standard_deviation_bound_percentage = config.standard_deviation_bound_percentage;
        let sampler = HttpsSamplerImpl::new_with_client(
            TEST_URI.clone(),
            TestClient::with_offset_responses(vec![Ok(TEST_UTC_OFFSET)]),
            &config,
        );
        let reference_before = zx::BootInstant::get();
        let sample = sampler.produce_sample(1).await.unwrap().await;
        let reference_after = zx::BootInstant::get();

        assert!(
            sample.utc.into_nanos()
                >= (reference_before + TEST_UTC_OFFSET - ONE_SECOND).into_nanos()
        );
        assert!(
            sample.utc.into_nanos()
                <= (reference_after + TEST_UTC_OFFSET + ONE_SECOND).into_nanos()
        );

        assert!(sample.reference >= reference_before);
        assert!(sample.reference <= reference_after);
        assert_eq!(
            sample.standard_deviation,
            sample.final_bound_size * standard_deviation_bound_percentage / 100
        );
        assert!(
            sample.final_bound_size
                <= (reference_after - reference_before + ONE_SECOND).to_utc_lossy()
        );
        assert_eq!(sample.polls.len(), 1);
        assert!(sample.polls[0].round_trip_time <= reference_after - reference_before);
    }

    #[fuchsia::test]
    async fn test_produce_sample_multiple_polls() {
        let config = make_test_config();
        let standard_deviation_bound_percentage = config.standard_deviation_bound_percentage;
        let sampler = HttpsSamplerImpl::new_with_client(
            TEST_URI.clone(),
            TestClient::with_offset_responses(vec![
                Ok(TEST_UTC_OFFSET),
                Ok(TEST_UTC_OFFSET),
                Ok(TEST_UTC_OFFSET),
            ]),
            &config,
        );
        let reference_before = zx::BootInstant::get();
        let sample = sampler.produce_sample(3).await.unwrap().await;
        let reference_after = zx::BootInstant::get();

        assert!(
            sample.utc.into_nanos()
                >= (reference_before + TEST_UTC_OFFSET - ONE_SECOND).into_nanos()
        );
        assert!(
            sample.utc.into_nanos()
                <= (reference_after + TEST_UTC_OFFSET + ONE_SECOND).into_nanos()
        );

        assert!(sample.reference.into_nanos() >= reference_before.into_nanos());
        assert!(sample.reference.into_nanos() <= reference_after.into_nanos());
        assert_eq!(
            sample.standard_deviation,
            sample.final_bound_size * standard_deviation_bound_percentage / 100
        );
        assert!(
            sample.final_bound_size.into_nanos()
                <= (reference_after - reference_before + ONE_SECOND).into_nanos()
        );
        assert_eq!(sample.polls.len(), 3);
        assert!(
            sample
                .polls
                .iter()
                .all(|poll| poll.round_trip_time <= reference_after - reference_before)
        );
    }

    #[fuchsia::test]
    async fn test_produce_sample_fails_if_initial_poll_fails() {
        let config = &make_test_config();
        let sampler = HttpsSamplerImpl::new_with_client(
            TEST_URI.clone(),
            TestClient::with_offset_responses(vec![Err(HttpsDateError::new(
                HttpsDateErrorType::NetworkError,
            ))]),
            config,
        );

        match sampler.produce_sample(3).await {
            Ok(_) => panic!("Expected error but received Ok"),
            Err(e) => assert_eq!(e.error_type(), HttpsDateErrorType::NetworkError),
        };
    }

    #[fuchsia::test]
    async fn test_produce_sample_succeeds_if_subsequent_poll_fails() {
        let config = &make_test_config();
        let sampler = HttpsSamplerImpl::new_with_client(
            TEST_URI.clone(),
            TestClient::with_offset_responses(vec![
                Ok(TEST_UTC_OFFSET),
                Ok(TEST_UTC_OFFSET),
                Err(HttpsDateError::new(HttpsDateErrorType::NetworkError)),
            ]),
            config,
        );

        let sample = sampler.produce_sample(3).await.unwrap().await;
        assert_eq!(sample.polls.len(), 2);
    }

    #[fuchsia::test]
    async fn test_produce_sample_takes_later_poll_if_polls_disagree() {
        let expected_offset = TEST_UTC_OFFSET + zx::BootDuration::from_hours(1);
        let config = &make_test_config();
        let sampler = HttpsSamplerImpl::new_with_client(
            TEST_URI.clone(),
            TestClient::with_offset_responses(vec![
                Ok(TEST_UTC_OFFSET),
                Ok(TEST_UTC_OFFSET),
                Ok(expected_offset),
            ]),
            config,
        );

        let reference_before = zx::BootInstant::get();
        let sample = sampler.produce_sample(3).await.unwrap().await;
        let reference_after = zx::BootInstant::get();

        assert_eq!(sample.polls.len(), 1);

        // These comparisons are not strictly correct, since they assume a specific
        // offset relationship between UTC and boot time.
        assert!(
            sample.utc.into_nanos()
                >= (reference_before + expected_offset - ONE_SECOND).into_nanos()
        );
        assert!(
            sample.utc.into_nanos()
                <= (reference_after + expected_offset + ONE_SECOND).into_nanos()
        );
    }

    #[fuchsia::test]
    fn test_ideal_poll_time_in_future() {
        let future_monotonic = zx::BootInstant::get() + zx::BootDuration::from_hours(100);
        let first_rtt_time_factor = make_test_config().first_rtt_time_factor;
        let bound_1 = Bound {
            reference: future_monotonic,
            utc_min: UtcInstant::from_nanos(3_000_000_000),
            utc_max: UtcInstant::from_nanos(4_000_000_000),
        };
        assert_eq!(
            ideal_next_poll_time(&bound_1, RTT_TIMES_ZERO_LATENCY.iter(), first_rtt_time_factor),
            future_monotonic + zx::BootDuration::from_millis(500),
        );
        assert_eq!(
            ideal_next_poll_time(&bound_1, RTT_TIMES_100_MS_LATENCY.iter(), first_rtt_time_factor),
            future_monotonic + zx::BootDuration::from_millis(500) - DURATION_50_MS,
        );

        let bound_2 = Bound {
            reference: future_monotonic,
            utc_min: UtcInstant::from_nanos(3_600_000_000),
            utc_max: UtcInstant::from_nanos(3_800_000_000),
        };
        assert_eq!(
            ideal_next_poll_time(&bound_2, RTT_TIMES_ZERO_LATENCY.iter(), first_rtt_time_factor),
            future_monotonic + zx::BootDuration::from_millis(300),
        );
        assert_eq!(
            ideal_next_poll_time(&bound_2, RTT_TIMES_100_MS_LATENCY.iter(), first_rtt_time_factor),
            future_monotonic + zx::BootDuration::from_millis(300) - DURATION_50_MS,
        );

        let bound_3 = Bound {
            reference: future_monotonic,
            utc_min: UtcInstant::from_nanos(0_500_000_000),
            utc_max: UtcInstant::from_nanos(2_500_000_000),
        };
        assert_eq!(
            ideal_next_poll_time(&bound_3, RTT_TIMES_ZERO_LATENCY.iter(), first_rtt_time_factor),
            future_monotonic + zx::BootDuration::from_millis(500),
        );
        assert_eq!(
            ideal_next_poll_time(&bound_3, RTT_TIMES_100_MS_LATENCY.iter(), first_rtt_time_factor),
            future_monotonic + zx::BootDuration::from_millis(500) - DURATION_50_MS,
        );
    }

    #[fuchsia::test]
    fn test_ideal_poll_time_in_past() {
        let monotonic_now = zx::BootInstant::get();
        let past_monotonic = zx::BootInstant::from_nanos(0);
        let first_rtt_time_factor = make_test_config().first_rtt_time_factor;
        let bound = Bound {
            reference: past_monotonic,
            utc_min: UtcInstant::from_nanos(3_000_000_000),
            utc_max: UtcInstant::from_nanos(4_000_000_000),
        };
        // The returned time should be in the future, but the subsecond component should match
        // the otherwise ideal time in the past.
        assert!(
            ideal_next_poll_time(&bound, RTT_TIMES_ZERO_LATENCY.iter(), first_rtt_time_factor)
                > monotonic_now
        );
        assert_eq!(
            time_subs(ideal_next_poll_time(
                &bound,
                RTT_TIMES_ZERO_LATENCY.iter(),
                first_rtt_time_factor
            )),
            time_subs(past_monotonic + zx::BootDuration::from_millis(500)),
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_fake_sampler() {
        let expected_responses = vec![
            Ok(HttpsSample {
                utc: UtcInstant::from_nanos(999),
                reference: zx::BootInstant::from_nanos(888_888_888),
                standard_deviation: UtcDuration::from_nanos(22),
                final_bound_size: UtcDuration::from_nanos(44),
                polls: vec![Poll { round_trip_time: zx::BootDuration::from_nanos(55) }],
            }),
            Err(HttpsDateErrorType::NetworkError),
            Err(HttpsDateErrorType::NoCertificatesPresented),
        ];
        let (fake_sampler, complete_fut) = FakeSampler::with_responses(
            expected_responses
                .iter()
                .cloned()
                .map(|response| response.map_err(HttpsDateError::new))
                .collect::<Vec<_>>(),
        );
        for expected in expected_responses {
            assert_eq!(
                expected,
                fake_sampler
                    .produce_sample(1)
                    .and_then(|sample_fut| async move { Ok(sample_fut.await) })
                    .await
                    .map_err(|e| e.error_type())
            );
        }

        // After exhausting canned responses, the sampler should stall.
        assert!(fake_sampler.produce_sample(1).now_or_never().is_none());
        // Completion is signalled via the future provided at construction.
        complete_fut.await;
    }
}
