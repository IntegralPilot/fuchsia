// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_pkg_http as fpkg_http;
use fuchsia_async as fasync;
use futures::future::FutureExt as _;
use futures::select;
use futures::stream::{StreamExt as _, TryStreamExt as _};
use std::net::SocketAddr;

async fn serve_blob_writer_request_stream(mut stream: ffxfs::BlobWriterRequestStream) -> Vec<u8> {
    let (vmo, size) = match stream.next().await.unwrap().unwrap() {
        ffxfs::BlobWriterRequest::GetVmo { size, responder } => {
            let vmo = zx::Vmo::create(size).unwrap();
            let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let () = responder.send(Ok(vmo_clone)).unwrap();
            (vmo, size)
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
    let mut content = vec![0; usize::try_from(size).unwrap()];
    let () = vmo.read(&mut content, 0).unwrap();
    content
}

async fn check_download_blob(client: fpkg_http::ClientProxy, addr: SocketAddr) {
    let (blob_writer, blob_writer_request_stream) =
        fidl::endpoints::create_request_stream::<ffxfs::BlobWriterMarker>();
    let blob_writer_server =
        fasync::Task::spawn(serve_blob_writer_request_stream(blob_writer_request_stream));

    assert_eq!(
        client
            .download_blob(
                &format!("http://{addr}"),
                blob_writer,
                zx::BootDuration::from_seconds(30).into_nanos(),
                zx::BootDuration::from_seconds(30).into_nanos(),
                0
            )
            .await
            .unwrap()
            .unwrap(),
        u64::try_from(crate::ROOT_DOCUMENT.len()).unwrap()
    );
    assert_eq!(blob_writer_server.await, crate::ROOT_DOCUMENT.as_bytes());
}

/// Tests that idle detection, escrow, and resume of fuchsia.pkg.http.Client connections is
/// implemented correctly.
#[fuchsia::test]
async fn test_idle_stop_escrow_start() {
    crate::run_without_connecting("idle_1ms", |addr, http_client| async move {
        let mut event_stream = EventStream::open().await.unwrap();

        let client: fpkg_http::ClientProxy =
            http_client.connect_to_protocol_at_exposed_dir().unwrap();

        // Cause the framework to deliver the loader server endpoint to the component.
        // This will start the component.
        check_download_blob(client.clone(), addr).await;
        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();

        // Wait for the component to stop because the connection stalled.
        _ = EventMatcher::ok()
            .stop(Some(ExitStatusMatcher::Clean))
            .moniker(http_client.moniker())
            .wait::<Stopped>(&mut event_stream)
            .await
            .unwrap();

        // Now make a two-way call on the connection again and it should still work
        // (by starting the component again).
        check_download_blob(client.clone(), addr).await;
        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();
    })
    .await
}

// Test that an active blob download prevents the component from stopping.
#[fuchsia::test]
async fn test_download_blob_blocks_idle_stop() {
    crate::run_without_connecting("idle_1ms", |addr, http_client| async move {
        let mut event_stream = EventStream::open().await.unwrap();
        let client: fpkg_http::ClientProxy =
            http_client.connect_to_protocol_at_exposed_dir().unwrap();
        let (blob_writer, _blob_writer_request_stream) =
            fidl::endpoints::create_request_stream::<ffxfs::BlobWriterMarker>();

        let _pending_fut = client.download_blob(
            &format!("http://{addr}{}", crate::PENDING),
            blob_writer,
            zx::BootDuration::from_seconds(30).into_nanos(),
            zx::BootDuration::from_seconds(30).into_nanos(),
            0,
        );

        _ = EventMatcher::ok()
            .moniker(http_client.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap();

        // Wait beyond the 1ms timeout. `http-client` should not stop.
        // A timeout is not the most ideal thing. But if `http-client` incorrectly stopped,
        // this test will flake. Having a flakily failing test should better than no test.
        let mut stop_event = Box::pin(
            EventMatcher::ok()
                .moniker(http_client.moniker())
                .wait::<Stopped>(&mut event_stream)
                .fuse(),
        );
        select! {
            event = &mut stop_event => panic!("Unexpected stop event {event:?}"),
            _ = fasync::Timer::new(fasync::MonotonicDuration::from_millis(500)).fuse() => {},
        };
    })
    .await
}
