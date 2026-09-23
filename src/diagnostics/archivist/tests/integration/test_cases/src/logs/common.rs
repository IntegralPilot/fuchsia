// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::{Data, Logs, LogsProperty, Severity};
use diagnostics_reader::{ArchiveReader, RetryConfig};
use fidl_fuchsia_diagnostics::{
    ArchiveAccessorProxy, ClientSelectorConfiguration, DataType, Format, StreamMode,
    StreamParameters,
};
use fidl_fuchsia_diagnostics_host::ArchiveAccessorProxy as HostArchiveAccessor;
use futures::StreamExt;
use futures::stream::BoxStream;
use log_command_ctf::LogsDataStream;

/// The interface format to use for reading logs.
#[derive(Debug)]
pub enum LogFormat {
    /// Reads logs using the FXT format. This path utilizes `internal::FfiReader`, which employs
    /// the `diagnostics_message` crate to parse the FXT data. This parsing logic is the same
    /// as the translation that occurs on the Rust side before log messages are exposed to the
    /// C++ logging interfaces.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    Ffi,
    /// Uses the standard Rust library via `diagnostics_reader`.
    Rust(Format),
}

/// A common format for representing log messages across different interfaces during tests.
#[derive(Debug, Clone)]
pub struct TestLogMessage {
    /// The component moniker tag associated with the log message.
    pub moniker_tag: String,
    /// The actual log message content.
    pub message: String,
    /// Tags associated with the log message.
    pub tags: Vec<String>,
    /// Severity of the log message.
    pub severity: Severity,
    /// Log message properties
    pub properties: Vec<LogsProperty>,
}

/// Abstract log reader used for reading logs into a common format
/// The methods are intentionally named differently from those on
/// LogReader to disambiguate them at call sites.
#[async_trait::async_trait]
pub trait LogReader {
    /// Gets a single snapshot of logs and completes.
    async fn get_test_snapshot(&self) -> Vec<TestLogMessage>;
    /// Gets a snapshot of current logs and then subscribes to new log messages.
    async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage>;
    /// Changes the retry config for protocols that implement retries for snapshots
    fn retry_config(&mut self, _config: RetryConfig) {}
    /// Selects all logs by component moniker for protocols that support selectors
    fn select_all_by_component(&mut self, _selector: &str) {}
}

// The FFI interface, which uses the FXT format, is currently only available at HEAD,
// so our tests, for now, only run at HEAD.
#[cfg(fuchsia_api_level_at_least = "HEAD")]
pub mod ffi_format {

    use crate::logs::common::rust_format::data_logs_to_test_logs;

    use super::*;
    use diagnostics_data::{LogsField, Severity};
    use diagnostics_message::MessageParser;
    use diagnostics_reader::{ToComponentSelectorArguments, ToSelectorArguments};
    use fidl_fuchsia_diagnostics::{
        BatchIteratorMarker, BatchIteratorProxy, ClientSelectorConfiguration, DataType,
        FormattedContent, StreamMode, StreamParameters,
    };
    use log_command_ctf::fxt_streamer::FxtStreamer;
    /// Internal implementation of LogReader using the FFI interfaces.
    pub struct FfiReader {
        accessor: ArchiveAccessorProxy,
        selector: Option<String>,
    }

