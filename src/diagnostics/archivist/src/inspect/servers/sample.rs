// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::accessor::{BatchIterator, BatchRetrievalTimeout};
use crate::diagnostics::AccessorStats;
use crate::error::AccessorError;
use crate::inspect::repository::InspectRepository;
use crate::inspect::{PerformanceConfig, ReaderServer};
use crate::pipeline::StaticHierarchyAllowlist;
use diagnostics_data::InspectData;
use diagnostics_hierarchy::{SelectResult, filter_tree, select_from_hierarchy};
use fidl::endpoints::{ControlHandle, DiscoverableProtocolMarker, ServerEnd};
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_diagnostics::{
    BatchIteratorMarker, BatchIteratorRequestStream, ConfigurationError, DataType, Format,
    RuntimeError, SampleDatum, SampleMarker, SampleParameters, SampleReady, SampleRequest,
    SampleRequestStream, SampleSinkEvent, SampleSinkProxy, SampleSinkResult, SampleStrategy,
    Selector, SelectorArgument, StreamMode, StreamParameters,
};
use fuchsia_async::{Interval, Scope};
use fuchsia_inspect::Node;
use fuchsia_trace as ftrace;
use futures::channel::oneshot;
use futures::{StreamExt, TryStreamExt, select};
use log::{debug, warn};
use selectors::{FastError, SelectorExt, ValidateExt, parse_selector};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use thiserror::Error;
use zx::MonotonicDuration;

/// This is an unfiltered pipeline.
///
/// `fuchsia.diagnostics.Sample` should NOT be used for data exfiltration that does
/// not have some other privacy control. An example of something acceptable is cobalt.
/// An example of something that would violate privacy policy is feedback.
const UNFILTERED_ALLOWLIST: StaticHierarchyAllowlist = StaticHierarchyAllowlist::new_disabled();

#[derive(Debug, Error)]
enum Error {
    #[error("Data field on SampleParameters must be populated")]
    DataCannotBeEmpty,

    #[error("Selectors field on SampleParameters must be populated")]
    DatumMissingSelectors,

    #[error("Sampling interval must be configured")]
    DatumMissingInterval,

