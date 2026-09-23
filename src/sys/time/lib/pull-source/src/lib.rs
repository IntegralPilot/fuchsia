// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The `pull_source` library defines an implementation of the `PullSource` API and traits to hook
//! in an algorithm that produces time updates.

use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_time_external::{
    self as ftexternal, Properties, PullSourceRequest, PullSourceRequestStream, TimeSample, Urgency,
};

use futures::TryStreamExt;
use futures::lock::Mutex;
use log::warn;
use std::sync::Arc;

/// An |UpdateAlgorithm| trait produces time samples on demand.
#[async_trait]
pub trait UpdateAlgorithm {
    /// Update the algorithm's knowledge of device properties.
    async fn update_device_properties(&self, properties: Properties);

    /// Produce a new time sample, taking into account `Urgency`.
    async fn sample(&self, urgency: Urgency) -> Result<TimeSample, SampleError>;

    /// Returns the reference time at which the next sample may be produced.
    ///
    /// A reference timeline is always given on the boot timeline, which means
    /// it could fall in a time when the device was suspended.  We may want
    /// to wake the device to sample time, but also may decide not to, depending
    /// on power policy.
    async fn next_possible_sample_time(&self) -> zx::BootInstant;
}

/// Reasons `sample()` may fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleError {
    /// An error occurred that cannot be classified as one of the more specific
    /// error statuses.
    Unknown,
    /// An internal error occurred. This usually indicates a bug in the
    /// component implementation.
    Internal,
    /// A local resource error occurred such as IO, FIDL, or memory allocation
    /// failure.
    Resource,
    /// A network error occurred.
    Network,
    /// Some hardware that the time source depends on failed.
    Hardware,
    /// A retriable error specific to the implemented time protocol occurred,
    /// such as a malformed response from a remote server.
    Protocol,
    /// Sampling failed in a nonretriable way. Examples include failed
    /// authentication, or a missing configuration.
    ProtocolUnrecoverable,
    /// The request was made too soon and the client should wait before making
    /// another request.
    RateLimited,
}

impl From<SampleError> for ftexternal::Error {
    fn from(e: SampleError) -> Self {
        match e {
            SampleError::Unknown => ftexternal::Error::Unknown,
            SampleError::Internal => ftexternal::Error::Internal,
            SampleError::Resource => ftexternal::Error::Resource,
            SampleError::Network => ftexternal::Error::Network,
            SampleError::Hardware => ftexternal::Error::Hardware,
            SampleError::Protocol => ftexternal::Error::Protocol,
            SampleError::ProtocolUnrecoverable => ftexternal::Error::ProtocolUnrecoverable,
            SampleError::RateLimited => ftexternal::Error::RateLimited,
        }
    }
}

/// An implementation of |fuchsia.time.external.PullSource| that routes time updates from an
/// |UpdateAlgorithm| to clients of the fidl protocol and routes device property updates from fidl
/// clients to the |UpdateAlgorithm|.
/// This implementation is based on assumption that there's only one client.
pub struct PullSource<UA: UpdateAlgorithm> {
    /// The algorithm used to obtain new updates.
    update_algorithm: UA,
}

impl<UA: UpdateAlgorithm> PullSource<UA> {
    /// Create a new |PullSource| that polls |update_algorithm| for time updates and starts in the
    /// |initial_status| status.
    pub fn new(update_algorithm: UA) -> Result<Self, Error> {
        Ok(Self { update_algorithm })
    }

