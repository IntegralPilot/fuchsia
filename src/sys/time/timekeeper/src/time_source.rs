// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, Result, anyhow, format_err};
use async_trait::async_trait;
use fidl_fuchsia_component::{
    self as fcomponent, ChildIteratorMarker, CreateChildArgs, RealmMarker, RealmProxy,
};
use fidl_fuchsia_component_decl::{
    Child, ChildRef, CollectionRef, ConfigOverride, ConfigSingleValue, ConfigValue, StartupMode,
};
use fidl_fuchsia_io::DirectoryProxy;
use fidl_fuchsia_time_external::{
    self as ftexternal, PushSourceProxy, Status, TimeSample, Urgency,
};
use fuchsia_component::client;
use fuchsia_runtime::UtcInstant;
use fuchsia_sync::Mutex;
use futures::stream::Stream;
use futures::{FutureExt, TryFutureExt};
use inspect_runtime::EscrowToken;
use log::debug;
use std::fmt::Debug;
use std::sync::Arc;
use time_pretty::{format_common, format_duration, format_timer};

const TIMESOURCE_COLLECTION_NAME: &str = "timesource";

/// A time sample received from a source of time.
#[derive(PartialEq, Clone, Copy)]
pub struct Sample {
    /// The UTC time.
    pub utc: UtcInstant,
    /// The reference time at which the UTC was most valid.
    pub reference: zx::BootInstant,
    /// The standard deviation of the UTC error.
    pub std_dev: zx::BootDuration,
}

impl Debug for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sample")
            .field("utc", &format_common(self.utc.into_nanos()))
            .field("reference", &format_timer(self.reference))
            .field("std_dev", &format_duration(self.std_dev))
            .finish()
    }
}

impl TryFrom<TimeSample> for Sample {
    type Error = anyhow::Error;

    fn try_from(sample: TimeSample) -> Result<Self, Self::Error> {
        let TimeSample { utc, monotonic, standard_deviation, reference, .. } = sample;
        match (utc, reference, standard_deviation, monotonic) {
            (None, _, _, _) => Err(anyhow!("sample missing utc")),
            (Some(utc), None, Some(std_dev), Some(monotonic)) => Ok(Sample {
                utc: UtcInstant::from_nanos(utc),
                // Treat `monotonic` as a boot instant, for compatibility.
                // This should work since any platform versions that still
                // use `monotonic` will not know the difference.
                reference: zx::BootInstant::from_nanos(monotonic),
                std_dev: zx::BootDuration::from_nanos(std_dev),
            }),
            (_, None, _, None) => Err(anyhow!("sample missing reference: {:?}", sample)),
            (_, _, None, _) => Err(anyhow!("sample missing standard deviation")),
            (Some(utc), Some(reference), Some(std_dev), _) => Ok(Sample {
                utc: UtcInstant::from_nanos(utc),
                reference,
                std_dev: zx::BootDuration::from_nanos(std_dev),
            }),
        }
    }
}

#[cfg(test)]
impl Sample {
    /// Constructs a new `Sample`.
    pub fn new(utc: UtcInstant, reference: zx::BootInstant, std_dev: zx::BootDuration) -> Sample {
        Sample { utc, reference, std_dev }
    }
}

/// An event that may be observed from a source of time.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Event {
    /// The status of the time source changed.
    StatusChange {
        /// The current status of the time source.
        status: Status,
    },
    /// The time source produced a new time sample.
    Sample(Sample),
}

impl From<Sample> for Event {
    fn from(sample: Sample) -> Event {
        Event::Sample(sample)
    }
}

/// One of the timesource API implementations.
#[derive(Debug)]
#[allow(dead_code)]
pub enum TimeSource {
    Push(BoxedPushSource),
    Pull(BoxedPullSource),
}

pub type BoxedPushSourceEventStream =
    Box<dyn Stream<Item = Result<Event, Error>> + Unpin + Send + Sync>;

