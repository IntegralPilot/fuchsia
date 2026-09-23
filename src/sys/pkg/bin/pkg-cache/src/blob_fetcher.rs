// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg_ext as pkg;
use fidl_fuchsia_pkg_http as fpkg_http;
use http_uri_ext::HttpUriExt as _;
use std::sync::Arc;

mod retry;

#[derive(Clone, Copy, Debug, typed_builder::TypedBuilder)]
pub(crate) struct Params {
    header_network_timeout: zx::BootDuration,
    body_network_timeout: zx::BootDuration,
    download_resumption_attempts_limit: u32,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct QueueContext {
    blob_base_url: http::Uri,
    conflict_behavior: ConflictBehavior,
}

/// How the blob fetcher should behave when asked to fetch a blob that is already in blobfs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictBehavior {
    /// Ask blobfs if the existing blob should be overwritten, if not skip the fetch.
    AskBlobfs,
    /// Always perform the fetch and overwrite the existing blob.
    Overwrite,
}

impl QueueContext {
    pub(crate) fn new(blob_base_url: http::Uri, conflict_behavior: ConflictBehavior) -> Self {
        Self { blob_base_url, conflict_behavior }
    }
}

impl work_queue::TryMerge for QueueContext {
    fn try_merge(&mut self, other: Self) -> Result<(), Self> {
        if self.blob_base_url == other.blob_base_url
            && self.conflict_behavior == other.conflict_behavior
        {
            return Ok(());
        }
        Err(other)
    }
}

/// A clonable handle to the blob fetch queue.  When all clones of [`BlobFetcher`] are dropped, the
/// queue will fetch all remaining blobs in the queue and terminate its output stream.
#[derive(Clone)]
pub struct BlobFetcher {
    sender: work_queue::WorkSender<pkg::BlobId, QueueContext, Result<Option<u64>, Arc<FetchError>>>,
}

impl BlobFetcher {
    /// Creates an unbounded queue that will fetch up to `max_concurrency` blobs at once.
    /// Returns:
    ///   1. a Future to be awaited that processes the queue
    ///   2. a Self that enables pushing work onto the queue
    pub(crate) fn new(
        max_concurrency: usize,
        params: Params,
        blobfs_client: blobfs::Client,
        http_client: fpkg_http::ClientProxy,
    ) -> (impl Future<Output = ()>, Self) {
        let (queue, sender) = work_queue::work_queue(
            max_concurrency,
            move |blob_id: pkg::BlobId, context: QueueContext| {
                let http_client = http_client.clone();
                let blobfs_client = blobfs_client.clone();
                async move {
                    fetch_blob_with_retry(blob_id, context, params, &blobfs_client, &http_client)
                        .await
                        .map_err(Arc::new)
                }
            },
        );
        (queue.into_future(), BlobFetcher { sender })
    }

    /// Enqueue the given blob to be fetched, or attach to an existing request to fetch the blob.
    /// On success, returns the number of bytes downloaded, which is not necessarily equal to the
    /// uncompressed size of the blob, or None if the blob did not need to be downloaded.
    pub(crate) fn push(
        &self,
        blob_id: pkg::BlobId,
        context: QueueContext,
    ) -> impl Future<Output = Result<Result<Option<u64>, Arc<FetchError>>, work_queue::Closed>>
    {
        self.sender.push(blob_id, context)
    }

    /// Enqueue all the given blobs to be fetched, merging them with existing
    /// known tasks if possible, returning an iterator of the futures that will
    /// resolve to the results.
    ///
    /// This method is similar to, but more efficient than, mapping an iterator
    /// to `BlobFetcher::push`.
    pub(crate) fn push_all(
        &self,
        entries: impl Iterator<Item = (pkg::BlobId, QueueContext)>,
    ) -> impl Iterator<
        Item = impl Future<Output = Result<Result<Option<u64>, Arc<FetchError>>, work_queue::Closed>>,
    > {
        self.sender.push_all(entries)
    }
}

