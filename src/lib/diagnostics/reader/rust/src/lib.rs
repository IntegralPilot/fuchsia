// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! A library for reading Inspect and Log data from
//! the ArchiveAccessor FIDL protocol.

use async_stream::stream;
use diagnostics_data::{DiagnosticsData, LogsData};
#[cfg(fuchsia_api_level_less_than = "HEAD")]
use diagnostics_message as _;
#[cfg(fuchsia_api_level_at_least = "HEAD")]
use diagnostics_message::{RustMessageFormatter, from_extended_record};
use fidl_fuchsia_diagnostics::{
    ArchiveAccessorMarker, ArchiveAccessorProxy, BatchIteratorMarker, BatchIteratorProxy,
    ClientSelectorConfiguration, Format, FormattedContent, PerformanceConfiguration, ReaderError,
    Selector, SelectorArgument, StreamMode, StreamParameters,
};
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt};
use fuchsia_component::client;
#[cfg(fuchsia_api_level_at_least = "HEAD")]
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use futures::sink::SinkExt;
use futures::stream::FusedStream;
use pin_project::pin_project;
use serde::Deserialize;
use std::future::ready;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use thiserror::Error;
use zx::{self as zx, MonotonicDuration};

/// Alias for ArchiveReader<Logs>. Used for reading logs.
pub type LogsArchiveReader = ArchiveReader<Logs>;

/// Alias for ArchiveReader<Inspect>. Used for reading inspect.
pub type InspectArchiveReader = ArchiveReader<Inspect>;

pub use diagnostics_data::{Data, Inspect, Logs, Severity};
pub use diagnostics_hierarchy::{DiagnosticsHierarchy, Property, hierarchy};

const RETRY_DELAY_MS: i64 = 300;

#[cfg(fuchsia_api_level_at_least = "HEAD")]
const FORMAT: Format = Format::Cbor;
#[cfg(fuchsia_api_level_less_than = "HEAD")]
const FORMAT: Format = Format::Json;

