// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::constants::FORMATTED_CONTENT_CHUNK_SIZE_TARGET;
use crate::diagnostics::{BatchIteratorConnectionStats, TRACE_CATEGORY};
use crate::error::AccessorError;
use crate::formatter::{
    FormattedStream, FxtPacketSerializer, JsonPacketSerializer, SerializedVmo, new_batcher,
};
use crate::inspect::ReaderServer;
use crate::inspect::repository::InspectRepository;
use crate::logs::repository::LogsRepository;
use crate::logs::servers::LogStreamServer;
use crate::logs::shared_buffer::FilterCursor;
use crate::pipeline::Pipeline;
use diagnostics_data::{Data, DiagnosticsData, ExtendedMoniker, Metadata};
use fidl::endpoints::{ControlHandle, RequestStream};
use fidl_fuchsia_diagnostics::{
    ArchiveAccessorRequest, ArchiveAccessorRequestStream, BatchIteratorControlHandle,
    BatchIteratorRequest, BatchIteratorRequestStream, ClientSelectorConfiguration,
    ComponentSelector, DataType, Format, FormattedContent, PerformanceConfiguration, Selector,
    SelectorArgument, StreamMode, StreamParameters, StringSelector, TreeSelector,
    TreeSelectorUnknown,
};
use fidl_fuchsia_diagnostics_host as fhost;
use fidl_fuchsia_mem::Buffer;
use fuchsia_async as fasync;
use fuchsia_inspect::NumericProperty;
use fuchsia_sync::Mutex;
use fuchsia_trace as ftrace;
use futures::StreamExt;
use futures::future::{Either, select};
use futures::prelude::*;
use futures::stream::Peekable;
use log::warn;
use selectors::FastError;
use serde::Serialize;
use std::collections::HashMap;
use std::pin::{Pin, pin};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Copy, Clone)]
pub struct BatchRetrievalTimeout(i64);

impl BatchRetrievalTimeout {
    pub fn from_seconds(s: i64) -> Self {
        Self(s)
    }

    #[cfg(test)]
    pub fn max() -> Self {
        Self::from_seconds(-1)
    }

    pub fn seconds(&self) -> i64 {
        if self.0 > 0 { self.0 } else { i64::MAX }
    }
}

/// ArchiveAccessorServer represents an incoming connection from a client to an Archivist
/// instance, through which the client may make Reader requests to the various data
/// sources the Archivist offers.
pub struct ArchiveAccessorServer {
    inspect_repository: Arc<InspectRepository>,
    logs_repository: Arc<LogsRepository>,
    maximum_concurrent_snapshots_per_reader: u64,
    scope: fasync::Scope,
    default_batch_timeout_seconds: BatchRetrievalTimeout,
}

fn validate_and_parse_selectors(
    selector_args: Vec<SelectorArgument>,
) -> Result<Vec<Selector>, AccessorError> {
    let mut selectors = vec![];
    let mut errors = vec![];

    if selector_args.is_empty() {
        return Err(AccessorError::EmptySelectors);
    }

    for selector_arg in selector_args {
        match selectors::take_from_argument::<FastError>(selector_arg) {
            Ok(s) => selectors.push(s),
            Err(e) => errors.push(e),
        }
    }

    if !errors.is_empty() {
        warn!(errors:?; "Found errors in selector arguments");
    }

    Ok(selectors)
}

fn validate_and_parse_log_selectors(
    selector_args: Vec<SelectorArgument>,
) -> Result<Vec<ComponentSelector>, AccessorError> {
    // Only accept selectors of the type: `component:root` for logs for now.
    let selectors = validate_and_parse_selectors(selector_args)?;
    let mut component_selectors = Vec::with_capacity(selectors.len());
    for selector in selectors {
        // Unwrap safe: Previous validation discards any selector without a node.
        let tree_selector = selector.tree_selector.as_ref().unwrap();
        match tree_selector {
            TreeSelector::PropertySelector(_) => {
                return Err(AccessorError::InvalidLogSelector);
            }
            TreeSelector::SubtreeSelector(subtree_selector) => {
                if subtree_selector.node_path.len() != 1 {
                    return Err(AccessorError::InvalidLogSelector);
                }
                match &subtree_selector.node_path[0] {
                    StringSelector::ExactMatch(val) if val == "root" => {}
                    StringSelector::StringPattern(val) if val == "root" => {}
                    _ => {
                        return Err(AccessorError::InvalidLogSelector);
                    }
                }
            }
            TreeSelectorUnknown!() => {}
        }
        component_selectors.push(selector.component_selector.unwrap());
    }
    Ok(component_selectors)
}

impl ArchiveAccessorServer {
    /// Create a new accessor for interacting with the archivist's data. The pipeline
    /// parameter determines which static configurations scope/restrict the visibility of
    /// data accessed by readers spawned by this accessor.
    pub fn new(
        inspect_repository: Arc<InspectRepository>,
        logs_repository: Arc<LogsRepository>,
        maximum_concurrent_snapshots_per_reader: u64,
        default_batch_timeout_seconds: BatchRetrievalTimeout,
        scope: fasync::Scope,
    ) -> Self {
        ArchiveAccessorServer {
            inspect_repository,
            logs_repository,
            maximum_concurrent_snapshots_per_reader,
            scope,
            default_batch_timeout_seconds,
        }
    }

