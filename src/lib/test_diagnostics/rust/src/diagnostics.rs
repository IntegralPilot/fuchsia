// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use diagnostics_data::LogsData;
use flex_fuchsia_test_manager as ftest_manager;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
#[cfg(not(feature = "fdomain"))]
use log_command::LogsDataStream;
#[cfg(feature = "fdomain")]
use log_command_fdomain::LogsDataStream;
use pin_project::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(target_os = "fuchsia")]
use crate::diagnostics::fuchsia::BatchLogStream;

#[pin_project]
pub struct LogStream {
    #[pin]
    stream: BoxStream<'static, Result<LogsData, Error>>,
}

impl LogStream {
    fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<LogsData, Error>> + Send + 'static,
    {
        Self { stream: stream.boxed() }
    }

    pub fn from_syslog(syslog: ftest_manager::Syslog) -> Result<LogStream> {
        get_log_stream(syslog)
    }
}

impl Stream for LogStream {
    type Item = Result<LogsData, Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        this.stream.poll_next(cx)
    }
}

#[cfg(target_os = "fuchsia")]
fn get_log_stream(syslog: ftest_manager::Syslog) -> Result<LogStream> {
    match syslog {
        ftest_manager::Syslog::Batch(client_end) => {
            Ok(LogStream::new(BatchLogStream::from_client_end(client_end)?))
        }
        ftest_manager::Syslog::Stream(client_end) => Ok(LogStream::new(
            LogsDataStream::new(flex_client::socket_to_async(client_end))
                .map(|result| result.map_err(Error::from)),
        )),
        _ => {
            panic!("not supported")
        }
    }
}

#[cfg(not(target_os = "fuchsia"))]
fn get_log_stream(syslog: ftest_manager::Syslog) -> Result<LogStream> {
    match syslog {
        ftest_manager::Syslog::Stream(client_end) => Ok(LogStream::new(
            LogsDataStream::new(flex_client::socket_to_async(client_end))
                .map(|result| result.map_err(Error::from)),
        )),
        ftest_manager::Syslog::Batch(_) => panic!("batch iterator not supported on host"),
        _ => {
            panic!("not supported")
        }
    }
}

#[cfg(target_os = "fuchsia")]
mod fuchsia {
    use super::*;
    use diagnostics_reader::Subscription;
    use fidl::endpoints::ClientEnd;
    use fidl_fuchsia_diagnostics::BatchIteratorMarker;

    #[pin_project]
    pub struct BatchLogStream {
        #[pin]
        subscription: Subscription,
    }

    impl BatchLogStream {
        #[cfg(test)]
        pub fn new() -> Result<(Self, ftest_manager::LogsIterator), fidl::Error> {
            let (proxy, server_end) = fidl::endpoints::create_proxy::<BatchIteratorMarker>();
            let subscription = Subscription::new(proxy);
            Ok((Self { subscription }, ftest_manager::LogsIterator::Batch(server_end)))
        }

        pub fn from_client_end(
            client_end: ClientEnd<BatchIteratorMarker>,
        ) -> Result<Self, fidl::Error> {
            Ok(Self { subscription: Subscription::new(client_end.into_proxy()) })
        }
    }