/// On success, returns the number of bytes downloaded, which is not necessarily equal to the
/// uncompressed size of the blob, or None if the blob did not need to be downloaded.
async fn fetch_blob_with_retry(
    blob_id: pkg::BlobId,
    QueueContext { blob_base_url, conflict_behavior }: QueueContext,
    Params {
        header_network_timeout,
        body_network_timeout,
        download_resumption_attempts_limit,
    }: Params,
    blobfs_client: &blobfs::Client,
    http_client: &fpkg_http::ClientProxy,
) -> Result<Option<u64>, FetchError> {
    match conflict_behavior {
        ConflictBehavior::AskBlobfs => {
            if blobfs_client.blob_present_and_up_to_date(&blob_id.into()).await {
                return Ok(None);
            }
        }
        ConflictBehavior::Overwrite => (),
    }
    let error_base = blob_base_url.clone();
    let blob_url = &blob_base_url
        .extend_dir_with_path(&blob_id.to_string())
        .map_err(|source| FetchError::BlobUrl { source, base_url: error_base, blob_id })?;
    fuchsia_backoff::retry_or_first_error(retry::blob_fetch(), || async move {
        let blob = blobfs_client
            .open_blob_for_write(&blob_id.into(), true)
            .await
            .map_err(FetchError::CreateBlob)?;
        http_client
            .download_blob(
                &blob_url.to_string(),
                blob,
                header_network_timeout.into_nanos(),
                body_network_timeout.into_nanos(),
                download_resumption_attempts_limit,
            )
            .await
            .map_err(FetchError::DownloadBlobFidl)?
            .map_err(FetchError::DownloadBlob)
            .map(Some)
    })
    .await
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error("could not create blob")]
    CreateBlob(#[source] blobfs::CreateError),

    #[error("creating blob url from base: {base_url}, id: {blob_id}")]
    BlobUrl {
        #[source]
        source: http_uri_ext::Error,
        base_url: http::Uri,
        blob_id: pkg::BlobId,
    },

    #[error("FIDL error while calling fuchsia.pkg.http.Client.DownloadBlob")]
    DownloadBlobFidl(#[source] fidl::Error),

    #[error("error while calling fuchsia.pkg.http.Client.DownloadBlob {0:?}")]
    DownloadBlob(fpkg_http::ClientDownloadBlobError),
}

impl FetchError {
    fn kind(&self) -> FetchErrorKind {
        use FetchError::*;
        match self {
            DownloadBlob(e) => match e {
                fpkg_http::ClientDownloadBlobError::NetworkRateLimit => {
                    FetchErrorKind::NetworkRateLimit
                }
                fpkg_http::ClientDownloadBlobError::Network => FetchErrorKind::Network,
                fpkg_http::ClientDownloadBlobError::NotFound => FetchErrorKind::NotFound,
                fpkg_http::ClientDownloadBlobError::NoSpace
                | fpkg_http::ClientDownloadBlobError::Other => FetchErrorKind::Other,
            },
            CreateBlob { .. } | BlobUrl { .. } | DownloadBlobFidl { .. } => FetchErrorKind::Other,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FetchErrorKind {
    NetworkRateLimit,
    Network,
    NotFound,
    Other,
}

impl From<&FetchError> for fidl_fuchsia_pkg::ResolveError {
    fn from(err: &FetchError) -> Self {
        use FetchError::*;
        use fidl_fuchsia_pkg::ResolveError as Err;
        match err {
            CreateBlob { .. } => Err::Io,
            BlobUrl { .. } => Err::Internal,
            DownloadBlobFidl { .. } => Err::Internal,
            DownloadBlob(e) => {
                use fidl_fuchsia_pkg_http::ClientDownloadBlobError::*;
                match e {
                    NoSpace => Err::NoSpace,
                    Network => Err::UnavailableBlob,
                    NotFound => Err::UnavailableBlob,
                    NetworkRateLimit => Err::Io,
                    Other => Err::Io,
                }
            }
        }
    }
}