    async fn spawn<R: ArchiveAccessorWriter + Send>(
        pipeline: Arc<Pipeline>,
        inspect_repo: Arc<InspectRepository>,
        log_repo: Arc<LogsRepository>,
        requests: R,
        params: StreamParameters,
        maximum_concurrent_snapshots_per_reader: u64,
        default_batch_timeout_seconds: BatchRetrievalTimeout,
    ) -> Result<(), AccessorError> {
        let format = params.format.ok_or(AccessorError::MissingFormat)?;
        if !matches!(format, Format::Json | Format::Cbor | Format::LegacyFxt | Format::Fxt) {
            return Err(AccessorError::UnsupportedFormat);
        }
        let mode = params.stream_mode.ok_or(AccessorError::MissingMode)?;

        let performance_config: PerformanceConfig = PerformanceConfig::new(
            &params,
            maximum_concurrent_snapshots_per_reader,
            default_batch_timeout_seconds,
        )?;

        let trace_id = ftrace::Id::new();
        match params.data_type.ok_or(AccessorError::MissingDataType)? {
            DataType::Inspect => {
                let _trace_guard = ftrace::async_enter!(
                    trace_id,
                    TRACE_CATEGORY,
                    c"ArchiveAccessorServer::spawn",
                    "data_type" => "Inspect",
                    "trace_id" => u64::from(trace_id)
                );
                if !matches!(mode, StreamMode::Snapshot) {
                    return Err(AccessorError::UnsupportedMode);
                }
                let stats = Arc::new(pipeline.accessor_stats().new_inspect_batch_iterator());

                let selectors =
                    params.client_selector_configuration.ok_or(AccessorError::MissingSelectors)?;

                let selectors = match selectors {
                    ClientSelectorConfiguration::Selectors(selectors) => {
                        Some(validate_and_parse_selectors(selectors)?)
                    }
                    ClientSelectorConfiguration::SelectAll(_) => None,
                    _ => return Err(AccessorError::InvalidSelectors("unrecognized selectors")),
                };

                let static_hierarchy_allowlist = pipeline.static_hierarchy_allowlist();
                let unpopulated_container_vec =
                    inspect_repo.fetch_inspect_data(&selectors, static_hierarchy_allowlist);

                let per_component_budget_opt = if unpopulated_container_vec.is_empty() {
                    None
                } else {
                    performance_config
                        .aggregated_content_limit_bytes
                        .map(|limit| (limit as usize) / unpopulated_container_vec.len())
                };

                if let Some(max_snapshot_size) = performance_config.aggregated_content_limit_bytes {
                    stats.global_stats().record_max_snapshot_size_config(max_snapshot_size);
                }
                BatchIterator::new(
                    ReaderServer::stream(
                        unpopulated_container_vec,
                        performance_config,
                        selectors,
                        Arc::clone(&stats),
                        trace_id,
                    ),
                    requests,
                    mode,
                    stats,
                    per_component_budget_opt,
                    trace_id,
                    format,
                )?
                .run()
                .await
            }
            DataType::Logs => {
                if format == Format::Cbor {
                    // CBOR is not supported for logs
                    return Err(AccessorError::UnsupportedFormat);
                }
                let _trace_guard = ftrace::async_enter!(
                    trace_id,
                    TRACE_CATEGORY,
                    c"ArchiveAccessorServer::spawn",
                    "data_type" => "Logs",
                    // An async duration cannot have multiple concurrent child async durations
                    // so we include the nonce as metadata to manually determine relationship.
                    "trace_id" => u64::from(trace_id)
                );
                let stats = Arc::new(pipeline.accessor_stats().new_logs_batch_iterator());
                let selectors = match params.client_selector_configuration {
                    Some(ClientSelectorConfiguration::Selectors(selectors)) => {
                        validate_and_parse_log_selectors(selectors)?
                    }
                    Some(ClientSelectorConfiguration::SelectAll(_)) => Vec::new(),
                    _ => return Err(AccessorError::InvalidSelectors("unrecognized selectors")),
                };
                match format {
                    Format::LegacyFxt => {
                        let cursor = log_repo.logs_cursor_raw(mode, selectors);
                        BatchIterator::new_serving_fxt(
                            cursor,
                            requests,
                            mode,
                            stats,
                            trace_id,
                            performance_config,
                            false,
                        )?
                        .run()
                        .await?;
                        Ok(())
                    }
                    Format::Fxt => {
                        let cursor = log_repo.logs_cursor_raw(mode, selectors);
                        match requests.into_socket_or_channel() {
                            Either::Left(socket) => {
                                LogStreamServer::stream_logs_with_manifest(socket, cursor).await?;
                            }
                            Either::Right(channel) => {
                                BatchIterator::new_serving_fxt(
                                    cursor,
                                    channel,
                                    mode,
                                    stats,
                                    trace_id,
                                    performance_config,
                                    true,
                                )?
                                .run()
                                .await?;
                            }
                        }
                        Ok(())
                    }
                    Format::Json => {
                        let logs = Box::pin(log_repo.logs_cursor(mode, selectors));
                        BatchIterator::new_serving_arrays(logs, requests, mode, stats, trace_id)?
                            .run()
                            .await?;
                        Ok(())
                    }
                    // TODO(https://fxbug.dev/401548725): Remove this from the FIDL definition.
                    Format::Text => Err(AccessorError::UnsupportedFormat),
                    Format::Cbor => unreachable!("CBOR is not supported for logs"),
                }
            }
        }
    }

    /// Spawn an instance `fidl_fuchsia_diagnostics/Archive` that allows clients to open
    /// reader session to diagnostics data.
    pub fn spawn_server<RequestStream>(&self, pipeline: Arc<Pipeline>, mut stream: RequestStream)
    where
        RequestStream: ArchiveAccessorTranslator + Send + 'static,
        <RequestStream as ArchiveAccessorTranslator>::InnerDataRequestChannel:
            ArchiveAccessorWriter + Send,
    {
        // Self isn't guaranteed to live into the exception handling of the async block. We need to clone self
        // to have a version that can be referenced in the exception handling.
        let log_repo = Arc::clone(&self.logs_repository);
        let inspect_repo = Arc::clone(&self.inspect_repository);
        let maximum_concurrent_snapshots_per_reader = self.maximum_concurrent_snapshots_per_reader;
        let default_batch_timeout_seconds = self.default_batch_timeout_seconds;
        let scope = self.scope.to_handle();
        self.scope.spawn(async move {
            let stats = pipeline.accessor_stats();
            stats.global_stats.connections_opened.add(1);
            while let Some(request) = stream.next().await {
                let control_handle = request.iterator.get_control_handle();
                stats.global_stats.stream_diagnostics_requests.add(1);
                let pipeline = Arc::clone(&pipeline);

                // Store the batch iterator task so that we can ensure that the client finishes
                // draining items through it when a Controller#Stop call happens. For example,
                // this allows tests to fetch all isolated logs before finishing.
                let inspect_repo_for_task = Arc::clone(&inspect_repo);
                let log_repo_for_task = Arc::clone(&log_repo);
                scope.spawn(async move {
                    if let Err(e) = Self::spawn(
                        pipeline,
                        inspect_repo_for_task,
                        log_repo_for_task,
                        request.iterator,
                        request.parameters,
                        maximum_concurrent_snapshots_per_reader,
                        default_batch_timeout_seconds,
                    )
                    .await
                        && let Some(control) = control_handle
                    {
                        e.close(control);
                    }
                });
            }
            pipeline.accessor_stats().global_stats.connections_closed.add(1);
        });
    }
}