    /// Handle a single client's requests received on the given |request_stream|.
    pub async fn handle_requests_for_stream(
        &self,
        mut request_stream: PullSourceRequestStream,
        inspect_controller: Arc<Mutex<Option<inspect_runtime::PublishedInspectController>>>,
    ) -> Result<(), Error> {
        while let Some(request) = request_stream.try_next().await? {
            match request {
                PullSourceRequest::Sample { urgency, responder } => {
                    let sample = self.update_algorithm.sample(urgency).await;
                    responder.send(sample.as_ref().map_err(|e| (*e).into()))?;
                }
                PullSourceRequest::NextPossibleSampleTime { responder, .. } => {
                    responder.send(
                        self.update_algorithm.next_possible_sample_time().await.into_nanos(),
                    )?;
                }
                PullSourceRequest::UpdateDeviceProperties { properties, .. } => {
                    self.update_algorithm.update_device_properties(properties).await;
                }
                PullSourceRequest::Shutdown { responder } => {
                    let mut controller_lock = inspect_controller.lock().await;
                    if let Some(controller) = controller_lock.take() {
                        let options = inspect_runtime::EscrowOptions::default();
                        match controller.escrow_frozen(options).await {
                            Ok(token) => {
                                if let Err(e) = responder.send(Ok(token)) {
                                    warn!("Failed to send shutdown response: {:?}", e);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to escrow frozen Inspect controller: {:?}", e);
                                if let Err(e) = responder
                                    .send(Err(ftexternal::EscrowError::EscrowInspectFailed))
                                {
                                    warn!("Failed to send shutdown error response: {:?}", e);
                                }
                            }
                        }
                    } else {
                        warn!("Inspect controller already taken or None during shutdown");
                        if let Err(e) =
                            responder.send(Err(ftexternal::EscrowError::EscrowNoInspectController))
                        {
                            warn!("Failed to send shutdown error response: {:?}", e);
                        }
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// An UpdateAlgorithm that is backed up by the samples, set up by a test.
/// This implementation allows other crates and integration tests to use an implementation of
/// `UpdateAlgorithm`.
pub struct TestUpdateAlgorithm {
    /// List of received device property updates
    device_property_updates: Mutex<Vec<Properties>>,

    /// Time Samples to be generated by `sample()`.
    samples: Mutex<Vec<(Urgency, Result<TimeSample, SampleError>)>>,
}

impl TestUpdateAlgorithm {
    /// Create a new instance of `TestUpdateAlgorithm` with empty collection of samples to be used.
    pub fn new() -> Self {
        let device_property_updates = Mutex::new(Vec::new());
        let samples = Mutex::new(Vec::new());
        TestUpdateAlgorithm { device_property_updates, samples }
    }
}

#[async_trait]
impl UpdateAlgorithm for TestUpdateAlgorithm {
    async fn update_device_properties(&self, properties: Properties) {
        self.device_property_updates.lock().await.push(properties);
    }

    async fn sample(&self, urgency: Urgency) -> Result<TimeSample, SampleError> {
        let mut samples = self.samples.lock().await;
        if samples.is_empty() {
            warn!("No test samples found.");
            return Err(SampleError::Internal);
        }
        let (expected_urgency, sample) = samples.remove(0);
        if urgency == expected_urgency {
            sample
        } else {
            warn!("Wrong urgency provided: expected {:?}, got {:?}.", expected_urgency, urgency);
            Err(SampleError::Internal)
        }
    }

    async fn next_possible_sample_time(&self) -> zx::BootInstant {
        zx::BootInstant::get()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_time_external::{PullSourceMarker, PullSourceProxy};
    use fuchsia_async as fasync;
    use std::sync::Arc;

    struct TestHarness {
        /// The `PullSource` under test.
        test_source: Arc<PullSource<TestUpdateAlgorithm>>,

        /// Task which handles requests from `PullSource` proxy client.
        _server: fasync::Task<Result<(), Error>>,
    }

    impl TestHarness {
        fn new() -> (Self, PullSourceProxy) {
            let update_algorithm = TestUpdateAlgorithm::new();
            let test_source = Arc::new(PullSource::new(update_algorithm).unwrap());
            let (proxy, stream) = create_proxy_and_stream::<PullSourceMarker>();
            let server = fasync::Task::spawn({
                let test_source = Arc::clone(&test_source);
                async move {
                    test_source.handle_requests_for_stream(stream, Arc::new(Mutex::new(None))).await
                }
            });
            (TestHarness { test_source, _server: server }, proxy)
        }

        async fn add_sample(&mut self, urgency: Urgency, sample: Result<TimeSample, SampleError>) {
            self.test_source.update_algorithm.samples.lock().await.push((urgency, sample));
        }

        async fn get_device_properties(&self) -> Vec<Properties> {
            self.test_source.update_algorithm.device_property_updates.lock().await.clone()
        }
    }

    #[fuchsia::test]
    async fn test_empty_harness() {
        let (_harness, client) = TestHarness::new();
        // Should generate an error here since there are no events set up.
        let result = client.sample(Urgency::Low).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Err(ftexternal::Error::Internal),);
    }

    #[fuchsia::test]
    async fn test_harness_expects_sample_urgency() {
        let (mut harness, client) = TestHarness::new();

        harness
            .add_sample(
                Urgency::Low,
                Ok(TimeSample {
                    reference: Some(zx::BootInstant::from_nanos(12)),
                    utc: Some(34),
                    standard_deviation: None,
                    ..Default::default()
                }),
            )
            .await;
        // Should generate an error here since there requested urgency doesn't match provided.
        let result = client.sample(Urgency::High).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Err(ftexternal::Error::Internal),);
    }

    #[fuchsia::test]
    async fn test_multiple_samples() {
        let (mut harness, client) = TestHarness::new();

        harness
            .add_sample(
                Urgency::Low,
                Ok(TimeSample {
                    reference: Some(zx::BootInstant::from_nanos(12)),
                    utc: Some(34),
                    standard_deviation: None,
                    ..Default::default()
                }),
            )
            .await;
        harness
            .add_sample(
                Urgency::High,
                Ok(TimeSample {
                    reference: Some(zx::BootInstant::from_nanos(56)),
                    utc: Some(78),
                    standard_deviation: None,
                    ..Default::default()
                }),
            )
            .await;

        assert_eq!(
            client.sample(Urgency::Low).await.unwrap().unwrap(),
            TimeSample {
                reference: Some(zx::BootInstant::from_nanos(12)),
                utc: Some(34),
                standard_deviation: None,
                ..Default::default()
            }
        );
        assert_eq!(
            client.sample(Urgency::High).await.unwrap().unwrap(),
            TimeSample {
                reference: Some(zx::BootInstant::from_nanos(56)),
                utc: Some(78),
                standard_deviation: None,
                ..Default::default()
            }
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_property_updates_sent_to_update_algorithm() {
        let (harness, client) = TestHarness::new();

        client.update_device_properties(&Properties::default()).unwrap();

        // Allow tasks to service the request before checking the properties.
        let _ = fasync::TestExecutor::poll_until_stalled(std::future::pending::<()>()).await;

        assert_eq!(harness.get_device_properties().await, vec![Properties::default()]);
    }
}
