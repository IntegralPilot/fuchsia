// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Event, EventSource};
#[cfg(target_os = "fuchsia")]
use fidl_fuchsia_net_http as fnet_http;
#[cfg(target_os = "fuchsia")]
use fuchsia_async as fasync;
use fuchsia_hyper::HttpsClient;
#[cfg(target_os = "fuchsia")]
use futures::io::AsyncReadExt as _;
use futures::stream::{Stream, StreamExt as _};
use futures::task::{Context, Poll};
use hyper::{Request, StatusCode};
use std::pin::Pin;
use thiserror::Error;

/// An http SSE client.
#[derive(derivative::Derivative)]
#[derivative(Debug)]
pub struct Client {
    #[derivative(Debug = "ignore")]
    chunks: futures::stream::BoxStream<'static, Result<hyper::body::Bytes, anyhow::Error>>,
    source: EventSource,
    events: std::vec::IntoIter<Event>,
}

impl Client {
    /// Connects to an http url and, on success, returns a `Stream` of SSE events.
    pub async fn from_hyper_client(
        https_client: &HttpsClient,
        url: impl AsRef<str>,
    ) -> Result<Self, FromHyperClientError> {
        let request = Request::get(url.as_ref())
            .header("accept", "text/event-stream")
            .body(http_body_util::Full::default())
            .map_err(|e| FromHyperClientError::CreateRequest(e))?;
        let response = https_client
            .request(request)
            .await
            .map_err(|e| FromHyperClientError::MakeRequest(e))?;
        if response.status() != StatusCode::OK {
            return Err(FromHyperClientError::HttpStatus(response.status()));
        }
        Ok(Self {
            chunks: http_body_util::BodyStream::new(response.into_body())
                .filter_map(|frame_res| async move {
                    match frame_res {
                        Ok(frame) => match frame.into_data() {
                            Ok(bytes) => Some(Ok(bytes)),
                            Err(_) => None,
                        },
                        Err(e) => Some(Err(anyhow::anyhow!(e))),
                    }
                })
                .boxed(),
            source: EventSource::new(),
            events: vec![].into_iter(),
        })
    }

    /// Connects to an http url and, on success, returns a `Stream` of SSE events.
    ///
    /// `stream_buf_size` is the size of the buffer to use when reading from the http response body.
    /// Consider making it a small multiple of the expected event size to optimize for low memory
    /// usage or a large multiple to optimize for event processing speed.
    #[cfg(target_os = "fuchsia")]
    pub async fn from_http_loader(
        loader: &fnet_http::LoaderProxy,
        url: String,
        stream_buf_size: usize,
    ) -> Result<Self, FromHttpLoaderError> {
        let resp = loader
            .fetch(fnet_http::Request {
                method: None,
                url: Some(url),
                headers: Some(vec![fnet_http::Header {
                    name: b"accept".to_vec(),
                    value: b"text/event-stream".to_vec(),
                }]),
                body: None,
                deadline: None,
                ..Default::default()
            })
            .await
            .map_err(FromHttpLoaderError::FetchFidl)?;
        let socket = match resp {
            fnet_http::Response {
                error: None, body: Some(body), status_code: Some(200), ..
            } => body,
            fnet_http::Response { error, status_code, status_line, .. } => {
                return Err(FromHttpLoaderError::FetchError { error, status_code, status_line });
            }
        };
        Ok(Self {
            chunks: stream_from_socket(fasync::Socket::from_socket(socket), stream_buf_size),
            source: EventSource::new(),
            events: vec![].into_iter(),
        })
    }
}

#[cfg(target_os = "fuchsia")]
fn stream_from_socket(
    socket: fasync::Socket,
    stream_buf_size: usize,
) -> futures::stream::BoxStream<'static, Result<hyper::body::Bytes, anyhow::Error>> {
    let initial_buf = vec![0; stream_buf_size];
    futures::stream::try_unfold((socket, initial_buf), |(mut socket, mut buf)| async move {
        match socket.read(&mut buf).await {
            Ok(0) => Ok(None),
            Ok(n) => {
                let chunk = hyper::body::Bytes::copy_from_slice(&buf[..n]);
                Ok(Some((chunk, (socket, buf))))
            }
            Err(e) => Err(anyhow::anyhow!(e).context("reading from socket")),
        }
    })
    .boxed()
}