pub type BoxedPushSource = Box<dyn PushSource>;
pub type BoxedPullSource = Box<dyn PullSource>;

/// Provides abstraction over `fuchsia.time.external.PushSource`.
/// https://fuchsia.dev/fuchsia-src/concepts/kernel/time/utc/architecture?hl=en#timekeeper
#[async_trait]
pub trait PushSource: Send + Sync + Debug {
    /// Attempts to launch the time source and return a stream of its time output and status
    /// change events.
    async fn watch(&self) -> Result<BoxedPushSourceEventStream, Error>;
}

/// Provides abstraction over `fuchsia.time.external.PullSource`.
/// https://fuchsia.dev/fuchsia-src/concepts/kernel/time/utc/architecture?hl=en#timekeeper
#[async_trait]
pub trait PullSource: Send + Sync + Debug {
    /// Attempts to start the timesource component and request a time sample. Component is
    /// unloaded after sample is returned in order to free system resources.
    async fn sample(&self, _urgency: &Urgency) -> Result<Sample, Error>;
}

/// Starts the component that provides one of timesouce FIDL APIs.
#[derive(Debug)]
pub struct TimeSourceLauncher {
    component_url: String,
    name: String,
    is_monitor: bool,
}

enum DestroyChildError {
    NotFound,
    Internal(anyhow::Error),
}

impl From<DestroyChildError> for anyhow::Error {
    fn from(error: DestroyChildError) -> Self {
        match error {
            DestroyChildError::NotFound => anyhow!("Unable to destroy timesource: not found"),
            DestroyChildError::Internal(e) => e,
        }
    }
}

impl From<anyhow::Error> for DestroyChildError {
    fn from(value: anyhow::Error) -> Self {
        Self::Internal(value)
    }
}

impl TimeSourceLauncher {
    /// Creates new launcher.
    pub fn new<S>(component_url: S, name: S, is_monitor: bool) -> Self
    where
        S: Into<String>,
    {
        let component_url = component_url.into();
        let name = name.into();
        TimeSourceLauncher { component_url, name, is_monitor }
    }