    #[error(transparent)]
    SelectorParsing(#[from] selectors::Error),

    #[error(transparent)]
    InvalidSelector(#[from] selectors::ValidationError),

    #[error("Sample period {actual:?} is less than the minimum required: {minimum:?}")]
    SamplePeriodTooSmall { minimum: MonotonicDuration, actual: MonotonicDuration },
}

impl From<Error> for ConfigurationError {
    fn from(error: Error) -> ConfigurationError {
        match error {
            Error::DataCannotBeEmpty
            | Error::DatumMissingSelectors
            | Error::DatumMissingInterval => Self::SampleParametersInvalid,
            Error::SelectorParsing(_) | Error::InvalidSelector(_) => Self::InvalidSelectors,
            Error::SamplePeriodTooSmall { .. } => Self::SamplePeriodTooSmall,
        }
    }
}

pub struct SampleServer {
    scope: Scope,
    repo: Arc<InspectRepository>,
    trace_id: ftrace::Id,
    minimum_sample_period: MonotonicDuration,
}

impl SampleServer {
    pub fn new(
        repo: Arc<InspectRepository>,
        minimum_sample_period: MonotonicDuration,
        scope: Scope,
    ) -> Self {
        Self { scope, trace_id: ftrace::Id::new(), repo, minimum_sample_period }
    }

    pub async fn install_and_serve(
        self: &Arc<Self>,
        accessors_dict_id: u64,
        id_gen: &sandbox::CapabilityIdGenerator,
        store: &mut fsandbox::CapabilityStoreProxy,
    ) {
        let (sample_receiver_client, sample_receiver_stream) =
            fidl::endpoints::create_request_stream::<fsandbox::ReceiverMarker>();
        let sample_connector_id = id_gen.next();
        // Unwraps: I think these unwraps are safe in the sense that being unable to install
        // accessors to this store at component startup is actually a big deal, and crashing
        // is appropriate.
        store.connector_create(sample_connector_id, sample_receiver_client).await.unwrap().unwrap();
        debug!("Added fuchsia.diagnostics.Sample to accessors dictionary.");

        // Unwraps: I think these unwraps are safe in the sense that being unable to install
        // accessors to this store at component startup is actually a big deal, and crashing
        // is appropriate.
        store
            .dictionary_insert(
                accessors_dict_id,
                &fsandbox::DictionaryItem {
                    key: SampleMarker::PROTOCOL_NAME.to_string(),
                    value: sample_connector_id,
                },
            )
            .await
            .unwrap()
            .unwrap();

        self.spawn_receiver_request_handler(sample_receiver_stream);
    }

    fn spawn_receiver_request_handler(
        self: &Arc<SampleServer>,
        mut receiver_stream: fsandbox::ReceiverRequestStream,
    ) {
        let this = Arc::clone(self);
        self.scope.spawn(async move {
            while let Some(request) = receiver_stream.try_next().await.unwrap() {
                match request {
                    fsandbox::ReceiverRequest::Receive { channel, control_handle: _ } => {
                        let server_end = ServerEnd::<SampleMarker>::new(channel);
                        let this_for_handler = Arc::clone(&this);
                        this.scope.spawn(
                            this_for_handler.sample_request_handler(server_end.into_stream()),
                        );
                    }
                    fsandbox::ReceiverRequest::_UnknownMethod { method_type, ordinal, .. } => {
                        warn!(method_type:?, ordinal; "Got unknown interaction on Receiver");
                    }
                }
            }
        });
    }

    async fn sample_request_handler(self: Arc<Self>, mut stream: SampleRequestStream) {
        let mut accumulator = SampleParameters { data: Some(vec![]), ..Default::default() };
        let mut accumulation_error = Result::<(), Error>::Ok(());
        while let Some(Ok(request)) = stream.next().await {
            match request {
                SampleRequest::Commit { sink, responder, .. } => {
                    if let Err(error) = accumulation_error {
                        warn!(error:?; "Configuration error in SampleRequest::Set");
                        let _ = responder.send(Err(ConfigurationError::from(error)));
                        accumulation_error = Ok(());
                        continue;
                    }
                    if let Err(error) = self.handle_set_request(accumulator, sink.into_proxy()) {
                        accumulator = SampleParameters { data: Some(vec![]), ..Default::default() };
                        accumulation_error = Ok(());

                        warn!(error:?; "Configuration error in SampleRequest::Commit");
                        let _ = responder.send(Err(ConfigurationError::from(error)));
                        continue;
                    }

                    accumulator = SampleParameters { data: Some(vec![]), ..Default::default() };
                    accumulation_error = Ok(());
                    let _ = responder.send(Ok(()));
                }
                SampleRequest::Set { sample_parameters, .. } => {
                    if accumulation_error.is_err() {
                        // this will return an error when `Commit` is called, so there
                        // is no point continuing to push data
                        continue;
                    }

                    let Some(new_data) = sample_parameters.data else {
                        accumulation_error = Err(Error::DataCannotBeEmpty);
                        continue;
                    };

                    let Some(ref mut acc) = accumulator.data.as_mut() else {
                        continue;
                    };

                    acc.extend(new_data);
                }

                SampleRequest::_UnknownMethod { ordinal, control_handle, method_type, .. } => {
                    warn!(ordinal, method_type:?; "Received unknown request for Sample");
                    control_handle.shutdown();
                }
            }
        }
    }

    fn handle_set_request(
        self: &Arc<Self>,
        sample_parameters: SampleParameters,
        sink: SampleSinkProxy,
    ) -> Result<(), Error> {
        validate_parameters(&sample_parameters)?;
        let data = sample_parameters
            .data
            .unwrap()
            .into_iter()
            .enumerate()
            .map(Datum::try_from)
            .collect::<Result<Vec<Datum>, Error>>()?;

        let sample_period = extract_sample_period(&data);
        if sample_period < self.minimum_sample_period {
            return Err(Error::SamplePeriodTooSmall {
                minimum: self.minimum_sample_period,
                actual: sample_period,
            });
        }

        let this = Arc::clone(self);
        self.scope.spawn(this.batch_processor(data, sample_period, sink));
        Ok(())
    }

    async fn read_inspect(
        self: &Arc<Self>,
        data: Vec<Selector>,
        config: PerformanceConfig,
    ) -> Vec<InspectData> {
        // TODO: b/448187491 - Set up real instrumentation.
        let stats = Arc::new(AccessorStats::new(Node::default()).new_inspect_batch_iterator());
        let data = Some(data);
        std::pin::pin!(ReaderServer::stream(
            self.repo.fetch_inspect_data(&data, UNFILTERED_ALLOWLIST),
            config,
            // `None` here means that all available data from the first parameter
            // gets streamed and collected. It gets trimmed down again based on the
            // cache later.
            None,
            stats,
            self.trace_id,
        ))
        .collect()
        .await
    }

    async fn batch_processor(
        self: Arc<Self>,
        data: Vec<Datum>,
        sample_period: MonotonicDuration,
        sink: SampleSinkProxy,
    ) {
        let mut ticks: i64 = 0;
        // This cache holds the most recently sampled value for data in `data` with
        // a SampleStrategy of OnDiff.
        let mut on_diff_cache: HashMap<DataId, SelectResult<'static, _>> = HashMap::new();
        let mut interval = Interval::new(sample_period).fuse();

        let mut client_events = sink.take_event_stream().fuse();
        let mut last_gasp: Option<(oneshot::Sender<_>, oneshot::Receiver<_>)> = None;

        loop {
            let current_event = sample_period * ticks;
            let batch: Vec<_> = data
                .iter()
                .filter(|datum| {
                    last_gasp.is_some() || (current_event.into_seconds() % datum.interval_secs) == 0
                })
                .collect();

            let (last_gasp_tx, last_gasp_rx) =
                last_gasp.take().map(|(tx, rx)| (Some(tx), Some(rx))).unwrap_or((None, None));

            if let Some(filtered_snapshot) = self.take_sample(batch, &mut on_diff_cache).await {
                let (client_end, request_stream) =
                    fidl::endpoints::create_request_stream::<BatchIteratorMarker>();
                if sink
                    .on_sample_readied(SampleSinkResult::Ready(SampleReady {
                        batch_iter: Some(client_end),
                        seconds_since_start: Some(current_event.into_seconds()),
                        ..Default::default()
                    }))
                    .is_err()
                {
                    // The client is gone, so we are done.
                    return;
                }

                let trace_id = self.trace_id;
                let sink = sink.clone();
                self.scope.spawn(async move {
                    if let Err(error) =
                        serve_iterator(filtered_snapshot, request_stream, trace_id).await
                    {
                        warn!(error:?; "Sample server error");
                        // I believe an error here means the control handle is a dud/shutdown.
                        // That could mean simply that the client doesn't care about hearing
                        // runtime errors, so we don't need to do anything special with the
                        // error from control_handle.
                        let _ = sink.on_sample_readied(SampleSinkResult::Error(
                            RuntimeError::BatchIteratorFailed,
                        ));
                    }

                    last_gasp_tx.map(|tx: oneshot::Sender<()>| tx.send(()));
                });
            }

            // Shutdown the server after a last gasp.
            if let Some(rx) = last_gasp_rx {
                let _ = rx.await;
                return;
            }

            select! {
                _ = interval.next() => {
                    ticks += 1;
                }
                client_event = client_events.next() => match client_event {
                    Some(Ok(SampleSinkEvent::OnNowOrNever { .. })) => {
                        last_gasp = Some(oneshot::channel());
                        continue;
                    }
                    Some(Err(err)) => warn!(
                        err:?;
                        "Received error on SampleSink event stream, ignoring"
                    ),
                    // `None` should imply that the client dropped the sample sink handle,
                    // which means we will shut down on the next iteration.
                    // An unknown interaction we can just ignore and continue servicing
                    // our `SampleSink`.
                    None | Some(Ok(SampleSinkEvent::_UnknownEvent { .. })) => continue,
                }
            }
        }
    }

    async fn take_sample(
        self: &Arc<Self>,
        batch: Vec<&Datum>,
        on_diff_cache: &mut HashMap<DataId, SelectResult<'static, String>>,
    ) -> Option<Vec<InspectData>> {
        if batch.is_empty() {
            return None;
        }

        let performance_config = PerformanceConfig::new(
            &StreamParameters {
                data_type: Some(DataType::Inspect),
                stream_mode: Some(StreamMode::Snapshot),
                format: Some(Format::Cbor),
                ..Default::default()
            },
            10,
            BatchRetrievalTimeout::from_seconds(10),
        )
        // Safety: this is not taking any input from clients, so it should always
        // be correct if it doesn't error in tests, which it does not.
        .unwrap();

        let selectors_to_read =
            batch.iter().map(|&datum| datum.selector.clone()).collect::<Vec<_>>();
        let snapshot = self.read_inspect(selectors_to_read.clone(), performance_config).await;
        let mut selectors_with_no_change = vec![];

        for inspect_data in &snapshot {
            let Some(ref current_hierarchy) = inspect_data.payload else {
                continue;
            };

            for datum in
                batch.iter().filter(|datum| matches!(datum.strategy, SampleStrategy::OnDiff))
            {
                if datum.selector.component_selector.as_ref().is_some_and(|m| {
                    !selectors::matches_selectors(&inspect_data.moniker, std::slice::from_ref(m))
                }) {
                    // NB: If the datum provides a moniker selector, ensure we
                    // only update the cache with inspect_data from that moniker.
                    continue;
                }

                let Ok(current) = select_from_hierarchy(current_hierarchy, &datum.selector) else {
                    continue;
                };

                match on_diff_cache.entry(datum.id) {
                    // indicates the value has not updated
                    Entry::Occupied(cached) if cached.get() == &current => {
                        selectors_with_no_change.push(&datum.selector);
                    }
                    // indicates the value has changed
                    Entry::Occupied(mut cached) => {
                        *cached.get_mut() = current.into_owned();
                    }
                    Entry::Vacant(new_data) => {
                        new_data.insert(current.into_owned());
                    }
                }
            }
        }

        let filtered_snapshot =
            filter_inspect_data(snapshot, &selectors_to_read, &selectors_with_no_change);

        if filtered_snapshot.is_empty() { None } else { Some(filtered_snapshot) }
    }
}

fn filter_inspect_data(
    raw: Vec<InspectData>,
    all_selectors: &[Selector],
    unchanged: &[&Selector],
) -> Vec<InspectData> {
    let mut filtered_snapshot = vec![];
    for mut inspect_data in raw {
        let Some(payload) = inspect_data.payload.take() else {
            continue;
        };

        let relevant_unchanged_selectors = inspect_data
            .moniker
            .match_against_selectors(unchanged.iter().copied())
            .filter_map(|r| if let Ok(s) = r { Some(s.tree_selector.as_ref()) } else { None })
            // unfortunate, but cloning the iterator and using `any` was not working
            .collect::<Vec<_>>();

        let mut selectors = inspect_data
            .moniker
            .match_against_selectors(all_selectors.iter())
            .filter_map(|r| {
                if let Ok(s) = r {
                    if relevant_unchanged_selectors.contains(&s.tree_selector.as_ref()) {
                        None
                    } else {
                        s.tree_selector.as_ref()
                    }
                } else {
                    None
                }
            })
            .peekable();

        if selectors.peek().is_none() {
            continue;
        }

        let Some(filtered) = filter_tree(payload, selectors) else {
            continue;
        };

        inspect_data.payload = Some(filtered);
        filtered_snapshot.push(inspect_data);
    }

    filtered_snapshot
}

async fn serve_iterator(
    hierarchy: Vec<InspectData>,
    stream: BatchIteratorRequestStream,
    trace_id: ftrace::Id,
) -> Result<(), AccessorError> {
    // TODO: b/448187491 - Set up real instrumentation.
    let stats = Arc::new(AccessorStats::new(Node::default()).new_inspect_batch_iterator());
    BatchIterator::new(
        futures::stream::iter(hierarchy),
        stream.peekable(),
        StreamMode::Snapshot,
        stats,
        None,
        trace_id,
        // This has to become JSON if we stabilize this protocol before CBOR is stabilized.
        // For now, Sample is only available at HEAD.
        Format::Cbor,
    )?
    .run()
    .await?;

    Ok(())
}

fn validate_parameters(params: &SampleParameters) -> Result<(), Error> {
    let data = params.data.as_ref().ok_or(Error::DataCannotBeEmpty)?;
    if data.is_empty() {
        return Err(Error::DataCannotBeEmpty);
    }
    for datum in data {
        if datum.selector.is_none() {
            return Err(Error::DatumMissingSelectors);
        }
        if datum.interval_secs.is_none() {
            return Err(Error::DatumMissingInterval);
        }
    }
    Ok(())
}

/// Returns the GCD of the intervals of the data item.
fn extract_sample_period(data: &[Datum]) -> MonotonicDuration {
    MonotonicDuration::from_seconds(
        // use the euclidean algorithm to compute GCD
        data.iter()
            .map(|d| d.interval_secs)
            .reduce(|mut a, mut b| {
                while b != 0 {
                    let tmp = b;
                    b = a % b;
                    a = tmp;
                }
                a
            })
            .expect("the laws of mathematics to hold (at least gcd of 1)"),
    )
}

type DataId = usize;

struct Datum {
    strategy: SampleStrategy,
    selector: Selector,
    interval_secs: i64,
    id: DataId,
}

impl TryFrom<(usize, SampleDatum)> for Datum {
    type Error = Error;

    fn try_from((id, data): (usize, SampleDatum)) -> Result<Datum, Error> {
        let selector = match data.selector {
            Some(SelectorArgument::StructuredSelector(s)) => s,
            Some(SelectorArgument::RawSelector(s)) => parse_selector::<FastError>(&s)?,
            None => unreachable!("selector validated before"),
            _ => unimplemented!("unknown selector variants not supported"),
        };

        selector.validate()?;

        Ok(Datum {
            interval_secs: data.interval_secs.expect("already validated"),
            strategy: data.strategy.unwrap_or(SampleStrategy::Always),
            selector,
            id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::*;
    use crate::inspect::container::InspectHandle;
    use crate::pipeline::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::*;
    use diagnostics_data::*;
    use diagnostics_reader::*;
    use fidl::endpoints::*;
    use fidl_fuchsia_diagnostics::*;
    use fidl_fuchsia_inspect::*;
    use fuchsia_async::*;
    use fuchsia_inspect::{Property, *};
    use futures::*;
    use inspect_runtime::service::*;
    use inspect_runtime::*;
    use selectors::*;
    use std::sync::LazyLock;
    use std::task::Poll;
    use zx;

    impl SampleServer {
        fn spawn(self: &Arc<Self>, stream: SampleRequestStream) {
            let this = Arc::clone(self);
            self.scope.spawn(async move {
                this.sample_request_handler(stream).await;
            });
        }
    }

    const TEST_URL: &str = "fuchsia-pkg://test";
    static MONIKER: LazyLock<ExtendedMoniker> =
        LazyLock::new(|| ExtendedMoniker::parse_str("./a/b/foo").unwrap());

    #[track_caller]
    fn permissive_pipeline(scope: &ScopeHandle) -> (Arc<Pipeline>, Arc<InspectRepository>) {
        let pipeline = Arc::new(Pipeline::for_test(Some(vec![
            selectors::parse_verbose("**:[...]*:*").unwrap(),
        ])));
        let inspect_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], scope.new_child()));

        (pipeline, inspect_repo)
    }

    fn spawn_test_inspector(scope: &ScopeHandle) -> (TreeProxy, Inspector) {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<TreeMarker>();
        let inspector = Inspector::default();
        inspector.root().record_int("hello", 0);
        spawn_tree_server_with_stream(
            inspector.clone(),
            TreeServerSendPreference::default(),
            stream,
            scope,
        );
        (proxy, inspector)
    }

    // recall that serialization happens here, so positive integers will flip to uint
    // values in the below tests
    async fn drain_batch(batch_iter: ClientEnd<BatchIteratorMarker>) -> Vec<InspectData> {
        drain_batch_iterator::<InspectData>(Arc::new(batch_iter.into_proxy()))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    async fn extract(
        sample_sink_server: &mut SampleSinkRequestStream,
    ) -> (ClientEnd<BatchIteratorMarker>, i64) {
        let Some(Ok(SampleSinkRequest::OnSampleReadied {
            event:
                SampleSinkResult::Ready(SampleReady {
                    batch_iter: Some(batch_iter),
                    seconds_since_start: Some(seconds_since_start),
                    ..
                }),
            ..
        })) = sample_sink_server.next().await
        else {
            panic!("Malformed SampleSinkResult")
        };

        (batch_iter, seconds_since_start)
    }

    #[fuchsia::test]
    async fn sample_server_bad_configuration() {
        let scope = Scope::new();
        let (_pipeline, inspect_repo) = permissive_pipeline(&scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, _) = spawn_test_inspector(&scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, _) = fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        sample_proxy
            .set(&SampleParameters {
                data: Some(vec![SampleDatum {
                    // missing selector
                    strategy: Some(SampleStrategy::OnDiff),
                    interval_secs: Some(300),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .unwrap();
        assert_matches!(
            sample_proxy.commit(sample_sink_client).await,
            Ok(Err(ConfigurationError::SampleParametersInvalid))
        );

        let (sample_sink_client, _) = fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        sample_proxy
            .set(&SampleParameters {
                data: Some(vec![SampleDatum {
                    selector: Some(SelectorArgument::RawSelector("::::".to_string())),
                    strategy: Some(SampleStrategy::OnDiff),
                    interval_secs: Some(300),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .unwrap();
        assert_matches!(
            sample_proxy.commit(sample_sink_client).await,
            Ok(Err(ConfigurationError::InvalidSelectors))
        );

        let (sample_sink_client, _) = fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        sample_proxy
            .set(&SampleParameters {
                data: Some(vec![
                    SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("*:*:root".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(60),
                        ..Default::default()
                    },
                    SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("*:*:root".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(61),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            })
            .unwrap();
        assert_matches!(
            sample_proxy.commit(sample_sink_client).await,
            Ok(Err(ConfigurationError::SamplePeriodTooSmall))
        );
    }

    #[fuchsia::test]
    fn sample_server_on_diff_with_changes_node_selector() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, inspector) = spawn_test_inspector(scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        let prop = inspector.root().create_int("foo", 0);

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })
                .unwrap();
            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].moniker, *MONIKER);
            assert_eq!(actual[0].metadata.component_url, TEST_URL);
            assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: {
                hello: 0u64,
                foo: 0u64,
            });

            prop.set(5);

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 300);

            let actual = drain_batch(batch_iter).await;

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].moniker, *MONIKER);
            assert_eq!(actual[0].metadata.component_url, TEST_URL);
            assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: {
                hello: 0u64,
                foo: 5u64,
            });
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut fut));
    }

    #[fuchsia::test]
    fn sample_server_on_diff_without_changes_node_selector() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, _) = spawn_test_inspector(scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })
                .unwrap();
            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                        .with_hierarchy(hierarchy! { root: { hello: 0u64 }})
                        .build()
                ]
            );

