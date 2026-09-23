// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_pkg_http as fpkg_http;
use fuchsia_async as fasync;
use fuchsia_hyper as fhyper;
use fuchsia_inspect as finspect;
use fuchsia_inspect::Property as _;
use fuchsia_trace as ftrace;
use futures::stream::TryStreamExt as _;
use log::warn;

const TCP_KEEPALIVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn serve_client_request_stream(
    stream: fpkg_http::ClientRequestStream,
    idle_timeout: fasync::MonotonicDuration,
    tcp_receive_buffer_size: Option<usize>,
    inspect: finspect::Node,
) -> Result<(), anyhow::Error> {
    let inspect = inspect.create_child("downloads");
    let request_count = std::sync::atomic::AtomicU64::new(0);
    // Reuse client with every request to take advantage of connection pooling.
    // Note that if the connection is escrowed subsequent requests will use a new client.
    let mut tcp_options = fhyper::TcpOptions::keepalive_timeout(TCP_KEEPALIVE_TIMEOUT);
    tcp_options.tcp_receive_buffer_size = tcp_receive_buffer_size;
    let client = fhyper::new_https_client_from_tcp_options(tcp_options);

    let (stream, unbind_if_stalled) = detect_stall::until_stalled(stream, idle_timeout);

    let () = stream
        .err_into::<anyhow::Error>()
        .try_for_each_concurrent(None, |message| {
            // NB: Shadow variables with a reference that we don't want to move
            // into the async closure.
            let client = &client;
            let inspect = &inspect;
            let request_count = &request_count;
            async move {
                match message {
                    fpkg_http::ClientRequest::DownloadBlob {
                        url,
                        destination,
                        header_timeout,
                        body_timeout,
                        resumption_attempt_limit,
                        responder,
                    } => {
                        let r = download_blob(
                            client,
                            url,
                            destination.into_proxy(),
                            crate::resuming_get::Params {
                                header_timeout: zx::BootDuration::from_nanos(header_timeout),
                                body_timeout: zx::BootDuration::from_nanos(body_timeout),
                                resumption_attempt_limit,
                            },
                            inspect.create_child(
                                request_count
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                    .to_string(),
                            ),
                        )
                        .await;
                        responder.send(r).map_err(anyhow::Error::from)
                    }
                }
            }
        })
        .await?;

    if let Ok(Some(server_end)) = unbind_if_stalled.await {
        fuchsia_component::client::connect_channel_to_protocol_at::<fpkg_http::ClientMarker>(
            server_end.into(),
            "/escrow",
        )?;
    }

    Ok(())
}

async fn download_blob(
    client: &fhyper::HttpsClient,
    url: String,
    writer: ffxfs::BlobWriterProxy,
    params: crate::resuming_get::Params,
    inspect: finspect::Node,
) -> Result<u64, fpkg_http::ClientDownloadBlobError> {
    download_blob_impl(client, &url, writer, params, inspect).await.map_err(
        |e: DownloadBlobError| {
            let fidl_err = e.to_fidl_err();
            warn!(url:%, params:?; "Failed to download blob: {:#}", anyhow!(e));
            fidl_err
        },
    )
}

async fn download_blob_impl(
    client: &fhyper::HttpsClient,
    url: &str,
    writer: ffxfs::BlobWriterProxy,
    params: crate::resuming_get::Params,
    inspect: finspect::Node,
) -> Result<u64, DownloadBlobError> {
    let trace_id = ftrace::Id::new();
    let _guard = ftrace::async_enter!(
        trace_id, "app", "http-client-download-blob",
        "url" => url
    );
    inspect.record_int("start_boot_ns", zx::BootInstant::get().into_nanos());
    inspect.record_string("url", url);
    let inspect_bytes_written = inspect.create_uint("bytes_written", 0);
    let inspect_state = inspect.create_string("state", "initial");
    let inspect_state_ts = inspect.create_int("state_boot_ns", zx::BootInstant::get().into_nanos());
    let set_inspect_state = |state| {
        inspect_state.set(state);
        inspect_state_ts.set(zx::BootInstant::get().into_nanos());
    };

    let (expected_len, content) = {
        let _guard = ftrace::async_enter(trace_id, "app", "http_get_startup", &[]);
        crate::resuming_get::resuming_get(
            client,
            url.parse().map_err(DownloadBlobError::ParseUrl)?,
            params,
            inspect.create_child("resuming-get"),
        )
        .await
        .map_err(DownloadBlobError::ObtainingBodyStream)?
    };
    inspect.record_uint("expected_size_bytes", expected_len);

    set_inspect_state("create-writer");
    let mut writer = {
        let _guard = ftrace::async_enter(trace_id, "app", "creating_writer", &[]);
        blob_writer::BlobWriter::create(writer, expected_len)
            .await
            .map_err(DownloadBlobError::CreateBlobWriter)?
    };

    futures::pin_mut!(content);
    let mut written = 0u64;
    while written < expected_len {
        set_inspect_state("read-http-body");
        let chunk = {
            let _guard = ftrace::async_enter(trace_id, "app", "reading_from_network", &[]);
            content.try_next().await.map_err(DownloadBlobError::ReadBodyStream)?
        };
        let Some(chunk) = chunk else {
            return Err(DownloadBlobError::BodyStreamTerminatedEarly);
        };
        if written + chunk.len() as u64 > expected_len {
            return Err(DownloadBlobError::BodyStreamTooManyBytes);
        }

        set_inspect_state("write-blob");
        let fut = writer.write(&chunk);
        let () = {
            let _guard = ftrace::async_enter(
                trace_id,
                "app",
                "waiting_for_blob_write_ack",
                &[ftrace::ArgValue::of("size", chunk.len() as u64)],
            );
            fut.await
        }
        .map_err(DownloadBlobError::WriteBlob)?;

        written += chunk.len() as u64;
        inspect_bytes_written.set(written);
    }
    set_inspect_state("write-complete");

    Ok(expected_len)
}