pub trait ArchiveAccessorWriter {
    /// Writes diagnostics data to the remote side.
    fn write(
        &mut self,
        results: Vec<FormattedContent>,
    ) -> impl Future<Output = Result<(), IteratorError>> + Send;

    /// Converts the writer into either a socket or the original writer.
    /// Returns `Left(socket)` if the writer can be converted to a socket,
    /// or `Right(writer)` if it cannot.
    fn into_socket_or_channel(self) -> Either<fuchsia_async::Socket, Self>
    where
        Self: Sized,
    {
        Either::Right(self)
    }

    /// Waits for a buffer to be available for writing into. For sockets, this is a no-op.
    fn wait_for_buffer(&mut self) -> impl Future<Output = anyhow::Result<()>> + Send {
        futures::future::ready(Ok(()))
    }

    /// Takes the control handle from the FIDL stream (or returns None
    /// if the handle has already been taken, or if this is a socket.
    fn get_control_handle(&self) -> Option<BatchIteratorControlHandle> {
        None
    }

    /// Sends an on ready event.
    fn maybe_respond_ready(&mut self) -> impl Future<Output = Result<(), AccessorError>> + Send {
        futures::future::ready(Ok(()))
    }

    /// Waits for ZX_ERR_PEER_CLOSED
    fn wait_for_close(&mut self) -> impl Future<Output = ()> + Send;
}

fn get_buffer_from_formatted_content(
    content: fidl_fuchsia_diagnostics::FormattedContent,
) -> Result<Buffer, AccessorError> {
    match content {
        FormattedContent::Json(json) => Ok(json),
        FormattedContent::Fxt(vmo) => {
            let size = vmo.get_content_size().map_err(AccessorError::VmoSize)?;
            Ok(fidl_fuchsia_mem::Buffer { vmo, size })
        }
        _ => Err(AccessorError::UnsupportedFormat),
    }
}

impl ArchiveAccessorWriter for fuchsia_async::Socket {
    async fn write(&mut self, data: Vec<FormattedContent>) -> Result<(), IteratorError> {
        if data.is_empty() {
            return Err(IteratorError::PeerClosed);
        }
        let mut buf = vec![0];
        for value in data {
            let data = get_buffer_from_formatted_content(value)?;
            buf.resize(data.size as usize, 0);
            data.vmo.read(&mut buf, 0)?;
            let res = self.write_all(&buf).await;
            if res.is_err() {
                // connection probably closed.
                break;
            }
        }
        Ok(())
    }

    fn into_socket_or_channel(self) -> Either<fuchsia_async::Socket, Self> {
        Either::Left(self)
    }

    async fn wait_for_close(&mut self) {
        let _ = self.on_closed().await;
    }
}