            // this should never complete
            let _ = sample_sink_server.next().await;
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(310)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut fut));
    }

    #[fuchsia::test]
    fn sample_server_mixed_strategies() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, inspector) = spawn_test_inspector(scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        let foo = inspector.root().create_int("foo", 0);
        let bar = inspector.root().create_string("bar", "baz");

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![
                        SampleDatum {
                            selector: Some(SelectorArgument::RawSelector(
                                "**:root:foo".to_string(),
                            )),
                            strategy: Some(SampleStrategy::Always),
                            interval_secs: Some(300),
                            ..Default::default()
                        },
                        SampleDatum {
                            selector: Some(SelectorArgument::RawSelector(
                                "**:root:bar".to_string(),
                            )),
                            strategy: Some(SampleStrategy::OnDiff),
                            interval_secs: Some(300),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                })
                .unwrap();
            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].moniker, *MONIKER);
            assert_eq!(actual[0].metadata.component_url, TEST_URL);
            assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: {
                foo: 0u64,
                bar: "baz",
            });

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 300);

            let actual = drain_batch(batch_iter).await;
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].moniker, *MONIKER);
            assert_eq!(actual[0].metadata.component_url, TEST_URL);
            assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: {
                foo: 0u64,
            });

            foo.set(10);

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 600);

            let actual = drain_batch(batch_iter).await;
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].moniker, *MONIKER);
            assert_eq!(actual[0].metadata.component_url, TEST_URL);
            assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: {
                foo: 10u64,
            });

            bar.set("foobarbaz");

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 900);

            let actual = drain_batch(batch_iter).await;
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].moniker, *MONIKER);
            assert_eq!(actual[0].metadata.component_url, TEST_URL);
            assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: {
                foo: 10u64,
                bar: "foobarbaz",
            });
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut fut));
    }

    #[fuchsia::test]
    fn sample_server_on_diff_with_changes() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, inspector) = spawn_test_inspector(scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        let prop = inspector.root().create_int("foo", 0);

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root:foo".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })
                .unwrap();
            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                        .with_hierarchy(hierarchy! { root: { foo: 0u64 }})
                        .build()
                ]
            );

            prop.set(5);

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 300);

            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                        .with_hierarchy(hierarchy! { root: { foo: 5u64 }})
                        .build()
                ]
            );
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut fut));
    }

    #[fuchsia::test]
    fn sample_server_on_diff_without_changes() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, _) = spawn_test_inspector(scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root:hello".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })
                .unwrap();
            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();
            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                        .with_hierarchy(hierarchy! { root: { hello: 0u64 }})
                        .build()
                ]
            );

            // this should never complete
            let _ = sample_sink_server.next().await;
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(310)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut fut));
    }

    // A regression test for b/503719785.
    //
    // Ensure that inspect data with the wrong moniker doesn't influence the
    // on_diff cache.
    #[fuchsia::test]
    fn sample_server_on_diff_with_results_from_multiple_monikers() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);

        // Create 3 inspectors. The last inspector is the one we'll actually
        // select against.
        struct InspectorState {
            inspector: Inspector,
            moniker: ExtendedMoniker,
            url: String,
        }
        let inspectors = (0..3)
            .map(|i| {
                let moniker = ExtendedMoniker::parse_str(format!("./a/b/foo{i}").as_str()).unwrap();
                let url = format!("fuchsia-package://test-url{i}");
                let identity = Arc::new(ComponentIdentity::new(moniker.clone(), url.as_str()));
                let (proxy, inspector) = spawn_test_inspector(scope);
                inspect_repo.add_inspect_handle(
                    Arc::clone(&identity),
                    InspectHandle::tree(proxy, Option::<String>::None),
                );
                InspectorState { inspector, moniker, url }
            })
            .collect::<Vec<_>>();

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        // Only set the expected property on the third inspector.
        let last_inspector = inspectors.last().unwrap();
        let prop = last_inspector.inspector.root().create_int("foo", 0);

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![SampleDatum {
                        selector: Some(SelectorArgument::RawSelector(format!(
                            "{}:root:foo",
                            last_inspector.moniker
                        ))),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })
                .unwrap();
            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(
                        last_inspector.moniker.clone(),
                        last_inspector.url.as_str(),
                        actual_ts
                    )
                    .with_hierarchy(hierarchy! { root: { foo: 0u64 }})
                    .build()
                ]
            );

            prop.set(5);

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 300);

            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(
                        last_inspector.moniker.clone(),
                        last_inspector.url.as_str(),
                        actual_ts
                    )
                    .with_hierarchy(hierarchy! { root: { foo: 5u64 }})
                    .build()
                ]
            );
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut fut));
    }

    #[fuchsia::test]
    async fn on_now_or_never() {
        let scope = Scope::new();
        let (_pipeline, inspect_repo) = permissive_pipeline(&scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, inspector) = spawn_test_inspector(&scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            // minimum duration should be ignored for OnNowOrNever
            MonotonicDuration::from_seconds(120),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        sample_proxy
            .set(&SampleParameters {
                data: Some(vec![
                    SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root:a".to_string())),
                        strategy: Some(SampleStrategy::Always),
                        interval_secs: Some(300),
                        ..Default::default()
                    },
                    SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root:b".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    },
                    SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root:c".to_string())),
                        strategy: Some(SampleStrategy::OnDiff),
                        interval_secs: Some(300),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            })
            .unwrap();

        sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

        // will never change, Always strategy
        inspector.root().record_int("a", 0);
        // will never change, OnDiff. Only observed once.
        inspector.root().record_int("b", 5);
        // will change, on diff strategy
        let c = inspector.root().create_int("c", 100);

        let mut sample_sink_server = sample_sink_server.into_stream();

        let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
        assert_eq!(runtime, 0);

        // recall that serialization happens here, so positive integers will flip to uint
        // values in the below tests
        let actual = drain_batch(batch_iter).await;

        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].moniker, *MONIKER);
        assert_eq!(actual[0].metadata.component_url, TEST_URL);
        assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: contains {
            a: 0u64,
            b: 5u64,
            c: 100u64,
        });

        // shouldn't be picked up for 300 more seconds, except by OnNowOrNever
        c.set(10);

        sample_sink_server.control_handle().send_on_now_or_never().unwrap();

        let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
        assert_eq!(runtime, 0);

        let actual = drain_batch(batch_iter).await;

        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].moniker, *MONIKER);
        assert_eq!(actual[0].metadata.component_url, TEST_URL);
        assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: contains {
            a: 0u64,
            c: 10u64,
        });
    }

    #[fuchsia::test]
    async fn sample_server_multiple_set() {
        let scope = Scope::new();
        let (_pipeline, inspect_repo) = permissive_pipeline(&scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, inspector) = spawn_test_inspector(&scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        sample_proxy
            .set(&SampleParameters {
                data: Some(vec![SampleDatum {
                    selector: Some(SelectorArgument::RawSelector("**:root:a".to_string())),
                    strategy: Some(SampleStrategy::Always),
                    interval_secs: Some(300),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .unwrap();

        sample_proxy
            .set(&SampleParameters {
                data: Some(vec![SampleDatum {
                    selector: Some(SelectorArgument::RawSelector("**:root:b".to_string())),
                    strategy: Some(SampleStrategy::Always),
                    interval_secs: Some(300),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .unwrap();

        sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

        inspector.root().record_int("a", 0);
        inspector.root().record_int("b", 1);

        let mut sample_sink_server = sample_sink_server.into_stream();

        let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
        assert_eq!(runtime, 0);

        // recall that serialization happens here, so positive integers will flip to uint
        // values in the below tests
        let actual = drain_batch(batch_iter).await;

        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].moniker, *MONIKER);
        assert_eq!(actual[0].metadata.component_url, TEST_URL);
        assert_data_tree!(actual[0].payload.as_ref().unwrap(), root: contains {
            a: 0u64,
            b: 1u64,
        });
    }

    #[fuchsia::test]
    fn sample_server_simple_always() {
        let mut exec = TestExecutor::new_with_fake_time();
        let scope = exec.global_scope();
        let (_pipeline, inspect_repo) = permissive_pipeline(scope);
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        let (proxy, _) = spawn_test_inspector(scope);
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));
        let (sample_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<SampleMarker>();
        sampler.spawn(stream);

        let (sample_sink_client, sample_sink_server) =
            fidl::endpoints::create_endpoints::<SampleSinkMarker>();

        let mut fut = async move {
            sample_proxy
                .set(&SampleParameters {
                    data: Some(vec![SampleDatum {
                        selector: Some(SelectorArgument::RawSelector("**:root:hello".to_string())),
                        strategy: Some(SampleStrategy::Always),
                        interval_secs: Some(300),
                        ..Default::default()
                    }]),
                    ..Default::default()
                })
                .unwrap();

            sample_proxy.commit(sample_sink_client).await.unwrap().unwrap();

            let mut sample_sink_server = sample_sink_server.into_stream();

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 0);

            // recall that serialization happens here, so positive integers will flip to uint
            // values in the below tests
            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                        .with_hierarchy(hierarchy! { root: { hello: 0u64 }})
                        .build()
                ]
            );

            let (batch_iter, runtime) = extract(&mut sample_sink_server).await;
            assert_eq!(runtime, 300);

            let actual = drain_batch(batch_iter).await;

            let actual_ts = actual[0].metadata.timestamp;
            assert_eq!(
                actual,
                vec![
                    InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                        .with_hierarchy(hierarchy! { root: { hello: 0u64 }})
                        .build()
                ]
            );
        }
        .boxed();

        let _ = exec.run_until_stalled(&mut fut);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(300)));
        exec.wake_expired_timers();
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut fut));
    }

    #[fuchsia::test]
    async fn read_inspect_test() {
        let scope = Scope::new();
        let (_pipeline, inspect_repo) = permissive_pipeline(&scope);

        let sampler = Arc::new(SampleServer::new(
            Arc::clone(&inspect_repo),
            MonotonicDuration::from_seconds(60),
            scope.new_child(),
        ));

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<TreeMarker>();
        let identity = Arc::new(ComponentIdentity::new(MONIKER.clone(), TEST_URL));
        inspect_repo.add_inspect_handle(
            Arc::clone(&identity),
            InspectHandle::tree(proxy, Option::<String>::None),
        );

        let inspector = Inspector::default();
        inspector.root().record_int("hello", 0);
        spawn_tree_server_with_stream(
            inspector,
            TreeServerSendPreference::default(),
            stream,
            &scope,
        );

        let config = PerformanceConfig {
            aggregated_content_limit_bytes: None,
            batch_timeout_sec: 100,
            maximum_concurrent_snapshots_per_reader: 100,
        };

        let actual = sampler
            .read_inspect(vec![selectors::parse_verbose("**:[...]*:*").unwrap()], config)
            .await;
        let actual_ts = actual[0].metadata.timestamp;
        assert_eq!(
            actual,
            vec![
                InspectDataBuilder::new(MONIKER.clone(), TEST_URL, actual_ts)
                    .with_hierarchy(hierarchy! { root: { hello: 0 }})
                    .build()
            ]
        );
    }

    #[fuchsia::test]
    fn test_sample_period() {
        let data = &[
            Datum {
                strategy: SampleStrategy::OnDiff,
                selector: parse_verbose("**:*:*").unwrap(),
                interval_secs: 300,
                id: 0,
            },
            Datum {
                strategy: SampleStrategy::OnDiff,
                selector: parse_verbose("**:*:*").unwrap(),
                interval_secs: 300,
                id: 1,
            },
        ];

        assert_eq!(extract_sample_period(data), MonotonicDuration::from_seconds(300));

        let data = &[
            Datum {
                strategy: SampleStrategy::OnDiff,
                selector: parse_verbose("**:*:*").unwrap(),
                interval_secs: 300,
                id: 0,
            },
            Datum {
                strategy: SampleStrategy::OnDiff,
                selector: parse_verbose("**:*:*").unwrap(),
                interval_secs: 50,
                id: 1,
            },
        ];

        assert_eq!(extract_sample_period(data), MonotonicDuration::from_seconds(50));

        let data = &[
            Datum {
                strategy: SampleStrategy::OnDiff,
                selector: parse_verbose("**:*:*").unwrap(),
                interval_secs: 300,
                id: 0,
            },
            Datum {
                strategy: SampleStrategy::OnDiff,
                selector: parse_verbose("**:*:*").unwrap(),
                interval_secs: 350,
                id: 1,
            },
        ];

        assert_eq!(extract_sample_period(data), MonotonicDuration::from_seconds(50));
    }
}