#[derive(thiserror::Error, Debug)]
enum DownloadBlobError {
    #[error("parsing url")]
    ParseUrl(#[source] http::uri::InvalidUri),

    #[error("obtaining body stream")]
    ObtainingBodyStream(#[source] crate::resuming_get::ResumingGetError),

    #[error("creating blob writer")]
    CreateBlobWriter(#[source] blob_writer::CreateError),

    #[error("reading from body stream")]
    ReadBodyStream(#[source] crate::resuming_get::ResumingGetError),

    #[error("body stream terminated early")]
    BodyStreamTerminatedEarly,

    #[error("body stream has too many bytes")]
    BodyStreamTooManyBytes,

    #[error("writing to blob writer")]
    WriteBlob(#[source] blob_writer::WriteError),
}

impl DownloadBlobError {
    fn to_fidl_err(&self) -> fpkg_http::ClientDownloadBlobError {
        use DownloadBlobError::*;
        match self {
            ParseUrl(_) => fpkg_http::ClientDownloadBlobError::Other,
            ObtainingBodyStream(e) | ReadBodyStream(e) => {
                use crate::resuming_get::ResumingGetError::*;
                match e {
                    StatusNotOk(status) | StatusNotPartialContent { status, .. } => match *status {
                        http::StatusCode::TOO_MANY_REQUESTS => {
                            fpkg_http::ClientDownloadBlobError::NetworkRateLimit
                        }
                        http::StatusCode::NOT_FOUND => fpkg_http::ClientDownloadBlobError::NotFound,
                        _ => fpkg_http::ClientDownloadBlobError::Network,
                    },
                    UnknownBodySize(_) => fpkg_http::ClientDownloadBlobError::Other,
                    _ => fpkg_http::ClientDownloadBlobError::Network,
                }
            }
            CreateBlobWriter(_) => fpkg_http::ClientDownloadBlobError::Other,
            BodyStreamTerminatedEarly => fpkg_http::ClientDownloadBlobError::Other,
            BodyStreamTooManyBytes => fpkg_http::ClientDownloadBlobError::Other,
            WriteBlob(blob_writer::WriteError::BytesReady(s)) if *s == zx::Status::NO_SPACE => {
                fpkg_http::ClientDownloadBlobError::NoSpace
            }
            WriteBlob(_) => fpkg_http::ClientDownloadBlobError::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::StreamExt as _;

    // Takes an async fn that maps requests to responses and returns an http server and the address
    // the server is listening on.
    fn create_http_server<F>(
        handler: impl FnMut(hyper::Request<hyper::body::Incoming>) -> F + Send + Clone + 'static,
    ) -> (std::net::SocketAddr, fasync::Task<()>)
    where
        F: std::future::Future<
                Output = std::result::Result<
                    hyper::Response<http_body_util::Full<hyper::body::Bytes>>,
                    std::convert::Infallible,
                >,
            > + Send
            + 'static,
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

    async fn serve_blob_writer_request_stream(
        expected_size: u64,
        mut stream: ffxfs::BlobWriterRequestStream,
    ) -> Vec<u8> {
        let vmo = match stream.next().await.unwrap().unwrap() {
            ffxfs::BlobWriterRequest::GetVmo { size, responder } => {
                assert_eq!(expected_size, size);
                let vmo = zx::Vmo::create(expected_size).unwrap();
                let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                let () = responder.send(Ok(vmo_clone)).unwrap();
                vmo
            }
            req => panic!("unexpected request {req:?}"),
        };
        while let Some(req) = stream.try_next().await.unwrap() {
            match req {
                ffxfs::BlobWriterRequest::BytesReady { bytes_written: _, responder } => {
                    let () = responder.send(Ok(())).unwrap();
                }
                req => panic!("unexpected request {req:?}"),
            }
        }
        let mut content = vec![0; usize::try_from(expected_size).unwrap()];
        let () = vmo.read(&mut content, 0).unwrap();
        content
    }

    #[fuchsia::test]
    async fn download_blob_writes_to_blob_writer() {
        let content: Vec<u8> =
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9].into_iter().cycle().take(500).collect();
        let content_clone = content.clone();
        let request_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        // Expects exactly one request, responds with `content`.
        let handler = move |req: hyper::Request<hyper::body::Incoming>| {
            assert_eq!(req.method(), http::Method::GET);
            assert_eq!(req.uri().path(), "/the-file.txt");
            assert_eq!(request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 0);
            let resp = hyper::Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_LENGTH, content.len())
                .body(http_body_util::Full::new(content.clone().into()))
                .unwrap();
            std::future::ready(Ok::<_, std::convert::Infallible>(resp))
        };
        let (server_addr, _server) = create_http_server(handler);
        let client = fhyper::new_https_client();
        let (proxy, blob_writer_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<ffxfs::BlobWriterMarker>();
        let blob_writer_server = fasync::Task::spawn(serve_blob_writer_request_stream(
            content_clone.len().try_into().unwrap(),
            blob_writer_request_stream,
        ));

        assert_eq!(
            download_blob(
                &client,
                format!("http://{server_addr}/the-file.txt").parse().unwrap(),
                proxy,
                crate::resuming_get::Params {
                    header_timeout: zx::BootDuration::from_seconds(30),
                    body_timeout: zx::BootDuration::from_seconds(30),
                    resumption_attempt_limit: 0
                },
                Default::default()
            )
            .await,
            Ok(u64::try_from(content_clone.len()).unwrap())
        );
        assert_eq!(blob_writer_server.await, content_clone);
    }
}
