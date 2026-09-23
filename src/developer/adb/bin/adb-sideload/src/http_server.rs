// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_archive::AsyncUtf8Reader;
use fuchsia_async as fasync;
use fuchsia_fs::file::{AsyncGetSize, AsyncReadAt};
use futures::lock::Mutex;
use futures::{Future, FutureExt as _, StreamExt as _};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder;
use std::net::SocketAddr;
use std::sync::Arc;

type Body = UnsyncBoxBody<Bytes, anyhow::Error>;

fn empty_body() -> Body {
    BodyExt::boxed_unsync(Empty::new().map_err(|e| match e {}))
}

fn stream_body<S, O, E>(stream: S) -> Body
where
    S: futures::Stream<Item = Result<O, E>> + Send + 'static,
    O: Into<Bytes>,
    E: Into<anyhow::Error>,
{
    BodyExt::boxed_unsync(StreamBody::new(Box::pin(
        stream.map(|res| res.map(|data| Frame::data(data.into())).map_err(Into::into)),
    )))
}

async fn serve_file<T: AsyncReadAt + AsyncGetSize + Unpin + Send + 'static>(
    req: Request<Incoming>,
    far_reader: Arc<AsyncUtf8Reader<Arc<Mutex<T>>>>,
    block_size: usize,
) -> Result<Response<Body>, hyper::http::Error> {
    let path = req.uri().path();
    let file_name = path.strip_prefix('/').unwrap_or(path);

    match far_reader.read_file_stream(file_name, block_size) {
        Ok((length, stream)) => Response::builder()
            .header(hyper::header::CONTENT_LENGTH, length)
            .body(stream_body(stream)),
        Err(fuchsia_archive::Error::PathNotPresent(_)) => {
            Response::builder().status(StatusCode::NOT_FOUND).body(empty_body())
        }
        Err(e) => {
            log::error!("Failed to read file: {:#}", anyhow::anyhow!(e));
            Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(empty_body())
        }
    }
}

pub fn start_server<T: AsyncReadAt + AsyncGetSize + Unpin + Send + 'static>(
    far_reader: AsyncUtf8Reader<Arc<Mutex<T>>>,
    block_size: usize,
) -> Result<
    (
        impl Future<Output = Result<(), hyper::Error>> + 'static,
        impl FnOnce() -> Arc<AsyncUtf8Reader<Arc<Mutex<T>>>>,
        SocketAddr,
    ),
    Error,
