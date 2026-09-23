// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg as fpkg;
use futures::future::TryFutureExt as _;
use futures::stream::Stream;

/// Converts a proxy to a FIDL iterator like:
///
/// protocol PayloadIterator {
///    Next() -> (vector<Payload>:MAX payloads);
/// };
///
/// into a `Stream` of `Result<Vec<Payload>, fidl::Error>`s.
///
/// The returned stream will never yield an empty `Vec`. When e.g. `PayloadIterator::Next` returns
/// an empty Vec, the returned stream will yield `None` (signaling the end of the stream).
///
/// To use with a new protocol (e.g. `PayloadIterator`), implement `FidlIterator` for
/// `PayloadIteratorProxy`.
pub fn fidl_iterator_to_stream<T: FidlIterator>(
    iterator: T,
) -> impl Stream<Item = Result<Vec<T::Item>, fidl::Error>> + Unpin {
    futures::stream::try_unfold(iterator, |iterator| {
        iterator.next().map_ok(|v| if v.is_empty() { None } else { Some((v, iterator)) })
    })
}

/// A FIDL proxy for a FIDL protocol following the iterator pattern.
pub trait FidlIterator {
    type Item: Unpin;

    fn next(&self) -> fidl::client::QueryResponseFut<Vec<Self::Item>>;
}

impl FidlIterator for fpkg::BlobInfoIteratorProxy {
    type Item = fpkg::BlobInfo;

    fn next(&self) -> fidl::client::QueryResponseFut<Vec<Self::Item>> {
        self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::Responder as _;
    use futures::future::join;
    use futures::stream::{StreamExt as _, TryStreamExt as _};
    use zx_status::Status;

    struct MockIteratorServer {
        reqs: fpkg::BlobInfoIteratorRequestStream,
    }

    impl MockIteratorServer {
        fn new() -> (Self, impl Stream<Item = Result<Vec<fpkg::BlobInfo>, fidl::Error>>) {
            let (proxy, reqs) =
                fidl::endpoints::create_proxy_and_stream::<fpkg::BlobInfoIteratorMarker>();
            (Self { reqs }, fidl_iterator_to_stream(proxy))
        }

        // On Some(resp) responds with resp, else closes channel with NO_RESOURCES.
        async fn expect_next(&mut self, resp: Option<Vec<fpkg::BlobInfo>>) {
            let fpkg::BlobInfoIteratorRequest::Next { responder } =
                self.reqs.next().await.unwrap().unwrap();
            match resp {
                Some(resp) => responder.send(&resp).unwrap(),
                None => responder.control_handle().shutdown_with_epitaph(Status::NO_RESOURCES),
            }
        }
    }

    fn blob_info(u: u8) -> fpkg::BlobInfo {
        fpkg::BlobInfo { blob_id: fpkg::BlobId { merkle_root: [u; 32] }, length: 0 }
    }

    #[fuchsia::test]
    async fn read_one_item() {
        let (mut server, mut stream) = MockIteratorServer::new();

        let ((), item) = join(server.expect_next(Some(vec![blob_info(1)])), stream.next()).await;

        assert_matches!(item, Some(Ok(v)) if v == vec![blob_info(1)]);
    }

    #[fuchsia::test]
    async fn read_two_items() {
        let (mut server, mut stream) = MockIteratorServer::new();

        let ((), (first, second)) = join(
            async {
                server.expect_next(Some(vec![blob_info(1)])).await;
                server.expect_next(Some(vec![blob_info(2)])).await
            },
            async { (stream.next().await, stream.next().await) },
        )
        .await;

        assert_matches!(first, Some(Ok(v)) if v == vec![blob_info(1)]);
        assert_matches!(second, Some(Ok(v)) if v == vec![blob_info(2)]);
    }

    #[fuchsia::test]
    async fn error_terminates() {
        let (mut server, mut stream) = MockIteratorServer::new();

        let ((), (first, second)) =
            join(server.expect_next(None), async { (stream.next().await, stream.next().await) })
                .await;

        assert_matches!(
            first,
            Some(Err(fidl::Error::ClientChannelClosed { epitaph, ..}))
                if epitaph == Status::NO_RESOURCES
        );
        assert_matches!(second, None);
    }

    #[fuchsia::test]
    async fn empty_response_terminates() {
        let (mut server, mut stream) = MockIteratorServer::new();

        let ((), item) = join(server.expect_next(Some(vec![])), stream.next()).await;

        assert_matches!(item, None);
    }

    #[fuchsia::test]
    async fn read_one_item_then_terminate_successfully() {
        let (mut server, stream) = MockIteratorServer::new();

        let ((), items) = join(
            async {
                server.expect_next(Some(vec![blob_info(1)])).await;
                server.expect_next(Some(vec![])).await
            },
            stream.map_err(|_| ()).try_concat(),
        )
        .await;

        assert_eq!(items, Ok(vec![blob_info(1)]));
    }

    #[fuchsia::test]
    async fn read_one_item_then_terminate_with_error() {
        let (mut server, stream) = MockIteratorServer::new();

        let ((), items) = join(
            async {
                server.expect_next(Some(vec![blob_info(1)])).await;
                server.expect_next(None).await
            },
            stream.map_err(|_| ()).collect::<Vec<_>>(),
        )
        .await;

        assert_eq!(items, vec![Ok(vec![blob_info(1)]), Err(())]);
    }
}