/// Errors that this library can return
#[derive(Debug, Error)]
pub enum Error {
    /// Failed to connect to the archive accessor
    #[error("Failed to connect to the archive accessor")]
    ConnectToArchive(#[source] anyhow::Error),

    /// Failed to create the BatchIterator channel ends
    #[error("Failed to create the BatchIterator channel ends")]
    CreateIteratorProxy(#[source] fidl::Error),

    /// Failed to stream diagnostics from the accessor
    #[error("Failed to stream diagnostics from the accessor")]
    StreamDiagnostics(#[source] fidl::Error),

    /// Failed to call iterator server
    #[error("Failed to call iterator server")]
    GetNextCall(#[source] fidl::Error),

    /// Received error from the GetNext response
    #[error("Received error from the GetNext response: {0:?}")]
    GetNextReaderError(ReaderError),

    /// Failed to read json received
    #[error("Failed to read json received")]
    ReadJson(#[source] serde_json::Error),

    /// Failed to read cbor received
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    #[error("Failed to read cbor received")]
    ReadCbor(#[source] anyhow::Error),

    /// Failed to parse the diagnostics data from the json received
    #[error("Failed to parse the diagnostics data from the json received")]
    ParseDiagnosticsData(#[source] serde_json::Error),

    /// Failed to read vmo from the response
    #[error("Failed to read vmo from the response")]
    ReadVmo(#[source] zx::Status),
}

/// An inspect tree selector for a component.
pub struct ComponentSelector {
    moniker: Vec<String>,
    tree_selectors: Vec<String>,
}

impl ComponentSelector {
    /// Create a new component event selector.
    /// By default it will select the whole tree unless tree selectors are provided.
    /// `moniker` is the realm path relative to the realm of the running component plus the
    /// component name. For example: [a, b, component].
    pub fn new(moniker: Vec<String>) -> Self {
        Self { moniker, tree_selectors: Vec::new() }
    }

    /// Select a section of the inspect tree.
    pub fn with_tree_selector(mut self, tree_selector: impl Into<String>) -> Self {
        self.tree_selectors.push(tree_selector.into());
        self
    }

    fn moniker_str(&self) -> String {
        self.moniker.join("/")
    }
}

/// Trait used for things that can be converted to selector arguments.
pub trait ToSelectorArguments {
    /// Converts this to selector arguments.
    fn to_selector_arguments(self) -> Box<dyn Iterator<Item = SelectorArgument>>;
}

/// Trait used for things that can be converted to component selector arguments.
pub trait ToComponentSelectorArguments {
    /// Converts this to selector arguments.
    fn to_component_selector_arguments(self) -> ComponentSelector;
}

impl ToComponentSelectorArguments for &str {
    fn to_component_selector_arguments(self) -> ComponentSelector {
        if self.contains("\\:") {
            // String is already escaped, don't escape it.
            ComponentSelector::new(self.split("/").map(|value| value.to_string()).collect())
        } else {
            // String isn't escaped, escape it
            ComponentSelector::new(
                selectors::sanitize_moniker_for_selectors(self)
                    .split("/")
                    .map(|value| value.to_string())
                    .collect(),
            )
            .with_tree_selector("[...]root")
        }
    }
}

impl ToComponentSelectorArguments for String {
    fn to_component_selector_arguments(self) -> ComponentSelector {
        self.as_str().to_component_selector_arguments()
    }
}

impl ToComponentSelectorArguments for ComponentSelector {
    fn to_component_selector_arguments(self) -> ComponentSelector {
        self
    }
}

impl ToSelectorArguments for String {
    fn to_selector_arguments(self) -> Box<dyn Iterator<Item = SelectorArgument>> {
        Box::new([SelectorArgument::RawSelector(self)].into_iter())
    }
}

impl ToSelectorArguments for &str {
    fn to_selector_arguments(self) -> Box<dyn Iterator<Item = SelectorArgument>> {
        Box::new([SelectorArgument::RawSelector(self.to_string())].into_iter())
    }
}

impl ToSelectorArguments for ComponentSelector {
    fn to_selector_arguments(self) -> Box<dyn Iterator<Item = SelectorArgument>> {
        let moniker = self.moniker_str();
        // If not tree selectors were provided, select the full tree.
        if self.tree_selectors.is_empty() {
            Box::new([SelectorArgument::RawSelector(format!("{moniker}:root"))].into_iter())
        } else {
            Box::new(
                self.tree_selectors
                    .into_iter()
                    .map(move |s| SelectorArgument::RawSelector(format!("{moniker}:{s}"))),
            )
        }
    }
}

impl ToSelectorArguments for Selector {
    fn to_selector_arguments(self) -> Box<dyn Iterator<Item = SelectorArgument>> {
        Box::new([SelectorArgument::StructuredSelector(self)].into_iter())
    }
}

/// Before unsealing this, consider whether your code belongs in this file.
pub trait SerializableValue: private::Sealed {
    /// The Format of this SerializableValue. Either Logs or Inspect.
    const FORMAT_OF_VALUE: Format;
}

/// Trait used to verify that a JSON payload has a valid diagnostics payload.
pub trait CheckResponse: private::Sealed {
    /// Returns true if the response has a valid payload.
    fn has_payload(&self) -> bool;
}

// The "sealed trait" pattern.
//
// https://rust-lang.github.io/api-guidelines/future-proofing.html
mod private {
    pub trait Sealed {}
}
impl private::Sealed for serde_json::Value {}
impl private::Sealed for ciborium::Value {}
impl<D: DiagnosticsData> private::Sealed for Data<D> {}

impl<D: DiagnosticsData> CheckResponse for Data<D> {
    fn has_payload(&self) -> bool {
        self.payload.is_some()
    }
}

impl SerializableValue for serde_json::Value {
    const FORMAT_OF_VALUE: Format = Format::Json;
}

impl CheckResponse for serde_json::Value {
    fn has_payload(&self) -> bool {
        match self {
            serde_json::Value::Object(obj) => {
                obj.get("payload").map(|p| !matches!(p, serde_json::Value::Null)).is_some()
            }
            _ => false,
        }
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl SerializableValue for ciborium::Value {
    const FORMAT_OF_VALUE: Format = Format::Cbor;
}

impl CheckResponse for ciborium::Value {
    fn has_payload(&self) -> bool {
        match self {
            ciborium::Value::Map(m) => {
                let payload_key = ciborium::Value::Text("payload".into());
                m.iter().any(|(key, _)| *key == payload_key)
            }
            _ => false,
        }
    }
}

/// Retry configuration for ArchiveReader
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetryConfig {
    /// The minimum schema count required for a successful read.
    /// This guarantees that a read will contain at least MinSchemaCount
    /// results.
    MinSchemaCount(usize),
}

impl RetryConfig {
    /// Always retry
    pub fn always() -> Self {
        Self::MinSchemaCount(1)
    }

    /// Never retry
    pub fn never() -> Self {
        Self::MinSchemaCount(0)
    }

    /// Retry result_count times
    fn should_retry(&self, result_count: usize) -> bool {
        match self {
            Self::MinSchemaCount(min) => *min > result_count,
        }
    }
}

/// A trait representing a type of diagnostics data.
pub trait DiagnosticsDataType: private::Sealed {}

impl private::Sealed for Logs {}

impl private::Sealed for Inspect {}

impl DiagnosticsDataType for Logs {}

impl DiagnosticsDataType for Inspect {}

/// Utility for reading inspect data of a running component using the injected Archive
/// Reader service.
pub struct ArchiveReader<T> {
    archive: Option<ArchiveAccessorProxy>,
    selectors: Vec<SelectorArgument>,
    retry_config: RetryConfig,
    timeout: Option<MonotonicDuration>,
    batch_retrieval_timeout_seconds: Option<i64>,
    max_aggregated_content_size_bytes: Option<u64>,
    format: Option<Format>,
    _phantom: PhantomData<T>,
}

impl<T: DiagnosticsDataType> ArchiveReader<T> {
    /// Initializes the ArchiveReader with a custom connection to an ArchiveAccessor.
    /// By default, the connection will be initialized by connecting to
    /// fuchsia.diagnostics.ArchiveAccessor
    pub fn with_archive(&mut self, archive: ArchiveAccessorProxy) -> &mut Self {
        self.archive = Some(archive);
        self
    }

    /// Sets the minimum number of schemas expected in a result in order for the
    /// result to be considered a success.
    pub fn with_minimum_schema_count(&mut self, minimum_schema_count: usize) -> &mut Self {
        self.retry_config = RetryConfig::MinSchemaCount(minimum_schema_count);
        self
    }

    /// Sets a custom retry configuration. By default we always retry.
    pub fn retry(&mut self, config: RetryConfig) -> &mut Self {
        self.retry_config = config;
        self
    }

    /// Sets the maximum time to wait for a response from the Archive.
    /// Do not use in tests unless timeout is the expected behavior.
    pub fn with_timeout(&mut self, duration: MonotonicDuration) -> &mut Self {
        self.timeout = Some(duration);
        self
    }

    /// Filters logs for a specific component or component selector.
    /// If string input, the string may be either a component selector string
    /// or a moniker, or a ComponentSelector may be passed directly.
    pub fn select_all_for_component(
        &mut self,
        component: impl ToComponentSelectorArguments,
    ) -> &mut Self {
        self.selectors.extend(component.to_component_selector_arguments().to_selector_arguments());
        self
    }

    /// Connects to the ArchiveAccessor and returns data matching provided selectors.
    async fn snapshot_shared<D>(&self, format: Format) -> Result<Vec<Data<D>>, Error>
    where
        D: DiagnosticsData + 'static,
    {
        let data_future = self.snapshot_inner::<D, Data<D>>(format);
        let data = match self.timeout {
            Some(timeout) => data_future.on_timeout(timeout.after_now(), || Ok(Vec::new())).await?,
            None => data_future.await?,
        };
        Ok(data)
    }

    async fn snapshot_inner<D, Y>(&self, format: Format) -> Result<Vec<Y>, Error>
    where
        D: DiagnosticsData,
        Y: for<'a> Deserialize<'a> + CheckResponse + Send + 'static,
    {
        loop {
            let iterator = self.batch_iterator::<D>(StreamMode::Snapshot, format)?;
            let result = drain_batch_iterator::<Y>(Arc::new(iterator))
                .filter_map(|value| ready(value.ok()))
                .collect::<Vec<_>>()
                .await;

            if self.retry_config.should_retry(result.len()) {
                fasync::Timer::new(fasync::MonotonicInstant::after(
                    zx::MonotonicDuration::from_millis(RETRY_DELAY_MS),
                ))
                .await;
            } else {
                return Ok(result);
            }
        }
    }

    fn batch_iterator<D>(
        &self,
        mode: StreamMode,
        format: Format,
    ) -> Result<BatchIteratorProxy, Error>
    where
        D: DiagnosticsData,
    {
        let archive = match &self.archive {
            Some(archive) => archive.clone(),
            None => client::connect_to_protocol::<ArchiveAccessorMarker>()
                .map_err(Error::ConnectToArchive)?,
        };

        let (iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
        let stream_parameters = StreamParameters {
            stream_mode: Some(mode),
            data_type: Some(D::DATA_TYPE),
            format: Some(format),
            client_selector_configuration: if self.selectors.is_empty() {
                Some(ClientSelectorConfiguration::SelectAll(true))
            } else {
                Some(ClientSelectorConfiguration::Selectors(self.selectors.to_vec()))
            },
            performance_configuration: Some(PerformanceConfiguration {
                max_aggregate_content_size_bytes: self.max_aggregated_content_size_bytes,
                batch_retrieval_timeout_seconds: self.batch_retrieval_timeout_seconds,
                ..Default::default()
            }),
            ..Default::default()
        };

        archive
            .stream_diagnostics(&stream_parameters, server_end)
            .map_err(Error::StreamDiagnostics)?;
        Ok(iterator)
    }
}

impl ArchiveReader<Logs> {
    /// Creates an ArchiveReader for reading logs
    pub fn logs() -> Self {
        ArchiveReader::<Logs> {
            timeout: None,
            format: None,
            selectors: vec![],
            retry_config: RetryConfig::always(),
            archive: None,
            batch_retrieval_timeout_seconds: None,
            max_aggregated_content_size_bytes: None,
            _phantom: PhantomData,
        }
    }

    #[doc(hidden)]
    pub fn with_format(&mut self, format: Format) -> &mut Self {
        self.format = Some(format);
        self
    }

    #[inline]
    fn format(&self) -> Format {
        match self.format {
            Some(f) => f,
            None => {
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                let ret = Format::LegacyFxt;
                #[cfg(fuchsia_api_level_less_than = "HEAD")]
                let ret = Format::Json;
                ret
            }
        }
    }

    /// Connects to the ArchiveAccessor and returns data matching provided selectors.
    pub async fn snapshot(&self) -> Result<Vec<Data<Logs>>, Error> {
        let fut = async {
            loop {
                let iterator = self.batch_iterator::<Logs>(StreamMode::Snapshot, self.format())?;
                let result = drain_batch_iterator_for_logs(Arc::new(iterator), Some(self.format()))
                    .filter_map(|value| ready(value.ok()))
                    .collect::<Vec<_>>()
                    .await;
                if self.retry_config.should_retry(result.len()) {
                    fasync::Timer::new(fasync::MonotonicInstant::after(
                        zx::MonotonicDuration::from_millis(RETRY_DELAY_MS),
                    ))
                    .await;
                } else {
                    return Ok(result);
                }
            }
        };
        match self.timeout {
            Some(timeout) => fut.on_timeout(timeout.after_now(), || Ok(Vec::new())).await,
            None => fut.await,
        }
    }

    /// Connects to the ArchiveAccessor and returns a stream of data containing a snapshot of the
    /// current buffer in the Archivist as well as new data that arrives.
    pub fn snapshot_then_subscribe(&self) -> Result<Subscription, Error> {
        let iterator =
            self.batch_iterator::<Logs>(StreamMode::SnapshotThenSubscribe, self.format())?;
        Ok(Subscription::new_with_format(iterator, self.format()))
    }
}

impl ArchiveReader<Inspect> {
    /// Creates an ArchiveReader for reading Inspect data.
    pub fn inspect() -> Self {
        ArchiveReader::<Inspect> {
            timeout: None,
            format: None,
            selectors: vec![],
            retry_config: RetryConfig::always(),
            archive: None,
            batch_retrieval_timeout_seconds: None,
            max_aggregated_content_size_bytes: None,
            _phantom: PhantomData,
        }
    }

    /// Set the maximum time to wait for a wait for a single component
    /// to have its diagnostics data "pumped".
    pub fn with_batch_retrieval_timeout_seconds(&mut self, timeout: i64) -> &mut Self {
        self.batch_retrieval_timeout_seconds = Some(timeout);
        self
    }

    /// Sets the total number of bytes allowed in a single VMO read.
    pub fn with_aggregated_result_bytes_limit(&mut self, limit_bytes: u64) -> &mut Self {
        self.max_aggregated_content_size_bytes = Some(limit_bytes);
        self
    }

    /// Connects to the ArchiveAccessor and returns inspect data matching provided selectors.
    /// Returns the raw json for each hierarchy fetched. This is used for CTF compatibility
    /// tests (which test various implementation details of the JSON format),
    /// and use beyond such tests is discouraged.
    pub async fn snapshot_raw<T>(&self) -> Result<T, Error>
    where
        T: for<'a> Deserialize<'a>
            + SerializableValue
            + From<Vec<T>>
            + CheckResponse
            + 'static
            + Send,
    {
        let data_future = self.snapshot_inner::<Inspect, T>(T::FORMAT_OF_VALUE);
        let data = match self.timeout {
            Some(timeout) => data_future.on_timeout(timeout.after_now(), || Ok(Vec::new())).await?,
            None => data_future.await?,
        };
        Ok(T::from(data))
    }

    /// Adds selectors used for performing filtering inspect hierarchies.
    /// This may be called multiple times to add additional selectors.
    pub fn add_selectors<T, S>(&mut self, selectors: T) -> &mut Self
    where
        T: Iterator<Item = S>,
        S: ToSelectorArguments,
    {
        for selector in selectors {
            self.add_selector(selector);
        }
        self
    }

    /// Requests a single component tree (or sub-tree).
    pub fn add_selector(&mut self, selector: impl ToSelectorArguments) -> &mut Self {
        self.selectors.extend(selector.to_selector_arguments());
        self
    }

    /// Sets the format to use when reading inspect data.
    pub fn with_format(&mut self, format: Format) -> &mut Self {
        self.format = Some(format);
        self
    }

    #[inline]
    fn format(&self) -> Format {
        match self.format {
            Some(f) => f,
            None => FORMAT,
        }
    }

    /// Connects to the ArchiveAccessor and returns data matching provided selectors.
    pub async fn snapshot(&self) -> Result<Vec<Data<Inspect>>, Error> {
        self.snapshot_shared::<Inspect>(self.format()).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    Many(Vec<T>),
    One(T),
}

fn stream_batch<T>(
    iterator: Arc<BatchIteratorProxy>,
    process_content: impl Fn(FormattedContent) -> Result<OneOrMany<T>, Error>,
) -> impl Stream<Item = Result<T, Error>>
where
    T: for<'a> Deserialize<'a> + Send + 'static,
{
    stream! {
        loop {
            let next_batch = iterator
                .get_next()
                .await
                .map_err(Error::GetNextCall)?
                .map_err(Error::GetNextReaderError)?;
            if next_batch.is_empty() {
                // End of stream
                return;
            }
            for formatted_content in next_batch {
                let output = process_content(formatted_content)?;
                match output {
                    OneOrMany::One(data) => yield Ok(data),
                    OneOrMany::Many(datas) => {
                        for data in datas {
                            yield Ok(data);
                        }
                    }
                }
            }
        }
    }
}

/// Drain a batch iterator.
pub fn drain_batch_iterator<T>(
    iterator: Arc<BatchIteratorProxy>,
) -> impl Stream<Item = Result<T, Error>>
where
    T: for<'a> Deserialize<'a> + Send + 'static,
{
    stream_batch(iterator, |formatted_content| match formatted_content {
        FormattedContent::Json(data) => {
            let mut buf = vec![0; data.size as usize];
            data.vmo.read(&mut buf, 0).map_err(Error::ReadVmo)?;
            serde_json::from_slice(&buf).map_err(Error::ReadJson)
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        FormattedContent::Cbor(vmo) => {
            let mut buf = vec![0; vmo.get_content_size().expect("Always returns Ok") as usize];
            vmo.read(&mut buf, 0).map_err(Error::ReadVmo)?;
            Ok(ciborium::from_reader(buf.as_slice()).map_err(|err| Error::ReadCbor(err.into()))?)
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        FormattedContent::Fxt(_) => unreachable!("We never expect FXT for Inspect"),
        FormattedContent::__SourceBreaking { unknown_ordinal: _ } => {
            unreachable!("Received unrecognized FIDL message")
        }
    })
}

fn drain_batch_iterator_for_logs(
    iterator: Arc<BatchIteratorProxy>,
    _format: Option<Format>,
) -> impl Stream<Item = Result<LogsData, Error>> {
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    let parser = Arc::new(Mutex::new(diagnostics_message::MessageParser::default()));
    stream_batch::<LogsData>(iterator, move |formatted_content| match formatted_content {
        FormattedContent::Json(data) => {
            let mut buf = vec![0; data.size as usize];
            data.vmo.read(&mut buf, 0).map_err(Error::ReadVmo)?;
            serde_json::from_slice(&buf).map_err(Error::ReadJson)
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        FormattedContent::Fxt(vmo) => {
            let mut buf = vec![0; vmo.get_content_size().expect("Always returns Ok") as usize];
            vmo.read(&mut buf, 0).map_err(Error::ReadVmo)?;
            let mut current_slice: &[u8] = &buf;
            let mut items = vec![];
            let mut parser = parser.lock();

            while !current_slice.is_empty() {
                if _format == Some(Format::Fxt) {
                    match parser.parse_next(current_slice, RustMessageFormatter) {
                        Ok((maybe_data, remaining)) => {
                            assert!(remaining.len() < current_slice.len(), "Parser must advance");
                            if let Some(data) = maybe_data {
                                items.push(data);
                            }
                            current_slice = remaining;
                        }
                        Err(_) => {
                            // This can happen if we are reading a truncated record.
                            // Stop parsing this buffer.
                            break;
                        }
                    }
                } else {
                    match from_extended_record(current_slice) {
                        Ok((data, remaining)) => {
                            items.push(data);
                            current_slice = remaining;
                        }
                        Err(_) => {
                            // This can happen if we are reading a truncated record.
                            // Stop parsing this buffer.
                            break;
                        }
                    }
                }
            }
            Ok(OneOrMany::Many(items))
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        FormattedContent::Cbor(_) => unreachable!("We never expect CBOR"),
        FormattedContent::__SourceBreaking { unknown_ordinal: _ } => {
            unreachable!("Received unrecognized FIDL message")
        }
    })
}

/// A subscription used for reading logs.
#[pin_project]
pub struct Subscription {
    #[pin]
    recv: Pin<Box<dyn FusedStream<Item = Result<LogsData, Error>> + Send>>,
    iterator: Arc<BatchIteratorProxy>,
}

const DATA_CHANNEL_SIZE: usize = 32;
const ERROR_CHANNEL_SIZE: usize = 2;

impl Subscription {
    /// Creates a new subscription stream to a batch iterator.
    /// The stream will return diagnostics data structures.
    pub fn new(iterator: BatchIteratorProxy) -> Self {
        let iterator = Arc::new(iterator);
        Subscription {
            recv: Box::pin(drain_batch_iterator_for_logs(iterator.clone(), None).fuse()),
            iterator,
        }
    }

    /// Creates a new subscription stream to a batch iterator.
    /// The stream will return diagnostics data structures.
    pub fn new_with_format(iterator: BatchIteratorProxy, format: Format) -> Self {
        let iterator = Arc::new(iterator);
        Subscription {
            recv: Box::pin(drain_batch_iterator_for_logs(iterator.clone(), Some(format)).fuse()),
            iterator,
        }
    }

    /// Wait for the connection with the server to be established.
    pub async fn wait_for_ready(&self) {
        self.iterator.wait_for_ready().await.expect("doesn't disconnect");
    }

    /// Splits the subscription into two separate streams: results and errors.
    pub fn split_streams(mut self) -> (SubscriptionResultsStream<LogsData>, mpsc::Receiver<Error>) {
        let (mut errors_sender, errors) = mpsc::channel(ERROR_CHANNEL_SIZE);
        let (mut results_sender, recv) = mpsc::channel(DATA_CHANNEL_SIZE);
        let _drain_task = fasync::Task::spawn(async move {
            while let Some(result) = self.next().await {
                match result {
                    Ok(value) => {
                        if results_sender.send(value).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        // Use try_send so that if the error receiver is full or unpolled,
                        // data stream processing does not deadlock.
                        let _ = errors_sender.try_send(e);
                    }
                };
            }
        });
        (SubscriptionResultsStream { recv, _drain_task }, errors)
    }
}

impl Stream for Subscription {
    type Item = Result<LogsData, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        this.recv.poll_next(cx)
    }
}

impl FusedStream for Subscription {
    fn is_terminated(&self) -> bool {
        self.recv.is_terminated()
    }
}

/// A stream for reading diagnostics data
#[pin_project]
pub struct SubscriptionResultsStream<T> {
    #[pin]
    recv: mpsc::Receiver<T>,
    _drain_task: fasync::Task<()>,
}

impl<T> Stream for SubscriptionResultsStream<T>
where
    T: for<'a> Deserialize<'a>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        this.recv.poll_next(cx)
    }
}

impl<T> FusedStream for SubscriptionResultsStream<T>
where
    T: for<'a> Deserialize<'a>,
{
    fn is_terminated(&self) -> bool {
        self.recv.is_terminated()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use diagnostics_log::{Publisher, PublisherOptions};
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_diagnostics as fdiagnostics;
    use fuchsia_component_test::{
        Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route,
    };
    use futures::TryStreamExt;
    use log::{error, info};

    const TEST_COMPONENT_URL: &str = "#meta/inspect_test_component.cm";

    struct ComponentOptions {
        publish_n_trees: u64,
    }

    async fn start_component(opts: ComponentOptions) -> Result<RealmInstance, anyhow::Error> {
        let builder = RealmBuilder::new().await?;
        let test_component = builder
            .add_child("test_component", TEST_COMPONENT_URL, ChildOptions::new().eager())
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .from(Ref::parent())
                    .to(&test_component),
            )
            .await?;
        builder.init_mutable_config_to_empty(&test_component).await.unwrap();
        builder
            .set_config_value(&test_component, "publish_n_trees", opts.publish_n_trees.into())
            .await
            .unwrap();
        let instance = builder.build().await?;
        Ok(instance)
    }

    // All selectors in this test select against all tree names, in order to ensure the expected
    // number of trees are published
    #[fuchsia::test]
    async fn inspect_data_for_component() -> Result<(), anyhow::Error> {
        let instance = start_component(ComponentOptions { publish_n_trees: 1 }).await?;
        let moniker = format!("realm_builder:{}/test_component", instance.root.child_name());
        let component_selector = selectors::sanitize_moniker_for_selectors(&moniker);
        let results = ArchiveReader::inspect()
            .add_selector(format!("{component_selector}:[...]root"))
            .snapshot()
            .await?;
        assert_eq!(results.len(), 1);
        assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
            "tree-0": 0u64,
            int: 3u64,
            "lazy-node": {
                a: "test",
                child: {
                    double: 3.25,
                },
            }
        });
        // add_selector can take either a String or a Selector.
        let lazy_property_selector = Selector {
            component_selector: Some(fdiagnostics::ComponentSelector {
                moniker_segments: Some(vec![
                    fdiagnostics::StringSelector::ExactMatch(format!(
                        "realm_builder:{}",
                        instance.root.child_name()
                    )),
                    fdiagnostics::StringSelector::ExactMatch("test_component".into()),
                ]),
                ..Default::default()
            }),
            tree_selector: Some(fdiagnostics::TreeSelector::PropertySelector(
                fdiagnostics::PropertySelector {
                    node_path: vec![
                        fdiagnostics::StringSelector::ExactMatch("root".into()),
                        fdiagnostics::StringSelector::ExactMatch("lazy-node".into()),
                    ],
                    target_properties: fdiagnostics::StringSelector::ExactMatch("a".into()),
                },
            )),
            tree_names: Some(fdiagnostics::TreeNames::All(fdiagnostics::All {})),
            ..Default::default()
        };
        let int_property_selector = format!("{component_selector}:[...]root:int");
        let mut reader = ArchiveReader::inspect();
        reader.add_selector(int_property_selector).add_selector(lazy_property_selector);
        let response = reader.snapshot().await?;
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].moniker.to_string(), moniker);
        assert_data_tree!(response[0].payload.as_ref().unwrap(), root: {
            int: 3u64,
            "lazy-node": {
                a: "test"
            }
        });
        Ok(())
    }

    #[fuchsia::test]
    async fn select_all_for_moniker() {
        let instance = start_component(ComponentOptions { publish_n_trees: 1 })
            .await
            .expect("component started");
        let moniker = format!("realm_builder:{}/test_component", instance.root.child_name());
        let results = ArchiveReader::inspect()
            .select_all_for_component(moniker)
            .snapshot()
            .await
            .expect("snapshotted");
        assert_eq!(results.len(), 1);
        assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
            "tree-0": 0u64,
            int: 3u64,
            "lazy-node": {
                a: "test",
                child: {
                    double: 3.25,
                },
            }
        });
    }

    #[fuchsia::test]
    async fn timeout() -> Result<(), anyhow::Error> {
        let instance = start_component(ComponentOptions { publish_n_trees: 1 }).await?;

        let mut reader = ArchiveReader::inspect();
        reader
            .add_selector(format!(
                "realm_builder\\:{}/test_component:root",
                instance.root.child_name()
            ))
            .with_timeout(zx::MonotonicDuration::from_nanos(0));
        let result = reader.snapshot().await;
        assert!(result.unwrap().is_empty());
        Ok(())
    }

    #[fuchsia::test]
    async fn component_selector() {
        let selector = ComponentSelector::new(vec!["a".to_string()]);
        assert_eq!(selector.moniker_str(), "a");
        let arguments: Vec<_> = selector.to_selector_arguments().collect();
        assert_eq!(arguments, vec![SelectorArgument::RawSelector("a:root".to_string())]);

        let selector =
            ComponentSelector::new(vec!["b".to_string(), "c".to_string(), "a".to_string()]);
        assert_eq!(selector.moniker_str(), "b/c/a");

        let selector = selector.with_tree_selector("root/b/c:d").with_tree_selector("root/e:f");
        let arguments: Vec<_> = selector.to_selector_arguments().collect();
        assert_eq!(
            arguments,
            vec![
                SelectorArgument::RawSelector("b/c/a:root/b/c:d".into()),
                SelectorArgument::RawSelector("b/c/a:root/e:f".into()),
            ]
        );
    }

    #[fuchsia::test]
    async fn custom_archive() {
        let proxy = spawn_fake_archive(serde_json::json!({
            "moniker": "moniker",
            "version": 1,
            "data_source": "Inspect",
            "metadata": {
              "component_url": "component-url",
              "timestamp": 0,
              "filename": "filename",
            },
            "payload": {
                "root": {
                    "x": 1,
                }
            }
        }));
        let result =
            ArchiveReader::inspect().with_archive(proxy).snapshot().await.expect("got result");
        assert_eq!(result.len(), 1);
        assert_data_tree!(result[0].payload.as_ref().unwrap(), root: { x: 1u64 });
    }

    #[fuchsia::test]
    async fn handles_lists_correctly_on_snapshot_raw() {
        let value = serde_json::json!({
            "moniker": "moniker",
            "version": 1,
            "data_source": "Inspect",
            "metadata": {
            "component_url": "component-url",
            "timestamp": 0,
            "filename": "filename",
            },
            "payload": {
                "root": {
                    "x": 1,
                }
            }
        });
        let proxy = spawn_fake_archive(serde_json::json!([value.clone()]));
        let mut reader = ArchiveReader::inspect();
        reader.with_archive(proxy);
        let json_result = reader.snapshot_raw::<serde_json::Value>().await.expect("got result");
        match json_result {
            serde_json::Value::Array(values) => {
                assert_eq!(values.len(), 1);
                assert_eq!(values[0], value);
            }
            result => panic!("unexpected result: {result:?}"),
        }
        let cbor_result = reader.snapshot_raw::<ciborium::Value>().await.expect("got result");
        match cbor_result {
            ciborium::Value::Array(values) => {
                assert_eq!(values.len(), 1);
                let json_result =
                    values[0].deserialized::<serde_json::Value>().expect("convert to json");
                assert_eq!(json_result, value);
            }
            result => panic!("unexpected result: {result:?}"),
        }
    }

    #[fuchsia::test(logging = false)]
    async fn snapshot_then_subscribe() {
        let (_instance, publisher, reader) = init_isolated_logging().await;
        let (mut stream, _errors) =
            reader.snapshot_then_subscribe().expect("subscribed to logs").split_streams();
        publisher.register_logger(None).unwrap();
        info!("hello from test");
        error!("error from test");
        let log = stream.next().await.unwrap();
        assert_eq!(log.msg().unwrap(), "hello from test");
        let log = stream.next().await.unwrap();
        assert_eq!(log.msg().unwrap(), "error from test");
    }

    #[fuchsia::test]
    async fn read_many_trees_with_filtering() {
        let instance = start_component(ComponentOptions { publish_n_trees: 2 })
            .await
            .expect("component started");
        let selector = format!(
            "realm_builder\\:{}/test_component:[name=tree-0]root",
            instance.root.child_name()
        );
        let results = ArchiveReader::inspect()
            .add_selector(selector)
            // Only one schema since empty schemas are filtered out
            .with_minimum_schema_count(1)
            .snapshot()
            .await
            .expect("snapshotted");
        assert_matches!(results.iter().find(|v| v.metadata.name.as_ref() == "tree-1"), None);
        let should_have_data =
            results.into_iter().find(|v| v.metadata.name.as_ref() == "tree-0").unwrap();
        assert_data_tree!(should_have_data.payload.unwrap(), root: contains {
            "tree-0": 0u64,
        });
    }

    fn spawn_fake_archive(data_to_send: serde_json::Value) -> fdiagnostics::ArchiveAccessorProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fdiagnostics::ArchiveAccessorMarker>();
        fasync::Task::spawn(async move {
            while let Some(request) = stream.try_next().await.expect("stream request") {
                match request {
                    fdiagnostics::ArchiveAccessorRequest::StreamDiagnostics {
                        result_stream,
                        ..
                    } => {
                        let data = data_to_send.clone();
                        fasync::Task::spawn(handle_batch_iterator(data, result_stream)).detach();
                    }
                    fdiagnostics::ArchiveAccessorRequest::WaitForReady { responder, .. } => {
                        let _ = responder.send();
                    }
                    fdiagnostics::ArchiveAccessorRequest::_UnknownMethod { .. } => {
                        unreachable!("Unexpected method call");
                    }
                    fidl_fuchsia_diagnostics::ArchiveAccessorRequest::StreamDiagnosticsToSocket { .. } => {
                        unreachable!("Unexpected method call");
                    },
                }
            }
        })
        .detach();
        proxy
    }

    async fn handle_batch_iterator(
        data: serde_json::Value,
        result_stream: ServerEnd<fdiagnostics::BatchIteratorMarker>,
    ) {
        let mut called = false;
        let mut stream = result_stream.into_stream();
        while let Some(req) = stream.try_next().await.expect("stream request") {
            match req {
                fdiagnostics::BatchIteratorRequest::WaitForReady { responder } => {
                    let _ = responder.send();
                }
                fdiagnostics::BatchIteratorRequest::GetNext { responder } => {
                    if called {
                        responder.send(Ok(Vec::new())).expect("send response");
                        continue;
                    }
                    called = true;
                    let content = serde_json::to_string_pretty(&data).expect("json pretty");
                    let vmo_size = content.len() as u64;
                    let vmo = zx::Vmo::create(vmo_size).expect("create vmo");
                    vmo.write(content.as_bytes(), 0).expect("write vmo");
                    let buffer = fidl_fuchsia_mem::Buffer { vmo, size: vmo_size };
                    responder
                        .send(Ok(vec![fdiagnostics::FormattedContent::Json(buffer)]))
                        .expect("send response");
                }
                fdiagnostics::BatchIteratorRequest::_UnknownMethod { .. } => {
                    unreachable!("Unexpected method call");
                }
            }
        }
    }

    async fn create_realm() -> RealmBuilder {
        let builder = RealmBuilder::new().await.expect("create realm builder");
        let archivist = builder
            .add_child("archivist", "#meta/archivist-for-embedding.cm", ChildOptions::new().eager())
            .await
            .expect("add child archivist");
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .capability(
                        Capability::protocol_by_name("fuchsia.tracing.provider.Registry")
                            .optional(),
                    )
                    .capability(Capability::event_stream("stopped"))
                    .capability(Capability::event_stream("capability_requested"))
                    .from(Ref::parent())
                    .to(&archivist),
            )
            .await
            .expect("added routes from parent to archivist");
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .from(&archivist)
                    .to(Ref::parent()),
            )
            .await
            .expect("routed LogSink from archivist to parent");
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.diagnostics.ArchiveAccessor"))
                    .from(Ref::dictionary(&archivist, "diagnostics-accessors"))
                    .to(Ref::parent()),
            )
            .await
            .expect("routed ArchiveAccessor from archivist to parent");
        builder
    }

    async fn init_isolated_logging() -> (RealmInstance, Publisher, ArchiveReader<Logs>) {
        let instance = create_realm().await.build().await.unwrap();
        let log_sink_client = instance.root.connect_to_protocol_at_exposed_dir().unwrap();
        let accessor_proxy = instance.root.connect_to_protocol_at_exposed_dir().unwrap();
        let mut reader = ArchiveReader::logs();
        reader.with_archive(accessor_proxy);
        let options = PublisherOptions::default().use_log_sink(log_sink_client);
        let publisher = Publisher::new_async(options).await.unwrap();
        (instance, publisher, reader)
    }

    #[fuchsia::test]
    fn retry_config_behavior() {
        let config = RetryConfig::MinSchemaCount(1);
        let got = 0;

        assert!(config.should_retry(got));

        let config = RetryConfig::MinSchemaCount(1);
        let got = 1;

        assert!(!config.should_retry(got));

        let config = RetryConfig::MinSchemaCount(1);
        let got = 2;

        assert!(!config.should_retry(got));

        let config = RetryConfig::MinSchemaCount(0);
        let got = 1;

        assert!(!config.should_retry(got));

        let config = RetryConfig::always();
        let got = 0;

        assert!(config.should_retry(got));

        let config = RetryConfig::never();
        let got = 0;

        assert!(!config.should_retry(got));
    }
}