    impl Stream for BatchLogStream {
        type Item = Result<LogsData, Error>;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.project();
            match this.subscription.poll_next(cx) {
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
                Poll::Ready(Some(Ok(value))) => Poll::Ready(Some(Ok(value))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_data::{ExtendedMoniker, Severity, Timestamp};
    use fuchsia_async as fasync;

    #[cfg(target_os = "fuchsia")]
    mod fuchsia {
        use super::*;
        use assert_matches::assert_matches;
        use fidl::endpoints::ServerEnd;
        use fidl_fuchsia_diagnostics::{
            BatchIteratorMarker, BatchIteratorRequest, FormattedContent, ReaderError,
        };
        use fidl_fuchsia_mem as fmem;
        use futures::TryStreamExt;

        fn create_log_stream() -> Result<(LogStream, ftest_manager::LogsIterator), fidl::Error> {
            let (stream, iterator) = BatchLogStream::new()?;
            Ok((LogStream::new(stream), iterator))
        }

        struct BatchIteratorOpts {
            with_error: bool,
        }

        /// Spanws a dummy batch iterator for testing that return 3 logs: "1", "2", "3" all with
        /// the same severity
        async fn spawn_batch_iterator_server(
            server_end: ServerEnd<BatchIteratorMarker>,
            opts: BatchIteratorOpts,
        ) {
            let mut request_stream = server_end.into_stream();
            let mut values = vec![1i64, 2, 3].into_iter();
            while let Some(request) = request_stream.try_next().await.expect("get next request") {
                match request {
                    BatchIteratorRequest::WaitForReady { responder } => {
                        responder.send().expect("sent response")
                    }
                    BatchIteratorRequest::GetNext { responder } => match values.next() {
                        None => {
                            responder.send(Ok(vec![])).expect("send empty response");
                        }
                        Some(value) => {
                            if opts.with_error {
                                responder.send(Err(ReaderError::Io)).expect("send error");
                                continue;
                            }
                            let content = get_json_data(value);
                            let size = content.len() as u64;
                            let vmo = zx::Vmo::create(size).expect("create vmo");
                            vmo.write(content.as_bytes(), 0).expect("write vmo");
                            let result = FormattedContent::Json(fmem::Buffer { vmo, size });
                            responder.send(Ok(vec![result])).expect("send response");
                        }
                    },
                    BatchIteratorRequest::_UnknownMethod { .. } => {
                        unreachable!("We aren't expecting any other call");
                    }
                }
            }
        }

        #[fasync::run_singlethreaded(test)]
        async fn log_stream_returns_logs() {
            let (mut log_stream, iterator) = create_log_stream().expect("got log stream");
            let server_end = match iterator {
                ftest_manager::LogsIterator::Batch(server_end) => server_end,
                _ => panic!("unexpected logs iterator server end"),
            };
            fasync::Task::spawn(spawn_batch_iterator_server(
                server_end,
                BatchIteratorOpts { with_error: false },
            ))
            .detach();
            assert_eq!(log_stream.next().await.unwrap().expect("got ok result").msg(), Some("1"));
            assert_eq!(log_stream.next().await.unwrap().expect("got ok result").msg(), Some("2"));
            assert_eq!(log_stream.next().await.unwrap().expect("got ok result").msg(), Some("3"));
            assert_matches!(log_stream.next().await, None);
        }

        #[fasync::run_singlethreaded(test)]
        async fn log_stream_can_return_errors() {
            let (mut log_stream, iterator) = create_log_stream().expect("got log stream");
            let server_end = match iterator {
                ftest_manager::LogsIterator::Batch(server_end) => server_end,
                _ => panic!("unexpected logs iterator server end"),
            };
            fasync::Task::spawn(spawn_batch_iterator_server(
                server_end,
                BatchIteratorOpts { with_error: true },
            ))
            .detach();
            assert_matches!(log_stream.next().await, Some(Err(_)));
        }
    }

    #[cfg(not(target_os = "fuchsia"))]
    mod host {
        use super::*;
        use assert_matches::assert_matches;
        use futures::AsyncWriteExt;

        fn create_log_stream(
            client: flex_client::ClientArg,
        ) -> Result<(LogStream, ftest_manager::LogsIterator), fidl::Error> {
            let (client_end, server_end) = client.create_stream_socket();
            let (stream, iterator) = (
                LogStream::new(
                    LogsDataStream::new(flex_client::socket_to_async(client_end))
                        .map(|result| result.map_err(Error::from)),
                ),
                ftest_manager::LogsIterator::Stream(server_end),
            );
            Ok((LogStream::new(stream), iterator))
        }

        async fn spawn_archive_iterator_server(socket: flex_client::Socket, with_error: bool) {
            let mut socket = flex_client::socket_to_async(socket);
            let mut values = vec![1, 2, 3].into_iter();
            loop {
                match values.next() {
                    None => {
                        // End of stream, close socket.
                        break;
                    }
                    Some(value) => {
                        if with_error {
                            // Send invalid JSON to trigger an error other than
                            // ZX_ERR_PEER_CLOSED
                            let _ = socket.write_all("5".as_bytes()).await;
                            continue;
                        }
                        socket.write_all(get_json_data(value).as_bytes()).await.unwrap();
                    }
                }
            }
        }

        async fn archive_stream_returns_logs(client: flex_client::ClientArg) {
            let (mut log_stream, iterator) = create_log_stream(client).expect("got log stream");
            let server_end = match iterator {
                ftest_manager::LogsIterator::Stream(server_end) => server_end,
                _ => panic!("unexpected logs iterator server end"),
            };
            fasync::Task::spawn(spawn_archive_iterator_server(server_end, false)).detach();
            assert_eq!(log_stream.next().await.unwrap().expect("got ok result").msg(), Some("1"));
            assert_eq!(log_stream.next().await.unwrap().expect("got ok result").msg(), Some("2"));
            assert_eq!(log_stream.next().await.unwrap().expect("got ok result").msg(), Some("3"));
            assert_matches!(log_stream.next().await, None);
        }

        #[fasync::run_singlethreaded(test)]
        async fn archive_stream_returns_logs_inline() {
            let client = fdomain_local::local_client_empty();
            archive_stream_returns_logs(client.clone()).await;
        }

        async fn archive_stream_can_return_errors(client: flex_client::ClientArg) {
            let (mut log_stream, iterator) = create_log_stream(client).expect("got log stream");
            let server_end = match iterator {
                ftest_manager::LogsIterator::Stream(server_end) => server_end,
                _ => panic!("unexpected logs iterator server end"),
            };
            fasync::Task::spawn(spawn_archive_iterator_server(server_end, true)).detach();
            assert_matches!(log_stream.next().await, Some(Err(_)));
        }

        #[fasync::run_singlethreaded(test)]
        async fn archive_stream_can_return_errors_inline() {
            let client = fdomain_local::local_client_empty();
            archive_stream_can_return_errors(client.clone()).await;
        }
    }

    fn get_json_data(value: i64) -> String {
        let data = diagnostics_data::LogsDataBuilder::new(diagnostics_data::BuilderArgs {
            timestamp: Timestamp::from_nanos(0),
            component_url: Some("fake-url".into()),
            moniker: ExtendedMoniker::parse_str("test/moniker").unwrap(),
            severity: Severity::Info,
        })
        .set_message(value.to_string())
        .build();

        serde_json::to_string(&data).expect("serialize to json")
    }
}