    /// Streams log messages from a BatchIterator and parses them into `TestLogMessage`.
    fn stream_messages(
        iterator: BatchIteratorProxy,
        parser: MessageParser,
    ) -> BoxStream<'static, TestLogMessage> {
        let state = (iterator, parser, Vec::new());
        futures::stream::unfold(state, |(iterator, mut parser, mut pending)| async move {
            loop {
                if !pending.is_empty() {
                    let msg = pending.remove(0);
                    return Some((msg, (iterator, parser, pending)));
                }
                let batch = match iterator.get_next().await {
                    Ok(Ok(b)) => b,
                    _ => return None,
                };
                if batch.is_empty() {
                    return None;
                }
                for content in batch {
                    match content {
                        FormattedContent::Fxt(vmo) => {
                            let size = vmo.get_content_size().unwrap() as usize;
                            let mut buf = vec![0u8; size];
                            vmo.read(&mut buf, 0).unwrap();

                            let allocator = bumpalo::Bump::new();
                            let formatter =
                                diagnostics_message::ffi::CPPMessageFormatter(&allocator);
                            let mut current_slice = &buf[..];
                            while !current_slice.is_empty() {
                                match parser.parse_next(current_slice, &formatter) {
                                    Ok((maybe_msg, remaining)) => {
                                        if let Some(msg) = maybe_msg {
                                            let moniker_tag = msg.moniker_tag.to_string();
                                            let mut message = msg.message.to_string();
                                            let mut properties = vec![];
                                            for kvp in msg.kvps.iter() {
                                                let key_str = kvp.key.to_string();
                                                if key_str == "rolled_out" {
                                                    if let diagnostics_message::ffi::CppValue::UnsignedInt(count) = kvp.value {
                                                        message = format!("rolled_out={count}");
                                                    }
                                                    continue;
                                                }
                                                let name = LogsField::Other(key_str);
                                                let property = match &kvp.value {
                                                    diagnostics_message::ffi::CppValue::SignedInt(v) => LogsProperty::Int(name, *v),
                                                    diagnostics_message::ffi::CppValue::UnsignedInt(v) => LogsProperty::Uint(name, *v),
                                                    diagnostics_message::ffi::CppValue::Floating(v) => LogsProperty::Double(name, *v),
                                                    diagnostics_message::ffi::CppValue::Boolean(v) => LogsProperty::Bool(name, *v),
                                                    diagnostics_message::ffi::CppValue::Text(v) => LogsProperty::String(name, v.to_string()),
                                                };
                                                properties.push(property);
                                            }
                                            let (_, severity) = Severity::parse_exact(msg.severity);
                                            let tags =
                                                msg.tags.iter().map(|s| s.to_string()).collect();
                                            pending.push(TestLogMessage {
                                                moniker_tag,
                                                message,
                                                tags,
                                                severity,
                                                properties,
                                            });
                                        }
                                        current_slice = remaining;
                                    }
                                    Err(_) => {
                                        break;
                                    }
                                }
                            }
                        }
                        _ => panic!("Expected FXT content"),
                    }
                }
            }
        })
        .boxed()
    }

    /// Retrieves a complete snapshot of all logs currently available from
    /// the given `ArchiveAccessorProxy`.
    async fn get_snapshot(accessor: &ArchiveAccessorProxy) -> Vec<TestLogMessage> {
        let (iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
        accessor
            .stream_diagnostics(
                &StreamParameters {
                    data_type: Some(DataType::Logs),
                    stream_mode: Some(StreamMode::Snapshot),
                    format: Some(Format::Fxt),
                    client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(
                        true,
                    )),
                    ..Default::default()
                },
                server_end,
            )
            .unwrap();
        let mut stream = stream_messages(iterator.clone(), MessageParser::default());
        let mut messages = Vec::new();
        while let Some(msg) = stream.next().await {
            messages.push(msg);
        }
        messages
    }

    pub struct HostReaderFxt {
        pub reader: HostArchiveAccessor,
    }

    pub struct StreamArchiveAccessorFxt(pub StreamArchiveAccessor);

    #[async_trait::async_trait]
    impl LogReader for StreamArchiveAccessorFxt {
        async fn get_test_snapshot(&self) -> Vec<TestLogMessage> {
            let (local, remote) = zx::Socket::create_stream();
            let reader = fuchsia_async::Socket::from_socket(local);
            self.0
                .0
                .stream_diagnostics_to_socket(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::Snapshot),
                        format: Some(Format::Fxt),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::SelectAll(true),
                        ),
                        ..Default::default()
                    },
                    remote,
                )
                .await
                .unwrap();
            FxtStreamer::new(reader)
                .stream()
                .map(|value| data_logs_to_test_logs(value.unwrap()))
                .collect::<Vec<_>>()
                .await
        }

        async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage> {
            let (local, remote) = zx::Socket::create_stream();
            let reader = fuchsia_async::Socket::from_socket(local);
            self.0
                .0
                .stream_diagnostics_to_socket(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::SnapshotThenSubscribe),
                        format: Some(Format::Fxt),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::SelectAll(true),
                        ),
                        ..Default::default()
                    },
                    remote,
                )
                .await
                .unwrap();
            FxtStreamer::new(reader)
                .stream()
                .map(|value| data_logs_to_test_logs(value.unwrap()))
                .boxed()
        }
    }

    #[async_trait::async_trait]
    impl LogReader for StreamArchiveAccessor {
        async fn get_test_snapshot(&self) -> Vec<TestLogMessage> {
            let reader =
                initialize_socket_accessor(&self.0, Format::Json, StreamMode::Snapshot).await;
            LogsDataStream::new(reader)
                .map(|value| data_logs_to_test_logs(value.unwrap()))
                .collect::<Vec<_>>()
                .await
        }

        async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage> {
            let reader = initialize_socket_accessor(
                &self.0,
                Format::Json,
                StreamMode::SnapshotThenSubscribe,
            )
            .await;
            LogsDataStream::new(reader).map(|value| data_logs_to_test_logs(value.unwrap())).boxed()
        }
    }

    async fn initialize_socket_accessor(
        proxy: &ArchiveAccessorProxy,
        format: Format,
        mode: StreamMode,
    ) -> fuchsia_async::Socket {
        let (local, remote) = zx::Socket::create_stream();
        let reader = fuchsia_async::Socket::from_socket(local);
        proxy
            .stream_diagnostics_to_socket(
                &StreamParameters {
                    data_type: Some(DataType::Logs),
                    stream_mode: Some(mode),
                    format: Some(format),
                    client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(
                        true,
                    )),
                    ..Default::default()
                },
                remote,
            )
            .await
            .unwrap();
        reader
    }

    #[async_trait::async_trait]
    impl LogReader for HostReaderFxt {
        async fn get_test_snapshot(&self) -> Vec<TestLogMessage> {
            let (local, remote) = zx::Socket::create_stream();
            let reader = fuchsia_async::Socket::from_socket(local);
            self.reader
                .stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::Snapshot),
                        format: Some(Format::Fxt),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::SelectAll(true),
                        ),
                        ..Default::default()
                    },
                    remote,
                )
                .await
                .unwrap();
            FxtStreamer::new(reader)
                .stream()
                .map(|value| rust_format::data_logs_to_test_logs(value.unwrap()))
                .collect::<Vec<_>>()
                .await
        }

        async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage> {
            let (local, remote) = zx::Socket::create_stream();
            let reader = fuchsia_async::Socket::from_socket(local);
            self.reader
                .stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::SnapshotThenSubscribe),
                        format: Some(Format::Fxt),
                        client_selector_configuration: Some(
                            ClientSelectorConfiguration::SelectAll(true),
                        ),
                        ..Default::default()
                    },
                    remote,
                )
                .await
                .unwrap();
            FxtStreamer::new(reader)
                .stream()
                .map(|value| rust_format::data_logs_to_test_logs(value.unwrap()))
                .boxed()
        }
    }

    impl FfiReader {
        /// Creates a new FfiReader from the provided `ArchiveAccessorProxy`.
        pub fn new(accessor: ArchiveAccessorProxy) -> Self {
            Self { accessor, selector: None }
        }
    }

    #[async_trait::async_trait]
    impl LogReader for FfiReader {
        async fn get_test_snapshot(&self) -> Vec<TestLogMessage> {
            get_snapshot(&self.accessor).await
        }

        fn select_all_by_component(&mut self, selector: &str) {
            self.selector = Some(selector.to_string());
        }

        async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage> {
            let (iterator, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
            self.accessor
                .stream_diagnostics(
                    &StreamParameters {
                        data_type: Some(DataType::Logs),
                        stream_mode: Some(StreamMode::SnapshotThenSubscribe),
                        format: Some(Format::Fxt),
                        client_selector_configuration: Some(
                            if let Some(selector) = &self.selector {
                                ClientSelectorConfiguration::Selectors(
                                    selector
                                        .clone()
                                        .to_component_selector_arguments()
                                        .to_selector_arguments()
                                        .collect::<Vec<_>>(),
                                )
                            } else {
                                ClientSelectorConfiguration::SelectAll(true)
                            },
                        ),
                        ..Default::default()
                    },
                    server_end,
                )
                .unwrap();
            stream_messages(iterator, MessageParser::default())
        }
    }

    pub struct StreamArchiveAccessor(pub ArchiveAccessorProxy);

    impl LogProtocol for StreamArchiveAccessor {
        fn build(self, format: LogFormat) -> Box<dyn LogReader> {
            match format {
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                LogFormat::Rust(Format::Fxt) => Box::new(new_host_fxt_stream(self)),
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                LogFormat::Rust(Format::Json) => Box::new(new_host_reader_stream(self)),
                _ => unreachable!("Only FXT and JSON formats are supported for host"),
            }
        }
    }
}

