// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_ext as fpkg_ext;
use fidl_fuchsia_pkg_internal as fpkg_internal;
use futures::stream::TryStreamExt as _;
use log::warn;
use std::sync::Arc;

/// Used only by the system-updater to fetch blobs during OTA, and so:
/// * assumes the retained blobs index has been initialized with the to-be-fetched blob
/// * to attempt to recover from a partially broken system:
///   * queries `fuchsia.fxfs/BlobCreator.NeedsOverwrite` for every blob (e.g. does not
///     short-circuit the blob write if a blob is readable via `fuchsia.fxfs/BlobReader.GetVmo`),
///     unless the client requests an overwrite, in which case blobfs isn't even queried and the
///     existing blob is overwritten
pub(crate) async fn serve_request_stream(
    stream: fpkg_internal::OtaDownloaderRequestStream,
    blob_fetcher: crate::blob_fetcher::BlobFetcher,
) -> anyhow::Result<()> {
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, async |req| match req {
            fpkg_internal::OtaDownloaderRequest::FetchBlob {
                hash,
                base_url,
                overwrite_existing,
                responder,
            } => {
                let blob_id = hash.into();
                responder
                    .send(
                        fetch_blob(blob_id, &base_url, overwrite_existing, &blob_fetcher)
                            .await
                            .map_err(|e| {
                                let fidl_err = (&e).into();
                                warn!(
                                    blob_id:%,
                                    base_url:%,
                                    overwrite_existing:%;
                                    "failed to fetch blob: {:#}", anyhow!(e)
                                );
                                fidl_err
                            }),
                    )
                    .context("sending fuchsia.pkg.internal/OtaDownloader.FetchBlob response")
            }
        })
        .await
}

async fn fetch_blob(
    blob_id: fpkg_ext::BlobId,
    base_url: &str,
    overwrite_existing: bool,
    blob_fetcher: &crate::blob_fetcher::BlobFetcher,
) -> Result<u64, Error> {
    blob_fetcher
        .push(
            blob_id,
            crate::blob_fetcher::QueueContext::new(
                base_url.parse().map_err(Error::InvalidBaseUrl)?,
                match overwrite_existing {
                    true => crate::blob_fetcher::ConflictBehavior::Overwrite,
                    false => crate::blob_fetcher::ConflictBehavior::AskBlobfs,
                },
            ),
        )
        .await
        .map_err(Error::BlobPush)?
        .map_err(Error::BlobFetch)
        .map(|o| o.unwrap_or(0))
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("invalid base url")]
    InvalidBaseUrl(#[source] http::uri::InvalidUri),

    #[error("pushing a blob onto the fetch queue")]
    BlobPush(#[source] work_queue::Closed),

    #[error("forwarding to blob fetcher")]
    BlobFetch(#[source] Arc<crate::blob_fetcher::FetchError>),
}

impl From<&Error> for fpkg::ResolveError {
    fn from(err: &Error) -> Self {
        use Error::*;
        use fpkg::ResolveError as Err;
        match err {
            InvalidBaseUrl(_) => Err::InvalidUrl,
            BlobPush(_) => Err::Internal,
            BlobFetch(e) => e.as_ref().into(),
        }
    }
}