#[derive(Debug, Error)]
pub enum FromHyperClientError {
    #[error("error creating http request")]
    CreateRequest(#[source] hyper::http::Error),

    #[error("error making http request")]
    MakeRequest(#[source] hyper_util::client::legacy::Error),

    #[error("http server responded with status other than OK: {0}")]
    HttpStatus(hyper::StatusCode),
}

#[cfg(target_os = "fuchsia")]
#[derive(Debug, Error)]
pub enum FromHttpLoaderError {
    #[error("fuchsia.net.http/Loader.Fetch fidl error")]
    FetchFidl(#[source] fidl::Error),

    #[error(
        "fuchsia.net.http/Loader.Fetch error: status {status_code:?} {error:?} {status_line:?}"
    )]
    FetchError {
        error: Option<fnet_http::Error>,
        status_code: Option<u32>,
        status_line: Option<Vec<u8>>,
    },
}

impl Stream for Client {
    type Item = Result<Event, ClientPollError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.events.next() {
                return Poll::Ready(Some(Ok(event)));
            }
            match Pin::new(&mut self.chunks).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.events = self.source.parse(&chunk).into_iter();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ClientPollError::NextChunk(e))));
                }
                Poll::Ready(None) => {
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ClientPollError {
    #[error("error downloading next chunk")]
    NextChunk(#[source] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use fuchsia_async::net::TcpListener;
    use fuchsia_hyper::new_https_client;
    use futures::TryStreamExt as _;
    use futures::future::Future;
    use hyper::Response;
    use hyper::service::service_fn;
    use std::net::{Ipv4Addr, SocketAddr};
    use test_case::test_case;
    pub enum Body {
        Empty(http_body_util::Empty<hyper::body::Bytes>),
        Full(http_body_util::Full<hyper::body::Bytes>),
        Stream(
            http_body_util::StreamBody<
                std::pin::Pin<
                    Box<
                        dyn futures::stream::Stream<
                                Item = Result<
                                    hyper::body::Frame<hyper::body::Bytes>,
                                    Box<dyn std::error::Error + Send + Sync + 'static>,
                                >,
                            > + Send,
                    >,
                >,
            >,
        ),
    }

    impl Body {
        pub fn empty() -> Self {
            Body::Empty(http_body_util::Empty::new())
        }
        pub fn wrap_stream<S, O, E>(stream: S) -> Self
        where
            S: futures::stream::Stream<Item = Result<O, E>> + Send + 'static,
            O: Into<hyper::body::Bytes>,
            E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
        {
            let stream = stream.map(|res| match res {
                Ok(data) => Ok(hyper::body::Frame::data(data.into())),
                Err(e) => Err(e.into()),
            });
            Body::Stream(http_body_util::StreamBody::new(Box::pin(stream)))
        }
    }
    impl From<Vec<u8>> for Body {
        fn from(data: Vec<u8>) -> Self {
            Body::Full(http_body_util::Full::new(data.into()))
        }
    }
    impl hyper::body::Body for Body {
        type Data = hyper::body::Bytes;
        type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

        fn poll_frame(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
            match self.get_mut() {
                Body::Empty(b) => std::pin::Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
                Body::Full(b) => std::pin::Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
                Body::Stream(b) => std::pin::Pin::new(b).poll_frame(cx),
            }
        }
    }

    fn spawn_server<F>(handle_req: fn(Request<hyper::body::Incoming>) -> F) -> String
    where
        F: Future<Output = Result<Response<Body>, hyper::Error>> + Send + 'static,
    {
        use futures::StreamExt as _;
        use hyper_util::rt::TokioIo;

        let (listener, url) = {
            let listener =
                TcpListener::bind(&SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)).unwrap();
            let local_addr = listener.local_addr().unwrap();
            (listener.accept_stream(), format!("http://{}", local_addr))
        };
        let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);
        fasync::Task::spawn(async move {
            let mut tasks = futures::stream::FuturesUnordered::new();
            let mut listener = listener.fuse();
            loop {
                futures::select! {
                    res = listener.next() => {
                        match res {
                            Some(Ok((stream, _addr))) => {
                                let stream = fuchsia_hyper::TcpStream { stream };
                                let service = service_fn(handle_req);
                                let builder = builder.clone();
                                tasks.push(fasync::Task::spawn(async move {
                                    let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                                }));
                            }
                            _ => break,
                        }
                    }
                    _ = tasks.next() => {}
                }
            }
        }).detach();
        url
    }

    fn make_event() -> Event {
        Event::from_type_and_data("event_type", "data_contents").unwrap()
    }

    async fn make_client(byte_source: &str, url: String) -> Client {
        match byte_source {
            "hyper" => Client::from_hyper_client(&new_https_client(), url).await.unwrap(),
            #[cfg(target_os = "fuchsia")]
            "loader" => Client::from_http_loader(
                &fuchsia_component::client::connect_to_protocol::<fnet_http::LoaderMarker>()
                    .unwrap(),
                url,
                50,
            )
            .await
            .unwrap(),
            s => panic!("unexpected byte_soure {s}"),
        }
    }

    #[test_case("hyper")]
    #[cfg(target_os = "fuchsia")]
    #[test_case("loader")]
    #[fasync::run_singlethreaded(test)]
    async fn receive_one_event(byte_source: &str) {
        async fn handle_req(
            _req: Request<hyper::body::Incoming>,
        ) -> Result<Response<Body>, hyper::Error> {
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(make_event().to_vec().into())
                .unwrap())
        }
        let url = spawn_server(handle_req);

        let client = make_client(byte_source, url).await;
        let events: Result<Vec<_>, _> = client.collect::<Vec<_>>().await.into_iter().collect();

        assert_eq!(events.unwrap(), vec![make_event()]);
    }

    #[test_case("hyper")]
    #[cfg(target_os = "fuchsia")]
    #[test_case("loader")]
    #[fasync::run_singlethreaded(test)]
    async fn client_sends_correct_http_headers(byte_source: &str) {
        async fn handle_req(
            req: Request<hyper::body::Incoming>,
        ) -> Result<Response<Body>, hyper::Error> {
            assert_eq!(req.method(), &hyper::Method::GET);
            assert_eq!(
                req.headers().get("accept").map(|h| h.as_bytes()),
                Some(&b"text/event-stream"[..])
            );
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(make_event().to_vec().into())
                .unwrap())
        }
        let url = spawn_server(handle_req);

        let client = make_client(byte_source, url).await;
        client.collect::<Vec<_>>().await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn error_create_request() {
        assert_matches!(
            Client::from_hyper_client(&new_https_client(), "\n").await,
            Err(FromHyperClientError::CreateRequest(_))
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn error_make_request() {
        assert_matches!(
            Client::from_hyper_client(&new_https_client(), "bad_url2").await,
            Err(FromHyperClientError::MakeRequest(_))
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn hyper_error_http_status() {
        async fn handle_req(
            _req: Request<hyper::body::Incoming>,
        ) -> Result<Response<Body>, hyper::Error> {
            Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap())
        }
        let url = spawn_server(handle_req);

        assert_matches!(
            Client::from_hyper_client(&new_https_client(), url).await,
            Err(FromHyperClientError::HttpStatus(_))
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn loader_error_http_status() {
        async fn handle_req(
            _req: Request<hyper::body::Incoming>,
        ) -> Result<Response<Body>, hyper::Error> {
            Ok(Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap())
        }
        let url = spawn_server(handle_req);

        assert_matches!(
            Client::from_http_loader(
                &fuchsia_component::client::connect_to_protocol::<fnet_http::LoaderMarker>()
                    .unwrap(),
                url,
                50,
            )
            .await,
            Err(FromHttpLoaderError::FetchError { status_code: Some(404), .. })
        );
    }

    // The fuchsia.net.http.Response contains the http response body as a zx::Socket of bytes,
    // so it doesn't have stream reading errors unless there is an error reading from the actual
    // socket.
    #[fasync::run_singlethreaded(test)]
    async fn error_downloading_chunk() {
        // If the body of an http response is not large enough, hyper will download the body
        // along with the header in the initial fuchsia_hyper::HttpsClient.request(). This means
        // that even if the body is implemented with a stream that fails before the transfer is
        // complete, the failure will occur during the initial request, before awaiting on the
        // body chunk stream.
        const BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_DELAYED_STREAMING: usize = 1_000_000;

        async fn handle_req(
            _req: Request<hyper::body::Incoming>,
        ) -> Result<Response<Body>, hyper::Error> {
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(
                    "content-length",
                    &format!("{}", BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_DELAYED_STREAMING),
                )
                .header("content-type", "text/event-stream")
                .body(Body::wrap_stream(futures::stream::iter(vec![
                    Ok(vec![b' '; BODY_SIZE_LARGE_ENOUGH_TO_TRIGGER_DELAYED_STREAMING - 1]),
                    Err("error-text".to_string()),
                ])))
                .unwrap())
        }
        let url = spawn_server(handle_req);
        let mut client = Client::from_hyper_client(&new_https_client(), url).await.unwrap();

        assert_matches!(client.try_next().await, Err(ClientPollError::NextChunk(_)));
    }

    #[test]
    fn test_stream_from_socket() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (snd, rcv) = zx::Socket::create_stream();
        let mut stream = stream_from_socket(fasync::Socket::from_socket(rcv), 10);

        // Empty socket is pending.
        let mut next = stream.next();
        assert_matches!(executor.run_until_stalled(&mut next), Poll::Pending);

        // Non-empty socket yields correctly truncated Bytes.
        assert_eq!(snd.write(b"test msg").unwrap(), 8);
        assert_matches!(
            executor.run_until_stalled(&mut next),
            Poll::Ready(Some(Ok(b))) if (&*b)[..] == *b"test msg".as_slice()
        );
        drop(next);

        // Write larger than buf split across multiple yields.
        assert_eq!(snd.write(b"0000000000111").unwrap(), 13);
        assert_matches!(
            executor.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(b))) if (&*b)[..] == *b"0000000000".as_slice()
        );
        assert_matches!(
            executor.run_until_stalled(&mut stream.next()),
            Poll::Ready(Some(Ok(b))) if (&*b)[..] == *b"111".as_slice()
        );

        // Closed socket is finished.
        drop(snd);
        assert_matches!(executor.run_until_stalled(&mut stream.next()), Poll::Ready(None));
    }
}