#[derive(Error, Debug)]
pub enum IteratorError {
    #[error("Peer closed")]
    PeerClosed,
    #[error(transparent)]
    Ipc(#[from] fidl::Error),
    #[error(transparent)]
    AccessorError(#[from] AccessorError),
    // This error should be unreachable. We should never
    // fail to read from a VMO that we created, but the type system
    // requires us to handle this.
    #[error("Error reading from VMO: {}", source)]
    VmoReadError {
        #[from]
        source: zx::Status,
    },
}

impl<T: ArchiveAccessorWriter, Y: ArchiveAccessorWriter> ArchiveAccessorWriter for Either<T, Y> {
    fn write(
        &mut self,
        results: Vec<FormattedContent>,
    ) -> impl Future<Output = Result<(), IteratorError>> + Send {
        match self {
            Either::Left(value) => Either::Left(value.write(results)),
            Either::Right(value) => Either::Right(value.write(results)),
        }
    }

    /// Converts the writer into either a socket or the original writer.
    /// Returns `Left(socket)` if the writer can be converted to a socket,
    /// or `Right(writer)` if it cannot.
    fn into_socket_or_channel(self) -> Either<fuchsia_async::Socket, Self>
    where
        Self: Sized,
    {
        match self {
            Either::Left(value) => match value.into_socket_or_channel() {
                Either::Left(socket) => Either::Left(socket),
                Either::Right(original) => Either::Right(Either::Left(original)),
            },
            Either::Right(value) => match value.into_socket_or_channel() {
                Either::Left(socket) => Either::Left(socket),
                Either::Right(original) => Either::Right(Either::Right(original)),
            },
        }
    }

    fn wait_for_buffer(&mut self) -> impl Future<Output = anyhow::Result<()>> + Send {
        match self {
            Either::Left(value) => Either::Left(value.wait_for_buffer()),
            Either::Right(value) => Either::Right(value.wait_for_buffer()),
        }
    }

    fn get_control_handle(&self) -> Option<BatchIteratorControlHandle> {
        match self {
            Either::Left(value) => value.get_control_handle(),
            Either::Right(value) => value.get_control_handle(),
        }
    }

    fn maybe_respond_ready(&mut self) -> impl Future<Output = Result<(), AccessorError>> + Send {
        match self {
            Either::Left(value) => Either::Left(value.maybe_respond_ready()),
            Either::Right(value) => Either::Right(value.maybe_respond_ready()),
        }
    }

    fn wait_for_close(&mut self) -> impl Future<Output = ()> + Send {
        match self {
            Either::Left(value) => Either::Left(value.wait_for_close()),
            Either::Right(value) => Either::Right(value.wait_for_close()),
        }
    }
}

impl ArchiveAccessorWriter for Peekable<BatchIteratorRequestStream> {
    async fn write(&mut self, data: Vec<FormattedContent>) -> Result<(), IteratorError> {
        loop {
            match self.next().await {
                Some(Ok(BatchIteratorRequest::GetNext { responder })) => {
                    responder.send(Ok(data))?;
                    return Ok(());
                }
                Some(Ok(BatchIteratorRequest::WaitForReady { responder })) => {
                    responder.send()?;
                }
                Some(Ok(BatchIteratorRequest::_UnknownMethod { method_type, ordinal, .. })) => {
                    warn!(method_type:?, ordinal; "Got unknown interaction on BatchIterator");
                    return Err(IteratorError::PeerClosed);
                }
                Some(Err(err)) => return Err(err.into()),
                None => {
                    return Err(IteratorError::PeerClosed);
                }
            }
        }
    }

    async fn maybe_respond_ready(&mut self) -> Result<(), AccessorError> {
        let mut this = Pin::new(self);
        if matches!(this.as_mut().peek().await, Some(Ok(BatchIteratorRequest::WaitForReady { .. })))
        {
            let Some(Ok(BatchIteratorRequest::WaitForReady { responder })) = this.next().await
            else {
                unreachable!("We already checked the next request was WaitForReady");
            };
            responder.send()?;
        }
        Ok(())
    }

    async fn wait_for_buffer(&mut self) -> anyhow::Result<()> {
        let this = Pin::new(self);
        match this.peek().await {
            Some(Ok(_)) => Ok(()),
            _ => Err(IteratorError::PeerClosed.into()),
        }
    }

    fn get_control_handle(&self) -> Option<BatchIteratorControlHandle> {
        Some(self.get_ref().control_handle())
    }

    async fn wait_for_close(&mut self) {
        let _ = self.get_ref().control_handle().on_closed().await;
    }
}

pub struct ArchiveIteratorRequest<R> {
    parameters: StreamParameters,
    iterator: R,
}

/// Translation trait used to support both remote and
/// local ArchiveAccessor implementations.
pub trait ArchiveAccessorTranslator {
    type InnerDataRequestChannel;
    fn next(
        &mut self,
    ) -> impl Future<Output = Option<ArchiveIteratorRequest<Self::InnerDataRequestChannel>>> + Send;
}

impl ArchiveAccessorTranslator for fhost::ArchiveAccessorRequestStream {
    type InnerDataRequestChannel = fuchsia_async::Socket;

    async fn next(&mut self) -> Option<ArchiveIteratorRequest<Self::InnerDataRequestChannel>> {
        match StreamExt::next(self).await {
            Some(Ok(fhost::ArchiveAccessorRequest::StreamDiagnostics {
                parameters,
                responder,
                stream,
            })) => {
                // It's fine for the client to send us a socket
                // and discard the channel without waiting for a response.
                // Future communication takes place over the socket so
                // the client may opt to use this as an optimization.
                let _ = responder.send();
                Some(ArchiveIteratorRequest {
                    iterator: fuchsia_async::Socket::from_socket(stream),
                    parameters,
                })
            }
            _ => None,
        }
    }
}

impl ArchiveAccessorTranslator for ArchiveAccessorRequestStream {
    type InnerDataRequestChannel =
        Either<Peekable<BatchIteratorRequestStream>, fuchsia_async::Socket>;

    async fn next(&mut self) -> Option<ArchiveIteratorRequest<Self::InnerDataRequestChannel>> {
        loop {
            match StreamExt::next(self).await {
                Some(Ok(ArchiveAccessorRequest::StreamDiagnostics {
                    control_handle: _,
                    result_stream,
                    stream_parameters,
                })) => {
                    return Some(ArchiveIteratorRequest {
                        iterator: Either::Left(result_stream.into_stream().peekable()),
                        parameters: stream_parameters,
                    });
                }
                Some(Ok(ArchiveAccessorRequest::StreamDiagnosticsToSocket {
                    stream,
                    stream_parameters,
                    responder,
                })) => {
                    // It's fine for the client to send us a socket
                    // and discard the channel without waiting for a response.
                    // Future communication takes place over the socket so
                    // the client may opt to use this as an optimization.
                    let _ = responder.send();
                    return Some(ArchiveIteratorRequest {
                        iterator: Either::Right(fuchsia_async::Socket::from_socket(stream)),
                        parameters: stream_parameters,
                    });
                }
                Some(Ok(ArchiveAccessorRequest::WaitForReady { responder })) => {
                    let _ = responder.send();
                }
                _ => return None,
            }
        }
    }
}
struct SchemaTruncationCounter {
    truncated_schemas: u64,
    total_schemas: u64,
}

impl SchemaTruncationCounter {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { truncated_schemas: 0, total_schemas: 0 }))
    }
}

pub(crate) struct BatchIterator<R> {
    requests: R,
    stats: Arc<BatchIteratorConnectionStats>,
    data: FormattedStream,
    truncation_counter: Option<Arc<Mutex<SchemaTruncationCounter>>>,
    parent_trace_id: ftrace::Id,
}

// Checks if a given schema is within a components budget, and if it is, updates the budget,
// then returns true. Otherwise, if the schema is not within budget, returns false.
fn maybe_update_budget(
    budget_map: &mut HashMap<ExtendedMoniker, usize>,
    moniker: &ExtendedMoniker,
    bytes: usize,
    byte_limit: usize,
) -> bool {
    if let Some(remaining_budget) = budget_map.get_mut(moniker) {
        if *remaining_budget + bytes > byte_limit {
            false
        } else {
            *remaining_budget += bytes;
            true
        }
    } else if bytes > byte_limit {
        budget_map.insert(moniker.clone(), 0);
        false
    } else {
        budget_map.insert(moniker.clone(), bytes);
        true
    }
}

