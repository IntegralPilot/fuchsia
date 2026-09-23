// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fuchsia_async::TimeoutExt as _;
use fuchsia_inspect as finspect;
use fuchsia_inspect::Property as _;
use futures::future::TryFutureExt as _;
use futures::stream::{Stream, StreamExt as _, TryStreamExt as _};
use http_body_util::BodyStream;
use hyper::body::Body as _;
use hyper::{Request, StatusCode};
use log::warn;
use std::convert::TryInto as _;
use std::str::FromStr;

/// Configuration for `resuming_get` to avoid having multiple parameters with the same type.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    pub(crate) header_timeout: zx::BootDuration,
    pub(crate) body_timeout: zx::BootDuration,
    pub(crate) resumption_attempt_limit: u32,
}

/// On success, returns the Content-Length of the resource, as determined by the first GET, and the
/// Body as a Stream of chunks. All of the chunks will be non-empty.
pub(crate) async fn resuming_get<'a>(
    client: &'a fuchsia_hyper::HttpsClient,
    uri: http::Uri,
    params: Params,
    inspect: finspect::Node,
) -> Result<
    (u64, impl Stream<Item = Result<hyper::body::Bytes, ResumingGetError>> + 'a),
    ResumingGetError,
> {
    let request = Request::get(&uri)
        .body(http_body_util::Full::default())
        .map_err(ResumingGetError::CreateHttpRequest)?;
    let response = client
        .request(request)
        .map_err(ResumingGetError::SendHttpRequest)
        .on_timeout(params.header_timeout, || Err(ResumingGetError::HeaderTimeout))
        .await?;

    if response.status() != StatusCode::OK {
        return Err(ResumingGetError::StatusNotOk(response.status()));
    }

    let expected_len = response
        .size_hint()
        .exact()
        .ok_or_else(|| ResumingGetError::UnknownBodySize(response.size_hint()))?;

    let chunks = async_generator::generate(move |mut co| {
        async move {
            let mut resumptions = 0;
            let mut bytes_downloaded = 0;
            let mut progress_this_attempt = false;
            let mut chunks = BodyStream::new(response.into_body())
                .filter_map(
                    |res: Result<hyper::body::Frame<hyper::body::Bytes>, hyper::Error>| async move {
                        match res {
                            Ok(frame) => frame.into_data().ok().map(Ok),
                            Err(e) => Some(Err(e)),
                        }
                    },
                )
                .boxed();
            let inspect_resumptions = inspect.create_uint("resumptions", 0);
            let inspect_first_byte_pos = inspect.create_uint("first-byte-pos", 0);
            while bytes_downloaded < expected_len {
                match chunks
                    .try_next()
                    .map_err(ResumingGetError::NextBodyChunk)
                    .on_timeout(params.body_timeout, || Err(ResumingGetError::BodyTimeout))
                    .await
                {
                    Ok(Some(chunk)) => {
                        if !chunk.is_empty() {
                            progress_this_attempt = true;
                            bytes_downloaded += chunk.len() as u64;
                            co.yield_(chunk).await;
                        }
                    }
                    Ok(None) => break,
                    Err(e) if progress_this_attempt => {
                        // It should be impossible to infinite loop on resumption requests because
                        // we only attempt to resume when progress was made on the previous GET.
                        // This is a backstop in case of a logic bug.
                        if resumptions >= params.resumption_attempt_limit.into() {
                            return Err(ResumingGetError::ResumptionLimitHit {
                                limit: params.resumption_attempt_limit,
                                previous_error: anyhow!(e),
                            });
                        }
                        warn!(
                            resumptions,
                            bytes_downloaded,
                            expected_len;
                            "Resuming failed blob GET after partial success: {:#}",
                            anyhow!(e),
                        );
                        progress_this_attempt = false;
                        resumptions += 1;
                        inspect_resumptions.set(resumptions);

                        let first_byte_pos = bytes_downloaded;
                        // This will never overflow, because expected_len is immutable and if
                        // expected_len is 0 then this while loop can never be entered.
                        let last_byte_pos = expected_len - 1;
                        let request = Request::get(&uri)
                            .header(
                                http::header::RANGE,
                                format!("bytes={first_byte_pos}-{last_byte_pos}"),
                            )
                            .body(http_body_util::Full::default())
                            .map_err(ResumingGetError::CreateHttpResumeRequest)?;

                        let response = client
                            .request(request)
                            .map_err(ResumingGetError::SendHttpResumeRequest)
                            .on_timeout(params.header_timeout, || {
                                Err(ResumingGetError::ResumeHeaderTimeout)
                            })
                            .await?;

                        if response.status() != StatusCode::PARTIAL_CONTENT {
                            return Err(ResumingGetError::StatusNotPartialContent {
                                status: response.status(),
                                first_byte_pos,
                                last_byte_pos,
                            });
                        }

                        let Some(content_range) =
                            response.headers().get(http::header::CONTENT_RANGE)
                        else {
                            return Err(ResumingGetError::MissingContentRangeHeader {
                                first_byte_pos,
                                last_byte_pos,
                            });
                        };

                        let content_range = content_range.try_into().map_err(|source| {
                            ResumingGetError::MalformedContentRangeHeader {
                                source,
                                first_byte_pos,
                                last_byte_pos,
                                content_range: content_range.clone(),
                            }
                        })?;

                        let expected = HttpContentRange {
                            range: Some((first_byte_pos, last_byte_pos)),
                            len: Some(expected_len),
                        };
                        if content_range != expected {
                            return Err(ResumingGetError::UnexpectedContentRangeHeader {
                                expected,
                                actual: content_range,
                            });
                        }

                        if let Some(content_length) = response.body().size_hint().exact() {
                            if content_length != 1 + last_byte_pos - first_byte_pos {
                                return Err(ResumingGetError::ContentLengthContentRangeMismatch {
                                    content_length,
                                    first_byte_pos,
                                    last_byte_pos,
                                });
                            }
                        }
                        chunks = BodyStream::new(response.into_body())
                            .filter_map(
                                |res: Result<
                                    hyper::body::Frame<hyper::body::Bytes>,
                                    hyper::Error,
                                >| async move {
                                    match res {
                                        Ok(frame) => frame.into_data().ok().map(Ok),
                                        Err(e) => Some(Err(e)),
                                    }
                                },
                            )
                            .boxed();
                        inspect_first_byte_pos.set(first_byte_pos);
                    }
                    // !progress_this_attempt
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            Ok(())
        }
    });

    Ok((expected_len, chunks.into_try_stream()))
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum ResumingGetError {
    #[error("creating http request")]
    CreateHttpRequest(#[source] hyper::http::Error),

    #[error("sending http request")]
    SendHttpRequest(#[source] hyper_util::client::legacy::Error),

    #[error(
        // LINT.IfChange(blob_header_timeout)
        "timed out waiting for http response header while downloading blob"
        // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go:blob_header_timeout)
    )]
    HeaderTimeout,

    #[error("status not ok: {0}")]
    StatusNotOk(http::StatusCode),

    #[error("unknown body size: {0:?}")]
    UnknownBodySize(hyper::body::SizeHint),

    #[error("next body chunk")]
    NextBodyChunk(#[source] hyper::Error),

    #[error("body timeout")]
    BodyTimeout,

    #[error("resumption limit of {limit} hit, previous error: {previous_error:#}")]
    ResumptionLimitHit { limit: u32, previous_error: anyhow::Error },

    #[error("creating http resume request")]
    CreateHttpResumeRequest(#[source] hyper::http::Error),

    #[error("sending http resume request")]
    SendHttpResumeRequest(#[source] hyper_util::client::legacy::Error),

    #[error("resume header timeout")]
    ResumeHeaderTimeout,

    #[error(
        "status not PARTIAL_CONTENT {status} \
             first_byte_pos {first_byte_pos} last_byte_pos {last_byte_pos}"
    )]
    StatusNotPartialContent { status: http::StatusCode, first_byte_pos: u64, last_byte_pos: u64 },

    #[error(
        "missing content range header: \
             first_byte_pos {first_byte_pos} last_byte_pos {last_byte_pos}"
    )]
    MissingContentRangeHeader { first_byte_pos: u64, last_byte_pos: u64 },

    #[error(
        "malformed content range header: \
             first_byte_pos {first_byte_pos} last_byte_pos {last_byte_pos} \
             content_range {content_range:?}"
    )]
    MalformedContentRangeHeader {
        source: ContentRangeParseError,
        first_byte_pos: u64,
        last_byte_pos: u64,
        content_range: http::HeaderValue,
    },

    #[error("unexpected content range header: expected {expected:?} actual {actual:?}")]
    UnexpectedContentRangeHeader { expected: HttpContentRange, actual: HttpContentRange },

    #[error(
        "content length content range mismatch: content_length {content_length} \
             first_byte_pos {first_byte_pos} last_byte_pos {last_byte_pos}"
    )]
    ContentLengthContentRangeMismatch {
        content_length: u64,
        first_byte_pos: u64,
        last_byte_pos: u64,
    },
}