mod rust_format {
    use super::*;

    #[async_trait::async_trait]
    impl LogReader for ArchiveReader<Logs> {
        async fn get_test_snapshot(&self) -> Vec<TestLogMessage> {
            self.snapshot()
                .await
                .map(|value| value.into_iter().map(data_logs_to_test_logs).collect::<Vec<_>>())
                .unwrap_or_default()
        }

        fn select_all_by_component(&mut self, selector: &str) {
            self.select_all_for_component(selector);
        }

        fn retry_config(&mut self, config: RetryConfig) {
            self.retry(config);
        }

        async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage> {
            Box::pin(self.snapshot_then_subscribe().unwrap().map(|value| {
                let log = value.unwrap();
                data_logs_to_test_logs(log)
            }))
        }
    }

    impl LogProtocol for ArchiveAccessorProxy {
        fn build(self, format: LogFormat) -> Box<dyn LogReader> {
            match format {
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                LogFormat::Ffi => Box::new(new_ffi_reader(self)),
                LogFormat::Rust(format) => {
                    let mut r1 = ArchiveReader::logs();
                    r1.with_archive(self)
                        .with_format(format)
                        .with_minimum_schema_count(0) // we want this to return even when
                        //no log messages
                        .retry(RetryConfig::never());
                    Box::new(new_reader(r1))
                }
            }
        }
    }