    /// Launches the timesource.
    async fn launch(&self) -> Result<DirectoryProxy, Error> {
        let realm = client::connect_to_protocol::<RealmMarker>()
            .context("failed to connect to fuchsia.component.Realm")?;
        self.ensure_timesource_destroyed(&realm).await.or_else(|e| match e {
            // The intent is to remove the child if it exists, so disregard the related error.
            DestroyChildError::NotFound => Ok(()),
            DestroyChildError::Internal(e) => Err(e),
        })?;
        debug!("Launching TimeSource at {}", self.component_url);
        let (child_decl, create_child_args) = if self.is_monitor {
            (
                Child {
                    name: Some(self.name.clone()),
                    url: Some(self.component_url.clone()),
                    startup: Some(StartupMode::Lazy),
                    config_overrides: Some(vec![ConfigOverride {
                        key: Some("is_monitor_time_source".to_string()),
                        value: Some(ConfigValue::Single(ConfigSingleValue::Bool(true))),
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
                CreateChildArgs::default(),
            )
        } else {
            (
                Child {
                    name: Some(self.name.clone()),
                    url: Some(self.component_url.clone()),
                    startup: Some(StartupMode::Lazy),
                    ..Default::default()
                },
                CreateChildArgs::default(),
            )
        };
        let collection_ref = CollectionRef { name: String::from(TIMESOURCE_COLLECTION_NAME) };

        realm
            .create_child(&collection_ref, &child_decl, create_child_args)
            .await
            .context("realm.create_child failed")?
            .map_err(|e| anyhow!("failed to create child: {:?} for {}", e, self.component_url))?;

        Ok(client::open_childs_exposed_directory(
            &self.name,
            Some(String::from(TIMESOURCE_COLLECTION_NAME)),
        )
        .await
        .with_context(|| format!("failed to open exposed directory for {}", self.component_url))?)
    }

    /// Destroys previously launched timesource. Will generate an error if the child was not found.
    async fn destroy(&self) -> Result<(), Error> {
        let realm = client::connect_to_protocol::<RealmMarker>()
            .context("failed to connect to fuchsia.component.Realm")?;
        self.ensure_timesource_destroyed(&realm).await.map_err(Into::into)
    }

    /// Destroys previously launched timesource and returns `RealmProxy` used.
    async fn ensure_timesource_destroyed(
        &self,
        realm: &RealmProxy,
    ) -> Result<(), DestroyChildError> {
        debug!("Ensure TimeSource is not running: {}", self.component_url);
        // Destroy the previously launched timesource.
        let child_ref = ChildRef {
            name: self.name.clone(),
            collection: Some(String::from(TIMESOURCE_COLLECTION_NAME)),
        };

        // Realm::DestroyChild is not quite idempotent: attempting to destroy
        // a child that does not exist results in log spam. So we only actually
        // call DestroyChild if there was indication that such a child existed.
        // It is still possible that such a child stops existing between the
        // check and the call, but that should be a rare event.
        if self.has_child(realm, &child_ref).await? {
            realm
                .destroy_child(&child_ref)
                .await
                .map_err(|e| DestroyChildError::Internal(e.into()))?
                .or_else(|err: fcomponent::Error| match err {
                    fcomponent::Error::InstanceNotFound => Err(DestroyChildError::NotFound),
                    _ => Err(DestroyChildError::Internal(format_err!(
                        "Error destroying child {:?}",
                        err
                    ))),
                })
        } else {
            Ok(())
        }
    }

    // Returns true if the `realm` contains the child referenced by `child_ref`.
    async fn has_child(&self, realm: &RealmProxy, child_ref: &ChildRef) -> Result<bool> {
        let (iter_proxy, server_end) = fidl::endpoints::create_proxy::<ChildIteratorMarker>();
        let collection = CollectionRef { name: TIMESOURCE_COLLECTION_NAME.into() };
        let _response = realm.list_children(&collection, server_end).await?;
        loop {
            let children = iter_proxy.next().await?;
            if children.is_empty() {
                break;
            }
            if children.into_iter().find(|e| *e == *child_ref).is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Into<BoxedPushSource> for TimeSourceLauncher {
    fn into(self) -> BoxedPushSource {
        Box::new(PushSourceImpl { launcher: self })
    }
}
impl Into<BoxedPullSource> for TimeSourceLauncher {
    fn into(self) -> BoxedPullSource {
        Box::new(PullSourceImpl { launcher: self, tokens: Mutex::new(None) })
    }
}

/// Production implementation of the `PushSource` trait.
#[derive(Debug)]
pub struct PushSourceImpl {
    launcher: TimeSourceLauncher,
}

impl PushSourceImpl {
    /// Returns a stream of time output and status change events received using the supplied
    /// `PushSourceProxy`, retaining the optional `App` for the same lifetime.
    fn events_from_proxy(proxy: PushSourceProxy) -> BoxedPushSourceEventStream {
        let proxy = Arc::new(proxy);

        let status_stream = futures::stream::try_unfold(Arc::clone(&proxy), |proxy| {
            proxy
                .watch_status()
                .map_ok(move |status| Some((Event::StatusChange { status }, proxy)))
                .err_into()
        });

        let sample_stream = futures::stream::try_unfold(proxy, |proxy| {
            proxy.watch_sample().map(move |result| {
                result
                    .map_err(Into::into) // convert fidl error to anyhow.
                    .and_then(TryInto::try_into) // convert TimeSample to Sample.
                    .map(|sample| Some((Event::Sample(sample), proxy))) // wrap in tuple.
            })
        });

        Box::new(futures::stream::select(Box::pin(status_stream), Box::pin(sample_stream)))
    }
}

#[async_trait]
impl PushSource for PushSourceImpl {
    /// Attempts to connect to PushSource FIDL API to receive time samples and status updates.
    async fn watch(&self) -> Result<BoxedPushSourceEventStream, Error> {
        let directory = self.launcher.launch().await?;
        let proxy =
            client::connect_to_protocol_at_dir_root::<ftexternal::PushSourceMarker>(&directory)
                .context("failed to connect to the fuchsia.time.external.PushSource")?;

        Ok(PushSourceImpl::events_from_proxy(proxy))
    }
}

/// Production implementation of the `PullSource` trait.
#[allow(dead_code)]
#[derive(Debug)]
pub struct PullSourceImpl {
    launcher: TimeSourceLauncher,
    tokens: Mutex<Option<EscrowToken>>,
}

impl PullSourceImpl {
    async fn sample_from_dir(
        &self,
        directory: &DirectoryProxy,
        urgency: &Urgency,
    ) -> Result<Sample, Error> {
        let proxy =
            client::connect_to_protocol_at_dir_root::<ftexternal::PullSourceMarker>(directory)
                .context("failed to connect to the fuchsia.time.external.PullSource")?;
        proxy
            .sample(*urgency)
            .await?
            .map_err(|e| format_err!("Error obtaining time sample: {:?}", e))?
            .try_into()
    }

    async fn shutdown(&self, directory: &DirectoryProxy) -> Result<EscrowToken, Error> {
        let proxy =
            client::connect_to_protocol_at_dir_root::<ftexternal::PullSourceMarker>(directory)
                .context("failed to connect to the fuchsia.time.external.PullSource")?;
        proxy
            .shutdown()
            .await?
            .map_err(|e| format_err!("Error obtaining time escrow token: {:?}", e))
    }
}

#[async_trait]
impl PullSource for PullSourceImpl {
    /// Attempts to start the timesource component and request a time sample. Component is
    /// unloaded after sample is returned in order to free system resources.
    async fn sample(&self, urgency: &Urgency) -> Result<Sample, Error> {
        // Drop current token if we have one, so Archivist can clean up old data.
        let _ = self.tokens.lock().take();

        let directory = self.launcher.launch().await?;
        // Don't check for errors here to ensure `destroy()` is called.
        let sample = self.sample_from_dir(&directory, urgency).await;

        match self.shutdown(&directory).await {
            Ok(token) => {
                *self.tokens.lock() = Some(token);
            }
            Err(e) => {
                log::warn!("Failed to shutdown and escrow: {:?}", e);
            }
        }

        self.launcher.destroy().await?;
        sample
    }
}

#[cfg(test)]
use futures::{StreamExt, stream};

/// A time source that immediately produces a collections of events supplied at construction.
/// The time source may be launched multiple times and will return a different collection of events
/// on each launch. It will return pending after the last event in the last collection, and will
/// terminate the stream after the last event in all other collections. The time source will return
/// an error if asked to launch after the last collection of events has been returned.
#[cfg(test)]
pub struct FakePushTimeSource {
    /// The collections of events to return. The TimeSource will return pending after the last
    /// event in the last collection, and will terminate the stream after the last event in all
    /// other collections.
    collections: Mutex<Vec<Vec<Result<Event, Error>>>>,
}

#[cfg(test)]
impl From<FakePushTimeSource> for TimeSource {
    fn from(s: FakePushTimeSource) -> Self {
        TimeSource::Push(Box::new(s))
    }
}

#[cfg(test)]
impl FakePushTimeSource {
    /// Creates a new `FakePushTimeSource` that produces the supplied single collection of
    /// successful events.
    pub fn events(events: Vec<Event>) -> Self {
        FakePushTimeSource {
            collections: Mutex::new(vec![events.into_iter().map(|evt| Ok(evt)).collect()]),
        }
    }

    /// Creates a new `FakePushTimeSource` that produces the supplied collections of successful
    /// events.
    pub fn event_collections(event_collections: Vec<Vec<Event>>) -> Self {
        FakePushTimeSource {
            collections: Mutex::new(
                event_collections
                    .into_iter()
                    .map(|collection| collection.into_iter().map(|evt| Ok(evt)).collect())
                    .collect(),
            ),
        }
    }

    /// Creates a new `FakePushTimeSource` that produces the supplied collections of results.
    pub fn result_collections(result_collections: Vec<Vec<Result<Event, Error>>>) -> Self {
        FakePushTimeSource { collections: Mutex::new(result_collections) }
    }

    /// Creates a new `FakePushTimeSource` that always fails to launch.
    pub fn failing() -> Self {
        FakePushTimeSource { collections: Mutex::new(vec![]) }
    }
}

#[cfg(test)]
impl Debug for FakePushTimeSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FakePushTimeSource")
    }
}

#[cfg(test)]
#[async_trait]
impl PushSource for FakePushTimeSource {
    async fn watch(&self) -> Result<BoxedPushSourceEventStream, Error> {
        let mut lock = self.collections.lock();
        if lock.is_empty() {
            return Err(anyhow!("FakePushTimeSource sent all supplied event collections"));
        }
        let events = lock.remove(0);
        // Return a pending after the last event if this was the last collection.
        if lock.is_empty() {
            Ok(Box::new(Box::pin(stream::iter(events).chain(stream::pending()))))
        } else {
            Ok(Box::new(Box::pin(stream::iter(events))))
        }
    }
}

/// A time source that upon request produces an event from the collection supplied at construction.
/// The time source may be launched multiple times and will return an event from the collection
/// on each launch. It will return error after the last event. The time source will return an error
/// if asked to launch after the last event from the collection has been returned.
#[cfg(test)]
pub struct FakePullTimeSource {
    /// The collection of events to return. The TimeSource will return error after the last
    /// event in the collection.
    collection: Mutex<Vec<(Urgency, Result<Sample, Error>)>>,
}

#[cfg(test)]
impl From<FakePullTimeSource> for TimeSource {
    fn from(s: FakePullTimeSource) -> Self {
        TimeSource::Pull(Box::new(s))
    }
}

#[cfg(test)]
impl Debug for FakePullTimeSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FakePullTimeSource")
    }
}

#[cfg(test)]
impl FakePullTimeSource {
    /// Creates a new `FakePullTimeSource` that produces the supplied collection of successful
    /// samples.
    pub fn samples(events: Vec<(Urgency, Sample)>) -> Self {
        FakePullTimeSource {
            collection: Mutex::new(events.into_iter().map(|(u, s)| (u, Ok(s))).collect()),
        }
    }

    /// Creates a new `FakePullTimeSource` that produces the supplied collection of results.
    pub fn results(results: Vec<(Urgency, Result<Sample, Error>)>) -> Self {
        FakePullTimeSource { collection: Mutex::new(results) }
    }

    /// Creates a new `FakePullTimeSource` that always fails to launch.
    pub fn failing() -> Self {
        FakePullTimeSource { collection: Mutex::new(Vec::new()) }
    }
}

#[cfg(test)]
#[async_trait]
impl PullSource for FakePullTimeSource {
    async fn sample(&self, urgency: &Urgency) -> Result<Sample, Error> {
        let mut events = self.collection.lock();
        if events.is_empty() {
            return Err(anyhow!("FakePullTimeSource sent all supplied events."));
        }
        let (expected_urgency, sample) = events.remove(0);
        if urgency == &expected_urgency {
            sample
        } else {
            Err(anyhow!(
                "Wrong urgency provided: expected {:?}, got {:?}.",
                expected_urgency,
                urgency
            ))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::prelude::*;
    use fuchsia_async as fasync;
    use std::sync::LazyLock;

    const STATUS_1: Status = Status::Initializing;
    const SAMPLE_1_UTC_NANOS: i64 = 1234567;
    const SAMPLE_1_REF_NANOS: i64 = 222;
    const SAMPLE_1_STD_DEV_NANOS: i64 = 8888;

    static STATUS_EVENT_1: LazyLock<Event> =
        LazyLock::new(|| Event::StatusChange { status: STATUS_1 });
    static SAMPLE_1: LazyLock<Sample> = LazyLock::new(|| Sample {
        utc: UtcInstant::from_nanos(SAMPLE_1_UTC_NANOS),
        reference: zx::BootInstant::from_nanos(SAMPLE_1_REF_NANOS),
        std_dev: zx::BootDuration::from_nanos(SAMPLE_1_STD_DEV_NANOS),
    });
    static SAMPLE_EVENT_1: LazyLock<Event> = LazyLock::new(|| Event::from(*SAMPLE_1));
    static SAMPLE_2: LazyLock<Sample> = LazyLock::new(|| Sample {
        utc: UtcInstant::from_nanos(12345678),
        reference: zx::BootInstant::from_nanos(333),
        std_dev: zx::BootDuration::from_nanos(9999),
    });
    static SAMPLE_EVENT_2: LazyLock<Event> = LazyLock::new(|| Event::from(*SAMPLE_2));

    #[fuchsia::test(allow_stalls = false)]
    async fn single_event_set() -> Result<(), Error> {
        let fake =
            FakePushTimeSource::events(vec![*STATUS_EVENT_1, *SAMPLE_EVENT_1, *SAMPLE_EVENT_2]);
        let mut events = fake.watch().await.context("Fake should watch without error")?;
        assert_eq!(events.next().await.unwrap().unwrap(), *STATUS_EVENT_1);
        assert_eq!(events.next().await.unwrap().unwrap(), *SAMPLE_EVENT_1);
        assert_eq!(events.next().await.unwrap().unwrap(), *SAMPLE_EVENT_2);
        // Making another call should lead to a stall and hence panic. We don't test this to
        // avoid a degenerate test, but do in fake_no_events_then_pending.
        assert!(fake.watch().await.is_err());
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn double_event_set() -> Result<(), Error> {
        let fake = FakePushTimeSource::event_collections(vec![
            vec![*STATUS_EVENT_1, *SAMPLE_EVENT_1],
            vec![*SAMPLE_EVENT_2],
        ]);
        let mut events = fake.watch().await.context("Fake should watch without error")?;
        assert_eq!(events.next().await.unwrap().unwrap(), *STATUS_EVENT_1);
        assert_eq!(events.next().await.unwrap().unwrap(), *SAMPLE_EVENT_1);
        assert!(events.next().await.is_none());
        let mut events = fake.watch().await.context("Fake should watch without error")?;
        assert_eq!(events.next().await.unwrap().unwrap(), *SAMPLE_EVENT_2);
        // Making another call should lead to a stall and hence panic. We don't test this to
        // avoid a degenerate test, but do in fake_no_events_then_pending.
        assert!(fake.watch().await.is_err());
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    #[should_panic]
    async fn fake_no_events_then_pending() {
        let fake = FakePushTimeSource::events(vec![]);
        let mut events = fake.watch().await.unwrap();
        // Getting an event from the last collection should never complete, leading to a stall.
        let _event = events.next().await;
    }

    #[fuchsia::test]
    async fn fake_failing() -> Result<(), Error> {
        let fake = FakePushTimeSource::failing();
        let events = fake.watch().await.context("Fake should launch without error");
        assert!(events.is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn new_primary_time_source() {
        const COMPONENT_NAME: &str = "alfred";
        const COMPONENT_URL: &str = "pennyworth";
        let time_source = TimeSourceLauncher::new(COMPONENT_URL, COMPONENT_NAME, false);
        assert_eq!(time_source.component_url, COMPONENT_URL);
        assert_eq!(time_source.name, COMPONENT_NAME);
        assert_eq!(time_source.is_monitor, false);
    }

    #[fuchsia::test]
    async fn push_time_source_events() {
        let (proxy, mut requests) =
            fidl::endpoints::create_proxy_and_stream::<ftexternal::PushSourceMarker>();

        let _task = fasync::Task::spawn(async move {
            while let Some(Ok(request)) = requests.next().await {
                match request {
                    ftexternal::PushSourceRequest::WatchStatus { responder, .. } => {
                        responder.send(STATUS_1).unwrap();
                    }
                    ftexternal::PushSourceRequest::WatchSample { responder, .. } => {
                        let sample = ftexternal::TimeSample {
                            utc: Some(SAMPLE_1_UTC_NANOS),
                            reference: Some(zx::BootInstant::from_nanos(SAMPLE_1_REF_NANOS)),
                            standard_deviation: Some(SAMPLE_1_STD_DEV_NANOS),
                            ..Default::default()
                        };
                        responder.send(&sample).unwrap();
                    }
                    _ => {}
                };
            }
        });

        let mut events = PushSourceImpl::events_from_proxy(proxy);
        // We expect to receive both events but the ordering is not deterministic.
        let event1 = events.next().await.unwrap().unwrap();
        let event2 = events.next().await.unwrap().unwrap();
        match event1 {
            Event::StatusChange { status: _ } => {
                assert_eq!(event1, *STATUS_EVENT_1);
                assert_eq!(event2, *SAMPLE_EVENT_1);
            }
            Event::Sample(_) => {
                assert_eq!(event1, *SAMPLE_EVENT_1);
                assert_eq!(event2, *STATUS_EVENT_1);
            }
        }
    }

    #[fuchsia::test]
    async fn push_time_source_failure() {
        let (proxy, mut requests) =
            fidl::endpoints::create_proxy_and_stream::<ftexternal::PushSourceMarker>();

        let _task = fasync::Task::spawn(async move {
            while let Some(Ok(request)) = requests.next().await {
                // Close the channel on the first watch status request.
                match request {
                    ftexternal::PushSourceRequest::WatchStatus { responder, .. } => {
                        responder.control_handle().shutdown_with_epitaph(zx::Status::BAD_STATE);
                    }
                    _ => {}
                };
            }
        });

        let mut events = PushSourceImpl::events_from_proxy(proxy);
        assert!(events.next().await.unwrap().is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_pull() -> Result<(), Error> {
        let fake = FakePullTimeSource::samples(vec![
            (Urgency::Low, *SAMPLE_1),
            (Urgency::Medium, *SAMPLE_2),
        ]);
        let sample_1 = fake.sample(&Urgency::Low).await.context("sample with Urgency::Low")?;
        assert_eq!(sample_1, *SAMPLE_1);

        let sample_2 =
            fake.sample(&Urgency::Medium).await.context("sample with Urgency::Medium")?;
        assert_eq!(sample_2, *SAMPLE_2);

        assert!(fake.sample(&Urgency::Low).await.is_err());
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_pull_results() -> Result<(), Error> {
        let fake = FakePullTimeSource::results(vec![
            (Urgency::Low, Err(anyhow!("test error"))),
            (Urgency::Low, Ok(*SAMPLE_1)),
        ]);
        assert!(fake.sample(&Urgency::Low).await.is_err());
        let sample = fake.sample(&Urgency::Low).await.context("sample with Urgency::Low")?;
        assert_eq!(sample, *SAMPLE_1);
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_pull_unexpected_urgency() -> Result<(), Error> {
        let fake = FakePullTimeSource::samples(vec![(Urgency::Medium, *SAMPLE_1)]);
        assert!(fake.sample(&Urgency::Low).await.is_err());
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_pull_no_events() -> Result<(), Error> {
        let fake = FakePullTimeSource::samples(Vec::new());
        assert!(fake.sample(&Urgency::Low).await.is_err());
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn fake_pull_failing() -> Result<(), Error> {
        let fake = FakePullTimeSource::failing();
        assert!(fake.sample(&Urgency::Low).await.is_err());
        Ok(())
    }
}