/// An http Content-Range header, e.g. "bytes 0-499/1234"
/// https://tools.ietf.org/html/rfc7233#section-4.2
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HttpContentRange {
    range: Option<(u64, u64)>,
    len: Option<u64>,
}

fn split_pair(s: &str, sep: char) -> Option<(&str, &str)> {
    let mut iter = s.splitn(2, sep).fuse();
    match (iter.next(), iter.next()) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub(crate) enum ContentRangeParseError {
    #[error("complete length")]
    CompleteLength,
    #[error("first byte position")]
    FirstBytePos,
    #[error("last byte position")]
    LastBytePos,
    #[error("invalid byte range")]
    InvalidByteRange,
    #[error("malformed header")]
    MalformedHeader,
    #[error("unsupported range unit")]
    UnsupportedRangeUnit,
    #[error("invalid utf8")]
    InvalidUtf8,
}

impl TryFrom<&http::HeaderValue> for HttpContentRange {
    type Error = ContentRangeParseError;
    fn try_from(value: &http::HeaderValue) -> Result<Self, Self::Error> {
        value.to_str().map_err(|_| ContentRangeParseError::InvalidUtf8)?.parse()
    }
}

impl FromStr for HttpContentRange {
    type Err = ContentRangeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match split_pair(s, ' ') {
            Some(("bytes", etc)) => {
                let (range, len) = match split_pair(etc, '/') {
                    Some(("*", "*")) => (None, None),
                    Some(("*", len)) => {
                        let len =
                            len.parse().map_err(|_| ContentRangeParseError::CompleteLength)?;
                        (None, Some(len))
                    }
                    Some((range, len)) => {
                        let range = match split_pair(range, '-') {
                            Some((a, b)) => {
                                let a =
                                    a.parse().map_err(|_| ContentRangeParseError::FirstBytePos)?;
                                let b =
                                    b.parse().map_err(|_| ContentRangeParseError::LastBytePos)?;
                                if b < a {
                                    return Err(ContentRangeParseError::InvalidByteRange);
                                }
                                Some((a, b))
                            }
                            _ => return Err(ContentRangeParseError::InvalidByteRange),
                        };
                        let len = if len == "*" {
                            None
                        } else {
                            Some(len.parse().map_err(|_| ContentRangeParseError::CompleteLength)?)
                        };
                        (range, len)
                    }
                    _ => return Err(ContentRangeParseError::MalformedHeader),
                };
                Ok(HttpContentRange { range, len })
            }
            _ => Err(ContentRangeParseError::UnsupportedRangeUnit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use fuchsia_hyper as fhyper;
    use std::sync::{Arc, Mutex};

    type TestBody = http_body_util::combinators::BoxBody<hyper::body::Bytes, std::io::Error>;
    fn stream_body(items: Vec<Result<Vec<u8>, String>>) -> TestBody {
        let stream = futures::stream::iter(items).map(|res| match res {
            Ok(bytes) => Ok(hyper::body::Frame::data(bytes.into())),
            Err(err) => Err(std::io::Error::new(std::io::ErrorKind::Other, err)),
        });
        http_body_util::BodyExt::boxed(http_body_util::StreamBody::new(Box::pin(stream)))
    }

    #[test]
    fn parse_http_content_range_success() {
        let cases = vec![
            ("bytes 42-1233/1234", Some((42, 1233)), Some(1234)),
            ("bytes 42-1233/*", Some((42, 1233)), None),
            ("bytes */1231", None, Some(1231)),
            ("bytes */*", None, None),
        ];

        for (input, range, len) in cases {
            assert_eq!(HttpContentRange::from_str(input).unwrap(), HttpContentRange { range, len });
        }
    }

    #[test]
    fn parse_http_content_range_failure() {
        use ContentRangeParseError::*;
        let cases = vec![
            ("", UnsupportedRangeUnit),
            ("nonunit 52-1233/1234", UnsupportedRangeUnit),
            ("byte", UnsupportedRangeUnit),
            ("byte ", UnsupportedRangeUnit),
            ("bytes", UnsupportedRangeUnit),
            ("bytes ", MalformedHeader),
            ("bytes invalid", MalformedHeader),
            ("bytes 1", MalformedHeader),
            ("bytes 1-", MalformedHeader),
            ("bytes 1-2", MalformedHeader),
            ("bytes 1-2/", CompleteLength),
            ("bytes 1/", InvalidByteRange),
            ("bytes 1-/", LastBytePos),
            ("bytes /", InvalidByteRange),
            ("bytes -", MalformedHeader),
            ("bytes -/", FirstBytePos),
            ("bytes -1/", FirstBytePos),
            ("bytes 1-/2", LastBytePos),
            ("bytes a-b/c", FirstBytePos),
            ("bytes a-10/100", FirstBytePos),
            ("bytes 10-b/100", LastBytePos),
            ("bytes 10-100/c", CompleteLength),
            ("bytes 10-4/*", InvalidByteRange),
        ];

        for (input, error) in cases {
            assert_matches!(HttpContentRange::from_str(input), Err(e) if e == error);
        }
    }

    impl HttpContentRange {
        fn stringify(&self) -> String {
            let (first, last) = self.range.unwrap();
            format!("bytes {}-{}/{}", first, last, self.len.unwrap())
        }
    }

    // Returns (X, Y) from "bytes=X-Y"
    fn first_and_last_byte_pos_from_range_header(header: &http::HeaderValue) -> (usize, usize) {
        let mut vals = header.to_str().unwrap().strip_prefix("bytes=").unwrap().split("-");
        let first = vals.next().unwrap().parse().unwrap();
        let last = vals.next().unwrap().parse().unwrap();
        assert_eq!(vals.next(), None);
        (first, last)
    }

    // If the http response body is small enough, hyper will download the body along with the header
    // in the initial call to fuchsia_hyper::HttpsClient.request(). This means that even if the
    // server implements the response body as a stream that sends some bytes and then fails before
    // the transfer is complete, from the perspective of the user of the hyper client library (e.g.
    // resuming_get), the error will occur on the initial request instead of when iterating over the
    // response body Bytes stream.
    // Which is to say, if we want to test how resuming_get behaves when it encounters an error
    // while reading from the Body stream, the Body needs to be larger than this value in bytes.
    // This value probably just needs to be larger than the Hyper buffer, which defaults to 400 kB
    // https://docs.rs/hyper/latest/hyper/client/conn/http1/struct.Builder.html#method.max_buf_size
    const BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING: usize = 600_000;

    // Takes an async fn that maps requests to responses and returns an http server and the address
    // the server is listening on.
    fn create_http_server<F>(
        handler: impl FnMut(hyper::Request<hyper::body::Incoming>) -> F + Send + Clone + 'static,
    ) -> (std::net::SocketAddr, fasync::Task<()>)
    where
        F: std::future::Future<
                Output = std::result::Result<hyper::Response<TestBody>, std::convert::Infallible>,
            >,
        F: Send + 'static,
    {
        let addr =
            std::net::SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0);
        let listener = fasync::net::TcpListener::bind(&addr).expect("bind");
        let server_addr = listener.local_addr().expect("local address");
        let handler = std::sync::Arc::new(std::sync::Mutex::new(handler));
        let server = fasync::Task::spawn(async move {
            let mut listener = listener;
            loop {
                match listener.accept().await {
                    Ok((next_listener, conn, _)) => {
                        listener = next_listener;
                        let io =
                            hyper_util::rt::TokioIo::new(fuchsia_hyper::TcpStream { stream: conn });
                        let handler = handler.clone();
                        let service = hyper::service::service_fn(move |req| {
                            let mut h = handler.lock().unwrap();
                            (h)(req)
                        });
                        fasync::Task::spawn(async move {
                            let _ = hyper_util::server::conn::auto::Builder::new(
                                fuchsia_hyper::Executor,
                            )
                            .serve_connection(io, service)
                            .await;
                        })
                        .detach();
                    }
                    Err(_) => break,
                }
            }
        });
        (server_addr, server)
    }

    // Tests that resuming_get will return a stream of the entire file contents, even if individual
    // HTTP GET requests fail with partial returns, by repeatedly using the HTTP RANGE header to
    // request the outstanding bytes.
    #[fuchsia::test]
    async fn resuming_get_resumes() {
        let requests = Arc::new(Mutex::new(vec![]));
        let requests_clone = requests.clone();
        let content: Vec<u8> = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
            .into_iter()
            .cycle()
            .take(BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING * 3)
            .collect();
        let content_clone = content.clone();
        // Responds to the first two requests by streaming enough body content to fill the hyper
        // client receive buffer and then erroring. Responds successfully to the third request with
        // the remainder of the content. Expects the first request to be a regular GET and the
        // second and third requests to have the RANGE header set.
        let handler = move |req: hyper::Request<hyper::body::Incoming>| {
            assert_eq!(req.method(), http::Method::GET);
            assert_eq!(req.uri().path(), "/the-file.txt");
            let range_header = req.headers().get(http::header::RANGE).cloned();
            let request_count = {
                let mut requests = requests.lock().unwrap();
                requests.push(range_header.clone());
                requests.len() - 1
            };
            let resp = match request_count {
                0 => hyper::Response::builder()
                    .status(http::StatusCode::OK)
                    .header(http::header::CONTENT_LENGTH, content.len())
                    .body(stream_body(vec![
                        Ok(content[..BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING].to_owned()),
                        Err("short return".to_owned()),
                    ]))
                    .unwrap(),
                1 => {
                    let first_byte_pos =
                        first_and_last_byte_pos_from_range_header(&range_header.unwrap()).0;
                    let content_range = HttpContentRange {
                        range: Some((first_byte_pos as u64, content.len() as u64 - 1)),
                        len: Some(content.len() as u64),
                    };
                    hyper::Response::builder()
                        .status(http::StatusCode::PARTIAL_CONTENT)
                        .header(
                            http::header::CONTENT_LENGTH,
                            (content.len() - first_byte_pos) as u64,
                        )
                        .header(http::header::CONTENT_RANGE, content_range.stringify())
                        .body(stream_body(vec![
                            Ok(content[first_byte_pos
                                ..first_byte_pos
                                    + BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING]
                                .to_vec()),
                            Err("short return".to_owned()),
                        ]))
                        .unwrap()
                }
                2 => {
                    let first_byte_pos =
                        first_and_last_byte_pos_from_range_header(&range_header.unwrap()).0;
                    let content_range = HttpContentRange {
                        range: Some((first_byte_pos as u64, content.len() as u64 - 1)),
                        len: Some(content.len() as u64),
                    };
                    hyper::Response::builder()
                        .status(http::StatusCode::PARTIAL_CONTENT)
                        .header(
                            http::header::CONTENT_LENGTH,
                            (content.len() - first_byte_pos) as u64,
                        )
                        .header(http::header::CONTENT_RANGE, content_range.stringify())
                        .body(stream_body(vec![Ok(content[first_byte_pos..].to_vec())]))
                        .unwrap()
                }
                _ => panic!("client should only resume twice"),
            };
            std::future::ready(Ok::<_, std::convert::Infallible>(resp))
        };

        let (server_addr, _server) = create_http_server(handler);

        let client = fhyper::new_https_client();
        let (len, content_stream) = resuming_get(
            &client,
            format!("http://{server_addr}/the-file.txt").parse().unwrap(),
            Params {
                header_timeout: zx::BootDuration::from_seconds(30),
                body_timeout: zx::BootDuration::from_seconds(30),
                resumption_attempt_limit: 10,
            },
            finspect::Node::default(),
        )
        .await
        .unwrap();

        assert_eq!(len, content_clone.len() as u64);

        let received_content = content_stream.map_ok(|b| b.to_vec()).try_concat().await.unwrap();
        assert_eq!(content_clone, received_content);

        let mut requests = requests_clone.lock().unwrap();
        assert!(requests.pop().unwrap().is_some()); // Third request sets RANGE.
        assert!(requests.pop().unwrap().is_some()); // Second request sets RANGE.
        assert!(requests.pop().unwrap().is_none()); // First request does not set RANGE.
        assert!(requests.is_empty()); // Only three requests were made.
    }

    // This test has a similar configuration to "resuming_get_resumes" (the server is configured to
    // partially respond to the first two HTTP GETs), except that resuming_get is given a resumption
    // limit of 1, so instead of resuming_get's returned stream completing successfully with three
    // HTTP GETs made, the stream will fail after the second HTTP GET fails.
    #[fuchsia::test]
    async fn resuming_get_respects_resumption_limit() {
        let requests = Arc::new(Mutex::new(vec![]));
        let requests_clone = requests.clone();
        let content: Vec<u8> = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
            .into_iter()
            .cycle()
            .take(BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING * 3)
            .collect();
        let content_clone = content.clone();
        // Responds to the first two requests by streaming enough body content to fill the hyper
        // client receive buffer and then erroring. Responds successfully to the third request with
        // the remainder of the content. Expects the first request to be a regular GET and the
        // second and third requests to have the RANGE header set.
        let handler = move |req: hyper::Request<hyper::body::Incoming>| {
            assert_eq!(req.method(), http::Method::GET);
            assert_eq!(req.uri().path(), "/the-file.txt");
            let range_header = req.headers().get(http::header::RANGE).cloned();
            let request_count = {
                let mut requests = requests.lock().unwrap();
                requests.push(range_header.clone());
                requests.len() - 1
            };
            let resp = match request_count {
                0 => hyper::Response::builder()
                    .status(http::StatusCode::OK)
                    .header(http::header::CONTENT_LENGTH, content.len())
                    .body(stream_body(vec![
                        Ok(content[..BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING].to_owned()),
                        Err("short return".to_owned()),
                    ]))
                    .unwrap(),
                1 => {
                    let first_byte_pos =
                        first_and_last_byte_pos_from_range_header(&range_header.unwrap()).0;
                    let content_range = HttpContentRange {
                        range: Some((first_byte_pos as u64, content.len() as u64 - 1)),
                        len: Some(content.len() as u64),
                    };
                    hyper::Response::builder()
                        .status(http::StatusCode::PARTIAL_CONTENT)
                        .header(
                            http::header::CONTENT_LENGTH,
                            (content.len() - first_byte_pos) as u64,
                        )
                        .header(http::header::CONTENT_RANGE, content_range.stringify())
                        .body(stream_body(vec![
                            Ok(content[first_byte_pos
                                ..first_byte_pos
                                    + BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING]
                                .to_vec()),
                            Err("short return".to_owned()),
                        ]))
                        .unwrap()
                }
                _ => panic!("client should only resume once"),
            };
            std::future::ready(Ok::<_, std::convert::Infallible>(resp))
        };

        let (server_addr, _server) = create_http_server(handler);

        let client = fhyper::new_https_client();
        let (len, content_stream) = resuming_get(
            &client,
            format!("http://{server_addr}/the-file.txt").parse().unwrap(),
            Params {
                header_timeout: zx::BootDuration::from_seconds(30),
                body_timeout: zx::BootDuration::from_seconds(30),
                resumption_attempt_limit: 1,
            },
            finspect::Node::default(),
        )
        .await
        .unwrap();

        assert_eq!(len, content_clone.len() as u64);

        assert_matches!(
            content_stream.map_ok(|b| b.to_vec()).try_concat().await,
            Err(ResumingGetError::ResumptionLimitHit{ limit, previous_error: _}) if limit == 1
        );

        let mut requests = requests_clone.lock().unwrap();
        assert!(requests.pop().unwrap().is_some()); // Second request sets RANGE.
        assert!(requests.pop().unwrap().is_none()); // First request does not set RANGE.
        assert!(requests.is_empty()); // Only two requests were made.
    }

    // This test has a similar configuration to "resuming_get_resumes" (the server will respond to
    // the first HTTP GET by streaming some of the body bytes and then failing), except that the
    // server will respond to resuming_get's second HTTP GET request with an immediate error, which
    // should cause resuming_get to terminate its returned stream with an error instead of making a
    // third HTTP GET request.
    #[fuchsia::test]
    async fn resuming_get_requires_forward_progress() {
        let requests = Arc::new(Mutex::new(vec![]));
        let requests_clone = requests.clone();
        let content: Vec<u8> = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
            .into_iter()
            .cycle()
            .take(BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING * 2)
            .collect();
        let content_clone = content.clone();
        // Responds to the first request by streaming enough body content to fill the hyper client
        // receive buffer and then erroring. Responds to the second request with an error. Expects
        // the first request to be a regular GET and the second request to have the RANGE header.
        let handler = move |req: hyper::Request<hyper::body::Incoming>| {
            assert_eq!(req.method(), http::Method::GET);
            assert_eq!(req.uri().path(), "/the-file.txt");
            let range_header = req.headers().get(http::header::RANGE).cloned();
            let request_count = {
                let mut requests = requests.lock().unwrap();
                requests.push(range_header.clone());
                requests.len() - 1
            };
            let resp = match request_count {
                0 => hyper::Response::builder()
                    .status(http::StatusCode::OK)
                    .header(http::header::CONTENT_LENGTH, content.len())
                    .body(stream_body(vec![
                        Ok(content[..BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING].to_owned()),
                        Err("short return".to_owned()),
                    ]))
                    .unwrap(),
                1 => {
                    let first_byte_pos =
                        first_and_last_byte_pos_from_range_header(&range_header.unwrap()).0;
                    let content_range = HttpContentRange {
                        range: Some((first_byte_pos as u64, content.len() as u64 - 1)),
                        len: Some(content.len() as u64),
                    };
                    hyper::Response::builder()
                        .status(http::StatusCode::PARTIAL_CONTENT)
                        .header(
                            http::header::CONTENT_LENGTH,
                            (content.len() - first_byte_pos) as u64,
                        )
                        .header(http::header::CONTENT_RANGE, content_range.stringify())
                        .body(stream_body(vec![Err("short return".to_owned())]))
                        .unwrap()
                }
                _ => panic!("client should only resume once"),
            };
            std::future::ready(Ok::<_, std::convert::Infallible>(resp))
        };

        let (server_addr, _server) = create_http_server(handler);

        let client = fhyper::new_https_client();
        let (len, content_stream) = resuming_get(
            &client,
            format!("http://{server_addr}/the-file.txt").parse().unwrap(),
            Params {
                header_timeout: zx::BootDuration::from_seconds(30),
                body_timeout: zx::BootDuration::from_seconds(30),
                resumption_attempt_limit: 1,
            },
            finspect::Node::default(),
        )
        .await
        .unwrap();

        assert_eq!(len, content_clone.len() as u64);

        assert_matches!(
            content_stream.map_ok(|b| b.to_vec()).try_concat().await,
            Err(ResumingGetError::SendHttpResumeRequest(_))
        );

        let mut requests = requests_clone.lock().unwrap();
        assert!(requests.pop().unwrap().is_some()); // Second request sets RANGE.
        assert!(requests.pop().unwrap().is_none()); // First request does not set RANGE.
        assert!(requests.is_empty()); // Only two requests were made.
    }
}