impl<R> BatchIterator<R>
where
    R: ArchiveAccessorWriter + Send,
{
    pub fn new<Items, D>(
        data: Items,
        requests: R,
        mode: StreamMode,
        stats: Arc<BatchIteratorConnectionStats>,
        per_component_byte_limit_opt: Option<usize>,
        parent_trace_id: ftrace::Id,
        format: Format,
    ) -> Result<Self, AccessorError>
    where
        Items: Stream<Item = Data<D>> + Send + 'static,
        D: DiagnosticsData + 'static,
    {
        let result_stats_for_fut = Arc::clone(&stats);

        let budget_tracker_shared = Arc::new(Mutex::new(HashMap::new()));

        let truncation_counter = SchemaTruncationCounter::new();
        let stream_owned_counter_for_fut = Arc::clone(&truncation_counter);

        let data = data.then(move |mut d| {
            let stream_owned_counter = Arc::clone(&stream_owned_counter_for_fut);
            let result_stats = Arc::clone(&result_stats_for_fut);
            let budget_tracker = Arc::clone(&budget_tracker_shared);
            async move {
                let trace_id = ftrace::Id::new();
                let _trace_guard = ftrace::async_enter!(
                    trace_id,
                    TRACE_CATEGORY,
                    c"BatchIterator::new.serialize",
                    // An async duration cannot have multiple concurrent child async durations
                    // so we include the nonce as metadata to manually determine relationship.
                    "parent_trace_id" => u64::from(parent_trace_id),
                    "trace_id" => u64::from(trace_id),
                    "moniker" => d.moniker.to_string().as_ref()
                );
                let mut unlocked_counter = stream_owned_counter.lock();
                let mut tracker_guard = budget_tracker.lock();
                unlocked_counter.total_schemas += 1;
                if d.metadata.has_errors() {
                    result_stats.add_result_error();
                }

                match SerializedVmo::serialize(&d, D::DATA_TYPE, format) {
                    Err(e) => {
                        result_stats.add_result_error();
                        Err(e)
                    }
                    Ok(contents) => {
                        result_stats.add_result(1);
                        match per_component_byte_limit_opt {
                            Some(x) => {
                                if maybe_update_budget(
                                    &mut tracker_guard,
                                    &d.moniker,
                                    contents.size as usize,
                                    x,
                                ) {
                                    Ok(contents)
                                } else {
                                    result_stats.add_schema_truncated();
                                    unlocked_counter.truncated_schemas += 1;
                                    d.drop_payload();
                                    // TODO(66085): If a payload is truncated, cache the
                                    // new schema so that we can reuse if other schemas from
                                    // the same component get dropped.
                                    SerializedVmo::serialize(&d, D::DATA_TYPE, format)
                                }
                            }
                            None => Ok(contents),
                        }
                    }
                }
            }
        });

        Self::new_inner(
            new_batcher(data, Arc::clone(&stats), mode),
            requests,
            stats,
            Some(truncation_counter),
            parent_trace_id,
        )
    }

    pub fn new_serving_fxt(
        cursor: FilterCursor,
        requests: R,
        mode: StreamMode,
        stats: Arc<BatchIteratorConnectionStats>,
        parent_trace_id: ftrace::Id,
        performance_config: PerformanceConfig,
        fxt_v2: bool,
    ) -> Result<Self, AccessorError> {
        let data = FxtPacketSerializer::new(
            Arc::clone(&stats),
            performance_config
                .aggregated_content_limit_bytes
                .unwrap_or(FORMATTED_CONTENT_CHUNK_SIZE_TARGET),
            cursor,
            fxt_v2,
        );
        Self::new_inner(
            new_batcher(data, Arc::clone(&stats), mode),
            requests,
            stats,
            None,
            parent_trace_id,
        )
    }

    pub fn new_serving_arrays<D, S>(
        data: S,
        requests: R,
        mode: StreamMode,
        stats: Arc<BatchIteratorConnectionStats>,
        parent_trace_id: ftrace::Id,
    ) -> Result<Self, AccessorError>
    where
        D: Serialize + Send + 'static,
        S: Stream<Item = D> + Send + Unpin + 'static,
    {
        let data = JsonPacketSerializer::new(
            Arc::clone(&stats),
            FORMATTED_CONTENT_CHUNK_SIZE_TARGET,
            data,
        );
        Self::new_inner(
            new_batcher(data, Arc::clone(&stats), mode),
            requests,
            stats,
            None,
            parent_trace_id,
        )
    }

    fn new_inner(
        data: FormattedStream,
        requests: R,
        stats: Arc<BatchIteratorConnectionStats>,
        truncation_counter: Option<Arc<Mutex<SchemaTruncationCounter>>>,
        parent_trace_id: ftrace::Id,
    ) -> Result<Self, AccessorError> {
        stats.open_connection();
        Ok(Self { data, requests, stats, truncation_counter, parent_trace_id })
    }

    pub async fn run(mut self) -> Result<(), AccessorError> {
        self.requests.maybe_respond_ready().await?;
        while self.requests.wait_for_buffer().await.is_ok() {
            self.stats.add_request();
            let start_time = zx::MonotonicInstant::get();
            let trace_id = ftrace::Id::new();
            let _trace_guard = ftrace::async_enter!(
                trace_id,
                TRACE_CATEGORY,
                c"BatchIterator::run.get_send_batch",
                // An async duration cannot have multiple concurrent child async durations
                // so we include the nonce as metadata to manually determine relationship.
                "parent_trace_id" => u64::from(self.parent_trace_id),
                "trace_id" => u64::from(trace_id)
            );
            let batch = {
                let wait_for_close = pin!(self.requests.wait_for_close());
                let next_data = self.data.next();
                match select(next_data, wait_for_close).await {
                    // if we get None back, treat that as a terminal batch with an empty vec
                    Either::Left((batch_option, _)) => batch_option.unwrap_or_default(),
                    // if the client closes the channel, stop waiting and terminate.
                    Either::Right(_) => break,
                }
            };

            // turn errors into epitaphs -- we drop intermediate items if there was an error midway
            let batch = batch.into_iter().collect::<Result<Vec<_>, _>>()?;

            // increment counters
            self.stats.add_response();
            if batch.is_empty() {
                if let Some(truncation_count) = &self.truncation_counter {
                    let unlocked_count = truncation_count.lock();
                    if unlocked_count.total_schemas > 0 {
                        self.stats.global_stats().record_percent_truncated_schemas(
                            ((unlocked_count.truncated_schemas as f32
                                / unlocked_count.total_schemas as f32)
                                * 100.0)
                                .round() as u64,
                        );
                    }
                }
                self.stats.add_terminal();
            }
            self.stats
                .global_stats()
                .record_batch_duration(zx::MonotonicInstant::get() - start_time);
            if self.requests.write(batch).await.is_err() {
                // Peer closed, end the stream.
                break;
            }
        }
        Ok(())
    }
}