    /// Converts a standard `Data<Logs>` value into a `TestLogMessage`. This function is used
    /// by the `LogFormat::Rust` readers, which can read either JSON or legacy FXT formats.
    /// Rolled-out logs are represented as a message indicating the count.
    pub fn data_logs_to_test_logs(log: Data<Logs>) -> TestLogMessage {
        let message = if let Some(count) = log.rolled_out_logs() {
            format!("rolled_out={count}")
        } else {
            log.msg().unwrap_or("").to_string()
        };

        let moniker_tag = log.moniker.as_ref().split('/').next_back().unwrap_or("").to_string();
        let tags = log.tags().cloned().unwrap_or_default();
        let properties = log.payload_keys().map(|p| p.properties.clone()).unwrap_or_default();
        let severity = log.severity();
        TestLogMessage { moniker_tag, message, tags, severity, properties }
    }
}

/// Creates a new log reader that uses the FFI (FXT) interface under the hood.
#[cfg(fuchsia_api_level_at_least = "HEAD")]
fn new_ffi_reader(accessor: ArchiveAccessorProxy) -> impl LogReader {
    ffi_format::FfiReader::new(accessor)
}

/// Creates a new log reader that uses the standard Rust diagnostics_reader.
fn new_reader(reader: ArchiveReader<Logs>) -> impl LogReader {
    reader
}

/// Creates a new log reader that uses the ffx host reader
fn new_host_reader(reader: HostArchiveAccessor) -> impl LogReader {
    reader
}

/// Creates a new log reader that uses the fxt host reader
#[cfg(fuchsia_api_level_at_least = "HEAD")]
fn new_host_fxt(reader: HostArchiveAccessor) -> impl LogReader {
    ffi_format::HostReaderFxt { reader }
}

/// Creates a new log reader that uses the ffx host reader
#[cfg(fuchsia_api_level_at_least = "HEAD")]
fn new_host_reader_stream(reader: ffi_format::StreamArchiveAccessor) -> impl LogReader {
    reader
}

/// Creates a new log reader that uses the ffx host reader
#[cfg(fuchsia_api_level_at_least = "HEAD")]
fn new_host_fxt_stream(reader: ffi_format::StreamArchiveAccessor) -> impl LogReader {
    ffi_format::StreamArchiveAccessorFxt(reader)
}

impl LogProtocol for HostArchiveAccessor {
    fn build(self, format: LogFormat) -> Box<dyn LogReader> {
        match format {
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            LogFormat::Rust(Format::Fxt) => Box::new(new_host_fxt(self)),
            LogFormat::Rust(Format::Json) => Box::new(new_host_reader(self)),
            _ => unreachable!("Only FXT and JSON formats are supported for HostArchiveAccessor"),
        }
    }
}

pub trait LogProtocol {
    fn build(self, format: LogFormat) -> Box<dyn LogReader>;
}

impl LogFormat {
    /// Builds a `MultiFormatLogReader` using the specific format configured in this enum.
    pub fn build(self, accessor: impl LogProtocol) -> Box<dyn LogReader> {
        accessor.build(self)
    }
}

async fn initialize_socket(
    proxy: &HostArchiveAccessor,
    format: Format,
    mode: StreamMode,
) -> fuchsia_async::Socket {
    let (local, remote) = zx::Socket::create_stream();
    let reader = fuchsia_async::Socket::from_socket(local);
    proxy
        .stream_diagnostics(
            &StreamParameters {
                data_type: Some(DataType::Logs),
                stream_mode: Some(mode),
                format: Some(format),
                client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(true)),
                ..Default::default()
            },
            remote,
        )
        .await
        .unwrap();
    reader
}

#[async_trait::async_trait]
impl LogReader for HostArchiveAccessor {
    async fn get_test_snapshot(&self) -> Vec<TestLogMessage> {
        let reader = initialize_socket(self, Format::Json, StreamMode::Snapshot).await;
        LogsDataStream::new(reader)
            .map(|value| rust_format::data_logs_to_test_logs(value.unwrap()))
            .collect::<Vec<_>>()
            .await
    }

    async fn get_test_snapshot_then_subscribe(&self) -> BoxStream<'static, TestLogMessage> {
        let reader = initialize_socket(self, Format::Json, StreamMode::SnapshotThenSubscribe).await;
        LogsDataStream::new(reader)
            .map(|value| rust_format::data_logs_to_test_logs(value.unwrap()))
            .boxed()
    }
}