> {
    let far_reader = Arc::new(far_reader);
    let (close_sender, close_receiver) = futures::channel::oneshot::channel();
    let addr = SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 0);
    let listener = fasync::net::TcpListener::bind(&addr)?;
    let local_addr = listener.local_addr()?;
    let server_fut = {
        let far_reader = Arc::clone(&far_reader);
        async move {
            let builder = Builder::new(fuchsia_hyper::Executor);
            let mut close_receiver = close_receiver.fuse();
            let mut listener = listener.accept_stream().fuse();
            let mut tasks = futures::stream::FuturesUnordered::new();
            loop {
                futures::select! {
                    stream_res = listener.next() => {
                        match stream_res {
                            Some(Ok((conn, _))) => {
                                let far_reader = Arc::clone(&far_reader);
                                let builder = builder.clone();
                                tasks.push(fasync::Task::spawn(async move {
                                    let _ = builder.serve_connection(
                                        TokioIo::new(fuchsia_hyper::TcpStream { stream: conn }),
                                        service_fn(move |req| {
                                            serve_file(req, Arc::clone(&far_reader), block_size)
                                        }),
                                    ).await;
                                }));
                            }
                            _ => break,
                        }
                    }
                    _ = tasks.next() => {}
                    _ = close_receiver => break,
                }
            }
            Ok::<(), hyper::Error>(())
        }
    };
    let close_fn = move || {
        let _ = close_sender.send(());
        far_reader
    };
    Ok((server_fut, close_fn, local_addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_fs::file::Adapter;
    use futures::SinkExt as _;
    use futures::channel::mpsc;
    use futures::io::Cursor;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[fuchsia::test]
    async fn test_server() {
        let mut far_data = Vec::new();
        let file_content = "hello world";
        let files = BTreeMap::from([(
            "file1.txt",
            (file_content.len() as u64, Box::new(file_content.as_bytes()) as Box<dyn Read>),
        )]);
        fuchsia_archive::write(&mut far_data, files).unwrap();

        let reader = Adapter::new(Cursor::new(far_data));
        let far_reader = AsyncUtf8Reader::new(Arc::new(Mutex::new(reader))).await.unwrap();

        let (server_fut, close_fn, addr) = start_server(far_reader, 1024).unwrap();
        let server_task = fasync::Task::spawn(server_fut);

        let client = fuchsia_hyper::new_client();
        let uri = format!("http://{addr}/file1.txt");
        let response = client.get(uri.parse().unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(hyper::header::CONTENT_LENGTH).unwrap(),
            file_content.len().to_string().as_bytes()
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), file_content.as_bytes());

        let uri = format!("http://{addr}/nonexistent.txt");
        let response = client.get(uri.parse().unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let far_reader = close_fn();
        server_task.await.unwrap();
        let far_reader = Arc::into_inner(far_reader).unwrap();
        let _reader: Adapter<_> = Arc::into_inner(far_reader.into_source()).unwrap().into_inner();
    }

    struct BlockingReader<R> {
        reader: R,
        receiver: Option<mpsc::Receiver<()>>,
    }

    impl<R: AsyncGetSize + Unpin> AsyncGetSize for BlockingReader<R> {
        fn poll_get_size(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Pin::new(&mut self.reader).poll_get_size(cx)
        }
    }

    impl<R: AsyncReadAt + Unpin> AsyncReadAt for BlockingReader<R> {
        fn poll_read_at(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            offset: u64,
            buf: &mut [u8],
        ) -> Poll<std::io::Result<usize>> {
            match &mut self.receiver {
                Some(receiver) => match receiver.poll_next_unpin(cx) {
                    Poll::Ready(Some(())) => {
                        Pin::new(&mut self.reader).poll_read_at(cx, offset, buf)
                    }
                    Poll::Ready(None) => Poll::Ready(Ok(0)),
                    Poll::Pending => Poll::Pending,
                },
                None => Pin::new(&mut self.reader).poll_read_at(cx, offset, buf),
            }
        }
    }

    #[fuchsia::test]
    async fn test_server_large_file_streaming() {
        let mut far_data = Vec::new();
        let file_content = vec![0; 200 * 1024];
        let files = BTreeMap::from([(
            "large_file",
            (file_content.len() as u64, Box::new(file_content.as_slice()) as Box<dyn Read>),
        )]);
        fuchsia_archive::write(&mut far_data, files).unwrap();

        let blocking_reader =
            BlockingReader { reader: Adapter::new(Cursor::new(far_data)), receiver: None };
        let blocking_reader = Arc::new(Mutex::new(blocking_reader));
        let far_reader = AsyncUtf8Reader::new(Arc::clone(&blocking_reader)).await.unwrap();

        let (mut sender, receiver) = mpsc::channel(1);
        blocking_reader.lock().await.receiver = Some(receiver);
        drop(blocking_reader);

        let (server_fut, close_fn, addr) = start_server(far_reader, 10 * 1024).unwrap();
        let server_task = fasync::Task::spawn(server_fut);

        let client = fuchsia_hyper::new_client();
        let uri = format!("http://{addr}/large_file");
        let response = client.get(uri.parse().unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(hyper::header::CONTENT_LENGTH).unwrap(),
            file_content.len().to_string().as_bytes()
        );
        let mut body = response.into_body();
        let mut received = Vec::new();

        // Unblock the first read.
        sender.send(()).await.unwrap();
        let mut chunk_count = 0;
        while let Some(frame) = body.frame().await {
            chunk_count += 1;
            if let Ok(data) = frame.unwrap().into_data() {
                received.extend_from_slice(&data);
            }
            sender.send(()).await.unwrap();
        }
        assert_eq!(received, file_content);
        // Based on the block size limit, there should be at least 20 chunks.
        assert!(chunk_count >= 20);

        let far_reader = close_fn();
        server_task.await.unwrap();
        let far_reader = Arc::into_inner(far_reader).unwrap();
        let _reader: BlockingReader<_> =
            Arc::into_inner(far_reader.into_source()).unwrap().into_inner();
    }
}