impl<R> Drop for BatchIterator<R> {
    fn drop(&mut self) {
        self.stats.close_connection();
    }
}

pub struct PerformanceConfig {
    pub batch_timeout_sec: i64,
    pub aggregated_content_limit_bytes: Option<u64>,
    pub maximum_concurrent_snapshots_per_reader: u64,
}

impl PerformanceConfig {
    pub fn new(
        params: &StreamParameters,
        maximum_concurrent_snapshots_per_reader: u64,
        default_batch_timeout_seconds: BatchRetrievalTimeout,
    ) -> Result<PerformanceConfig, AccessorError> {
        let batch_timeout = match params {
            // If only nested batch retrieval timeout is definitely not set,
            // use the optional outer field.
            StreamParameters {
                batch_retrieval_timeout_seconds,
                performance_configuration: None,
                ..
            }
            | StreamParameters {
                batch_retrieval_timeout_seconds,
                performance_configuration:
                    Some(PerformanceConfiguration { batch_retrieval_timeout_seconds: None, .. }),
                ..
            } => batch_retrieval_timeout_seconds,
            // If the outer field is definitely not set, and the inner field might be,
            // use the inner field.
            StreamParameters {
                batch_retrieval_timeout_seconds: None,
                performance_configuration:
                    Some(PerformanceConfiguration { batch_retrieval_timeout_seconds, .. }),
                ..
            } => batch_retrieval_timeout_seconds,
            // Both the inner and outer fields are set, which is an error.
            _ => return Err(AccessorError::DuplicateBatchTimeout),
        }
        .map(BatchRetrievalTimeout::from_seconds)
        .unwrap_or(default_batch_timeout_seconds);

        let aggregated_content_limit_bytes = match params {
            StreamParameters {
                performance_configuration:
                    Some(PerformanceConfiguration { max_aggregate_content_size_bytes, .. }),
                ..
            } => *max_aggregate_content_size_bytes,
            _ => None,
        };

        Ok(PerformanceConfig {
            batch_timeout_sec: batch_timeout.seconds(),
            aggregated_content_limit_bytes,
            maximum_concurrent_snapshots_per_reader,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::AccessorStats;
    use crate::logs::shared_buffer::create_ring_buffer;
    use assert_matches::assert_matches;
    use fidl_fuchsia_diagnostics::{ArchiveAccessorMarker, BatchIteratorMarker};
    use fuchsia_inspect::{Inspector, Node};

    #[fuchsia::test]
    async fn logs_only_accept_basic_component_selectors() {
        let scope = fasync::Scope::new();
        let (accessor, stream) =
            fidl::endpoints::create_proxy_and_stream::<ArchiveAccessorMarker>();
        let pipeline = Arc::new(Pipeline::for_test(None));
        let inspector = Inspector::default();
        let log_repo = LogsRepository::new(
            create_ring_buffer(1_000_000),
            std::iter::empty(),
            inspector.root(),
            scope.new_child(),
        );
        let inspect_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], scope.new_child()));
        let server = ArchiveAccessorServer::new(
            inspect_repo,
            log_repo,
            4,
            BatchRetrievalTimeout::max(),
            scope.new_child(),
        );
        server.spawn_server(pipeline, stream);

        // A selector of the form `component:node/path:property` is rejected.
        let (batch_iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
        assert!(
            accessor
                .r#stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::SnapshotThenSubscribe),
                        format: Some(Format::Json),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::Selectors(vec![
                                SelectorArgument::RawSelector("foo:root/bar:baz".to_string()),
                            ])
                        ),
                        ..Default::default()
                    },
                    server_end
                )
                .is_ok()
        );
        assert_matches!(
            batch_iterator.get_next().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                if epitaph == zx_status::Status::INVALID_ARGS
        );

        // A selector of the form `component:root` is accepted.
        let (batch_iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
        assert!(
            accessor
                .r#stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::Snapshot),
                        format: Some(Format::Json),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::Selectors(vec![
                                SelectorArgument::RawSelector("foo:root".to_string()),
                            ])
                        ),
                        ..Default::default()
                    },
                    server_end
                )
                .is_ok()
        );

        assert!(batch_iterator.get_next().await.is_ok());
    }

    #[fuchsia::test]
    async fn accessor_skips_invalid_selectors() {
        let scope = fasync::Scope::new();
        let (accessor, stream) =
            fidl::endpoints::create_proxy_and_stream::<ArchiveAccessorMarker>();
        let pipeline = Arc::new(Pipeline::for_test(None));
        let inspector = Inspector::default();
        let log_repo = LogsRepository::new(
            create_ring_buffer(1_000_000),
            std::iter::empty(),
            inspector.root(),
            scope.new_child(),
        );
        let inspect_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], scope.new_child()));
        let server = Arc::new(ArchiveAccessorServer::new(
            inspect_repo,
            log_repo,
            4,
            BatchRetrievalTimeout::max(),
            scope.new_child(),
        ));
        server.spawn_server(pipeline, stream);

        // A selector of the form `component:node/path:property` is rejected.
        let (batch_iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();

        assert!(
            accessor
                .r#stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Inspect),
                        stream_mode: Some(StreamMode::Snapshot),
                        format: Some(Format::Json),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::Selectors(vec![
                                SelectorArgument::RawSelector("invalid".to_string()),
                                SelectorArgument::RawSelector("valid:root".to_string()),
                            ])
                        ),
                        ..Default::default()
                    },
                    server_end
                )
                .is_ok()
        );

        // The batch iterator proxy should remain valid and providing responses regardless of the
        // invalid selectors that were given.
        assert!(batch_iterator.get_next().await.is_ok());
    }

    #[fuchsia::test]
    async fn buffered_iterator_handles_two_consecutive_buffer_waits() {
        let (client, server) = fidl::endpoints::create_proxy_and_stream::<BatchIteratorMarker>();
        let _fut = client.get_next();
        let mut server = server.peekable();
        assert_matches!(server.wait_for_buffer().await, Ok(()));
        assert_matches!(server.wait_for_buffer().await, Ok(()));
    }

    #[fuchsia::test]
    async fn buffered_iterator_handles_peer_closed() {
        let (client, server) = fidl::endpoints::create_proxy_and_stream::<BatchIteratorMarker>();
        let mut server = server.peekable();
        drop(client);
        assert_matches!(
            server
                .write(vec![FormattedContent::Json(Buffer {
                    size: 1,
                    vmo: zx::Vmo::create(1).unwrap(),
                })])
                .await,
            Err(IteratorError::PeerClosed)
        );
    }

    #[fuchsia::test]
    fn socket_writer_handles_json() {
        let vmo = zx::Vmo::create(1).unwrap();
        vmo.write(&[5u8], 0).unwrap();
        let koid = vmo.koid().unwrap();
        let text = FormattedContent::Json(Buffer { size: 1, vmo });
        let result = get_buffer_from_formatted_content(text).unwrap();
        assert_eq!(result.size, 1);
        assert_eq!(result.vmo.koid().unwrap(), koid);
        let mut buffer = [0];
        result.vmo.read(&mut buffer, 0).unwrap();
        assert_eq!(buffer[0], 5);
    }

    #[fuchsia::test]
    fn socket_writer_does_not_handle_cbor() {
        let vmo = zx::Vmo::create(1).unwrap();
        vmo.write(&[5u8], 0).unwrap();
        let text = FormattedContent::Cbor(vmo);
        let result = get_buffer_from_formatted_content(text);
        assert_matches!(result, Err(AccessorError::UnsupportedFormat));
    }

    #[fuchsia::test]
    async fn socket_writer_handles_closed_socket() {
        let (local, remote) = zx::Socket::create_stream();
        drop(local);
        let mut remote = fuchsia_async::Socket::from_socket(remote);
        {
            let result = ArchiveAccessorWriter::write(
                &mut remote,
                vec![FormattedContent::Json(Buffer { size: 1, vmo: zx::Vmo::create(1).unwrap() })],
            )
            .await;
            assert_matches!(result, Ok(()));
        }
        remote.wait_for_close().await;
    }

    #[fuchsia::test]
    fn batch_iterator_terminates_on_client_disconnect() {
        let mut executor = fasync::TestExecutor::new();
        let (batch_iterator_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<BatchIteratorMarker>();
        // Create a batch iterator that uses a hung stream to serve logs.
        let batch_iterator = BatchIterator::new(
            futures::stream::pending::<diagnostics_data::Data<diagnostics_data::Logs>>(),
            stream.peekable(),
            StreamMode::Subscribe,
            Arc::new(AccessorStats::new(Node::default()).new_inspect_batch_iterator()),
            None,
            ftrace::Id::new(),
            Format::Json,
        )
        .expect("create batch iterator");

        let mut batch_iterator_fut = batch_iterator.run().boxed();
        assert!(executor.run_until_stalled(&mut batch_iterator_fut).is_pending());

        // After sending a request, the request should be unfulfilled.
        let mut iterator_request_fut = batch_iterator_proxy.get_next();
        assert!(executor.run_until_stalled(&mut iterator_request_fut).is_pending());
        assert!(executor.run_until_stalled(&mut batch_iterator_fut).is_pending());
        assert!(executor.run_until_stalled(&mut iterator_request_fut).is_pending());

        // After closing the client end of the channel, the server should terminate and release
        // resources.
        drop(iterator_request_fut);
        drop(batch_iterator_proxy);
        assert_matches!(executor.run_singlethreaded(&mut batch_iterator_fut), Ok(()));
    }

    #[fuchsia::test]
    async fn batch_iterator_on_ready_is_called() {
        let scope = fasync::Scope::new();
        let (accessor, stream) =
            fidl::endpoints::create_proxy_and_stream::<ArchiveAccessorMarker>();
        let pipeline = Arc::new(Pipeline::for_test(None));
        let inspector = Inspector::default();
        let log_repo = LogsRepository::new(
            create_ring_buffer(1_000_000),
            std::iter::empty(),
            inspector.root(),
            scope.new_child(),
        );
        let inspect_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], scope.new_child()));
        let server = ArchiveAccessorServer::new(
            inspect_repo,
            log_repo,
            4,
            BatchRetrievalTimeout::max(),
            scope.new_child(),
        );
        server.spawn_server(pipeline, stream);

        // A selector of the form `component:node/path:property` is rejected.
        let (batch_iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
        assert!(
            accessor
                .r#stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::Subscribe),
                        format: Some(Format::Json),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::SelectAll(true)
                        ),
                        ..Default::default()
                    },
                    server_end
                )
                .is_ok()
        );

        // We receive a response for WaitForReady
        assert!(batch_iterator.wait_for_ready().await.is_ok());
    }

    #[fuchsia::test]
    async fn accessor_logs_with_manifest_reused_tag() {
        use crate::identity::ComponentIdentity;
        use crate::logs::shared_buffer::create_ring_buffer;
        use crate::logs::testing::make_message;
        use diagnostics_log_encoding::parse::parse_record;
        use diagnostics_log_encoding::{LOG_CONTROL_BIT, Value};
        use moniker::ExtendedMoniker;
        use zerocopy::FromBytes;

        let scope = fasync::Scope::new();
        // Use a small buffer to facilitate rolling out logs.
        let repo = LogsRepository::new(
            create_ring_buffer(65536),
            std::iter::empty(),
            &Default::default(),
            scope.new_child(),
        );

        let proxy_scope = scope.new_child();
        let (accessor, stream) =
            fidl::endpoints::create_proxy_and_stream::<ArchiveAccessorMarker>();
        let pipeline = Arc::new(Pipeline::for_test(None));
        let _inspector = fuchsia_inspect::Inspector::default();
        let inspect_repo = Arc::new(InspectRepository::new(
            vec![Arc::downgrade(&pipeline)],
            proxy_scope.new_child(),
        ));
        let server = ArchiveAccessorServer::new(
            inspect_repo,
            Arc::clone(&repo),
            4,
            BatchRetrievalTimeout::max(),
            proxy_scope,
        );
        server.spawn_server(pipeline, stream);

        let (batch_iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();

        // Connect with subscribe_to_manifest = true
        assert!(
            accessor
                .r#stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::SnapshotThenSubscribe),
                        format: Some(Format::Fxt),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::SelectAll(true)
                        ),
                        ..Default::default()
                    },
                    server_end
                )
                .is_ok()
        );

        // 1. Setup Identity A
        let identity_a = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "fuchsia-pkg://foo",
        ));
        let container_a = repo.get_log_container(Arc::clone(&identity_a));
        let tag_a = container_a.buffer().iob_tag();

        // 2. Ingest A
        container_a.ingest_message(make_message("msg_a", None, zx::BootInstant::from_nanos(1)));

        // Helper to read all records from current VMO format stream
        async fn get_next_vmo_records(
            iterator: &fidl_fuchsia_diagnostics::BatchIteratorProxy,
        ) -> Vec<(diagnostics_log_encoding::Header, diagnostics_log_encoding::Record<'static>)>
        {
            let batch = iterator.get_next().await.unwrap().unwrap();
            let mut records = vec![];
            for content in batch {
                if let FormattedContent::Fxt(vmo) = content {
                    let size = vmo.get_content_size().unwrap();
                    let mut buf = vec![0u8; size as usize];
                    vmo.read(&mut buf, 0).expect("vmo read");
                    let mut current_slice = buf.as_slice();

                    // We need actual length from VMO properties or parse_record error
                    while !current_slice.is_empty() {
                        if let Ok((record, remaining)) = parse_record(current_slice) {
                            let header = diagnostics_log_encoding::Header::read_from_bytes(
                                &current_slice[..8],
                            )
                            .unwrap();
                            records.push((header, record.into_owned()));
                            current_slice = remaining;
                        } else {
                            break; // Completed or buffer end
                        }
                    }
                }
            }
            records
        }

        // 3. Read Manifest A and Log A from proxy output
        let records = get_next_vmo_records(&batch_iterator).await;
        assert!(records.len() >= 2);

        let manifest_a = &records[0].1;
        assert_eq!(manifest_a.arguments[0].value(), Value::Text("foo".into()));

        let _log_a = &records[1].1;
        // In full stream via BatchIterator, index 2 usually holds message text if mapped.
        // For fxt, the data should just have Arguments appended?
        // Let's verify "msg_a" is contained.
        assert!(records.iter().any(|(_, r)| {
            r.arguments
                .iter()
                .any(|a| a.name() == "message" && a.value() == Value::Text("msg_a".into()))
        }));

        // 4. Mark A inactive and release
        container_a.mark_stopped();
        drop(container_a);

        // 5. Force rollout A by ingesting filler logs
        let identity_filler = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./filler").unwrap(),
            "fuchsia-pkg://filler",
        ));
        let container_filler = repo.get_log_container(Arc::clone(&identity_filler));

        let identity_b = Arc::new(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "fuchsia-pkg://bar",
        ));

        let mut container_b;
        loop {
            // Ingest filler
            container_filler.ingest_message(make_message(
                "fill",
                None,
                zx::BootInstant::from_nanos(1),
            ));

            // Try to allocate B
            container_b = repo.get_log_container(Arc::clone(&identity_b));
            if container_b.buffer().iob_tag() == tag_a {
                break;
            }

            // Failed to reuse, clean up B
            container_b.mark_stopped();
            drop(container_b);

            // Yield to let cleanup tasks run
            fasync::Timer::new(std::time::Duration::from_millis(10)).await;
        }

        // 6. Ingest B
        container_b.ingest_message(make_message("msg_b", None, zx::BootInstant::from_nanos(2)));

        // 7. Read Manifest B + Log B
        let mut manifest_b_found = false;
        let mut log_b_found = false;

        'outer: loop {
            let records = get_next_vmo_records(&batch_iterator).await;
            for (h, r) in records {
                // Is it Manifest B?
                if (h.tag() & LOG_CONTROL_BIT) != 0 {
                    if r.arguments
                        .first()
                        .map(|a| a.name() == "moniker" && a.value() == Value::Text("bar".into()))
                        .unwrap_or(false)
                    {
                        manifest_b_found = true;
                    }
                } else {
                    // Is it Log B?
                    if r.arguments
                        .iter()
                        .any(|a| a.name() == "message" && a.value() == Value::Text("msg_b".into()))
                    {
                        log_b_found = true;
                    }
                }

                if manifest_b_found && log_b_found {
                    break 'outer;
                }
            }
        }

        assert!(manifest_b_found, "Manifest B must be emitted on tag reuse");
        assert!(log_b_found, "Log B must be emitted");
    }

    #[fuchsia::test]
    fn test_get_buffer_from_formatted_content_fxt() {
        let vmo = zx::Vmo::create(100).unwrap();
        vmo.set_content_size(&50).unwrap();
        let content = fidl_fuchsia_diagnostics::FormattedContent::Fxt(vmo);
        let buffer = get_buffer_from_formatted_content(content).unwrap();
        assert_eq!(buffer.size, 50);
    }
}
