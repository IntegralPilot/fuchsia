// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::repository::Repository;
use crate::repository_manager::Stats;
use cobalt_sw_delivery_registry as metrics;
use fidl_contrib::protocol_connector::ProtocolSender;
use fidl_fuchsia_metrics::MetricEvent;
use fidl_fuchsia_pkg::{self as fpkg};
use fidl_fuchsia_pkg_ext::{self as pkg, BlobId, BlobInfo};
use fidl_fuchsia_pkg_http as fpkg_http;
use fuchsia_cobalt_builders::MetricEventExt as _;
use fuchsia_pkg::PackageDirectory;
use fuchsia_sync::Mutex;
use fuchsia_trace as ftrace;
use fuchsia_url::PackageVariant;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use futures::lock::Mutex as AsyncMutex;
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use http_uri_ext::HttpUriExt as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tuf::metadata::{MetadataPath, MetadataVersion, TargetPath};
use zx::Status;

pub use fidl_fuchsia_pkg_ext::BasePackageIndex;

mod inspect;
mod retry;

#[derive(Clone, Copy, Debug, typed_builder::TypedBuilder)]
pub struct BlobFetchParams {
    header_network_timeout: zx::BootDuration,
    body_network_timeout: zx::BootDuration,
    download_resumption_attempts_limit: u32,
}

impl BlobFetchParams {
    pub fn header_network_timeout(&self) -> zx::BootDuration {
        self.header_network_timeout
    }

    pub fn body_network_timeout(&self) -> zx::BootDuration {
        self.body_network_timeout
    }

    pub fn download_resumption_attempts_limit(&self) -> u32 {
        self.download_resumption_attempts_limit
    }
}

pub async fn cache_package<'a>(
    repo: Arc<AsyncMutex<Repository>>,
    url: &'a AbsolutePackageUrl,
    gc_protection: fpkg::GcProtection,
    cache: &'a pkg::cache::Client,
    blob_fetcher: &'a BlobFetcher,
    blob_base_url: http::Uri,
    cobalt_sender: ProtocolSender<MetricEvent>,
    trace_id: ftrace::Id,
) -> Result<(BlobId, PackageDirectory), CacheError> {
    let merkle = merkle_for_url(repo, url, cobalt_sender).await.map_err(CacheError::MerkleFor)?;
    // If a merkle pin was specified, use it, but only after having verified that the name and
    // variant exist in the TUF repo.  Note that this doesn't guarantee that the merkle pinned
    // package ever actually existed in the repo or that the merkle pin refers to the named
    // package.
    let merkle = if let Some(merkle_pin) = url.hash() { BlobId::from(merkle_pin) } else { merkle };

    let meta_far_blob = BlobInfo { blob_id: merkle, length: 0 };

    let mut get = cache.get(meta_far_blob, gc_protection)?;

    let blob_fetch_res = async {
        // Do not add the meta.far fetch to the queue if we are sure that the meta.far is already
        // cached to avoid blocking resolves of fully cached packages behind blob fetches (in the
        // case where the blob fetch queue is already at its concurrency limit).
        //
        // The NeededBlob created by this call to `make_open_meta_blob().open()` cannot be reused by
        // the queue because the queue could already contain a fetch request for the blob, which
        // would result in the NeededBlob being invalid by the time it is processed.
        // Once c++blobfs is removed and fxblob is changed to support concurrent writes of the same
        // blob that both complete successfully, we can pass the NeededBlob in, which will avoid
        // making an additional pkg-resolver -> pkg-cache -> blobfs -> pkg-cache -> pkg-resolver
        // FIDL loop, at the expense of sometimes downloading meta.fars multiple times.
        //
        // With fxblob, it is ok to open the blob for write (i.e. call `open` on the
        // DeferredOpenBlob) outside of the queue because fxblob allows concurrent creation attempts
        // as long as only one succeeds.
        //
        // With c++blob, it is *not* okay to open the blob for write (or even read) outside of the
        // fetch queue to test for presence because open connections even to partially written blobs
        // keep the blob alive. This means that this open creates the possibility for the following
        // race condition:
        // 1. this open occurs for resolve A
        // 2. a concurrent resolve attempt B (perhaps for a package that has this package as a
        //    subpackage) opens the blob for write
        // 3. resolve B obtains the blob size from the network
        // 4. resolve B calls Resize on the blob
        // 5. resolve B encounters an error that qualifes for retrying the fetch and closes the blob
        // 6. resolve B tries to open the blob for write again, which fails
        //
        // This is very unlikely to occur in practice because resolve A closes the connection
        // immediately, so this would need to be delayed somehow until after resolve B has made a
        // number of network operations and FIDL calls to remote services.
        //
        // Note that if this open occurs after resolve B has called Resize, the open will fail
        // instead of creating a blocking connection.
        //
        // The race could be prevented entirely by adding a method to NeededBlobs that uses
        // ReadDirents on /blob to check for blob presence.
        let fetch_meta_far = match get.make_open_meta_blob().open().await {
            Ok(Some(pkg::cache::NeededBlob { blob: _ })) => {
                // Dropping `blob` will cancel the creation.
                true
            }
            Ok(None) => false,
            // The open for write will fail on c++blob if the queue is writing the blob.
            Err(_) => true,
        };
        if fetch_meta_far {
            let _size = blob_fetcher
                .push(
                    merkle,
                    FetchBlobContext {
                        opener: get.make_open_meta_blob(),
                        blob_base_url: blob_base_url.clone(),
                        parent_trace_id: trace_id,
                    },
                )
                .await
                .expect("processor exists")
                .map_err(|e| CacheError::FetchMetaFar(e, merkle))?;
        }

        let mut fetches = FuturesUnordered::new();
        let mut missing_blobs = get.get_missing_blobs().fuse();
        let mut first_closed_error: Option<Arc<FetchError>> = None;

        loop {
            futures::select! {
                fetch_res = fetches.select_next_some() => {
                    match Result::<Result<_, Arc<FetchError>>, _>::expect(
                        fetch_res,
                        "processor exists"
                    ) {
                        Ok(_) => {}
                        Err(e) if e.is_unexpected_pkg_cache_closed() => {
                            first_closed_error.get_or_insert(e);
                        }
                        Err(e) => return Err(CacheError::FetchContentBlob(e, merkle)),
                    }
                }
                chunk = missing_blobs.select_next_some() => {
                    #[allow(clippy::needless_collect)]
                    // Not sure if this collect is significant -- without it, the
                    // compiler believes we are double-borrowing |get|.
                    let chunk = chunk?
                        .into_iter()
                        .map(|need| {
                            (
                                need.blob_id,
                                FetchBlobContext {
                                    // TODO(b/303737132) Consider checking if the blob is cached
                                    // before adding to the queue, like is done for meta.fars.
                                    opener: get.make_open_blob(need.blob_id),
                                    blob_base_url: blob_base_url.clone(),
                                    parent_trace_id: trace_id,
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    let () = fetches.extend(blob_fetcher.push_all(chunk.into_iter()));
                }
                complete => {
                    match first_closed_error {
                        None =>  return Ok(()),
                        Some(e) => return Err(CacheError::FetchContentBlob(e, merkle)),
                    }
                }
            }
        }
    }
    .await;

    match blob_fetch_res {
        Ok(()) => Ok((merkle, get.finish().await?)),
        Err(e) => {
            get.abort().await;
            Err(e)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CacheError {
    #[error("fidl error")]
    Fidl(#[from] fidl::Error),

    #[error("while looking up merkle root for package")]
    MerkleFor(#[source] MerkleForError),

    #[error("while listing needed blobs for package")]
    ListNeeds(#[from] pkg::cache::ListMissingBlobsError),

    #[error("while fetching the meta.far: {1}")]
    FetchMetaFar(#[source] Arc<FetchError>, BlobId),

    #[error("while fetching content blob for meta.far {1}")]
    FetchContentBlob(#[source] Arc<FetchError>, BlobId),

    #[error("Get() request failed")]
    Get(#[from] pkg::cache::GetError),

    #[error("opening a blob from the blobstore")]
    Open(#[from] pkg::cache::OpenBlobError),
}

pub(crate) trait ToResolveError {
    fn to_resolve_error(&self) -> pkg::ResolveError;
}

impl ToResolveError for Status {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match *self {
            Status::ACCESS_DENIED => pkg::ResolveError::AccessDenied,
            Status::IO => pkg::ResolveError::Io,
            Status::NOT_FOUND => pkg::ResolveError::PackageNotFound,
            Status::NO_SPACE => pkg::ResolveError::NoSpace,
            Status::UNAVAILABLE => pkg::ResolveError::UnavailableBlob,
            Status::INVALID_ARGS => pkg::ResolveError::InvalidUrl,
            Status::INTERNAL => pkg::ResolveError::Internal,
            _ => pkg::ResolveError::Internal,
        }
    }
}

pub(crate) trait ToResolveStatus {
    fn to_resolve_status(&self) -> Status;
}
impl ToResolveStatus for pkg::ResolveError {
    fn to_resolve_status(&self) -> Status {
        use pkg::ResolveError::*;
        match *self {
            AccessDenied => Status::ACCESS_DENIED,
            Io => Status::IO,
            PackageNotFound | RepoNotFound | BlobNotFound => Status::NOT_FOUND,
            NoSpace => Status::NO_SPACE,
            UnavailableBlob | UnavailableRepoMetadata => Status::UNAVAILABLE,
            InvalidUrl | InvalidContext => Status::INVALID_ARGS,
            Internal => Status::INTERNAL,
        }
    }
}

// From resolver.fidl:
// * `ZX_ERR_INTERNAL` if the resolver encountered an otherwise unspecified error
//   while handling the request
// * `ZX_ERR_NOT_FOUND` if the package does not exist.
// * `ZX_ERR_ADDRESS_UNREACHABLE` if the resolver does not know about the repo.
impl ToResolveError for CacheError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match self {
            CacheError::Fidl(_) => pkg::ResolveError::Io,
            CacheError::MerkleFor(err) => err.to_resolve_error(),
            CacheError::ListNeeds(err) => err.to_resolve_error(),
            CacheError::FetchMetaFar(err, ..) => err.to_resolve_error(),
            CacheError::FetchContentBlob(err, _) => err.to_resolve_error(),
            CacheError::Get(err) => err.to_resolve_error(),
            CacheError::Open(err) => err.to_resolve_error(),
        }
    }
}

impl ToResolveStatus for MerkleForError {
    fn to_resolve_status(&self) -> Status {
        match self {
            MerkleForError::MetadataNotFound { .. } => Status::INTERNAL,
            MerkleForError::TargetNotFound(_) => Status::NOT_FOUND,
            MerkleForError::InvalidTargetPath(_) => Status::INTERNAL,
            // FIXME(42326) when tuf::Error gets an HTTP error variant, this should be mapped to Status::UNAVAILABLE
            MerkleForError::FetchTargetDescription(..) => Status::INTERNAL,
            MerkleForError::NoCustomMetadata => Status::INTERNAL,
            MerkleForError::SerdeError(_) => Status::INTERNAL,
        }
    }
}

impl ToResolveError for MerkleForError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match self {
            MerkleForError::MetadataNotFound { .. } => pkg::ResolveError::Internal,
            MerkleForError::TargetNotFound(_) => pkg::ResolveError::PackageNotFound,
            MerkleForError::InvalidTargetPath(_) => pkg::ResolveError::Internal,
            // FIXME(42326) when tuf::Error gets an HTTP error variant, this should be mapped to Status::UNAVAILABLE
            MerkleForError::FetchTargetDescription(..) => pkg::ResolveError::Internal,
            MerkleForError::NoCustomMetadata => pkg::ResolveError::Internal,
            MerkleForError::SerdeError(_) => pkg::ResolveError::Internal,
        }
    }
}

impl ToResolveError for pkg::cache::OpenError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match self {
            pkg::cache::OpenError::NotFound => pkg::ResolveError::PackageNotFound,
            pkg::cache::OpenError::UnexpectedResponse(_) | pkg::cache::OpenError::Fidl(_) => {
                pkg::ResolveError::Internal
            }
        }
    }
}

impl ToResolveError for pkg::cache::GetError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match self {
            pkg::cache::GetError::UnexpectedResponse(_) => pkg::ResolveError::Internal,
            pkg::cache::GetError::Fidl(_) => pkg::ResolveError::Internal,
        }
    }
}

impl ToResolveError for pkg::cache::OpenBlobError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match self {
            pkg::cache::OpenBlobError::OutOfSpace => pkg::ResolveError::NoSpace,
            pkg::cache::OpenBlobError::ConcurrentWrite => pkg::ResolveError::Internal,
            pkg::cache::OpenBlobError::UnspecifiedIo => pkg::ResolveError::Io,
            pkg::cache::OpenBlobError::Internal => pkg::ResolveError::Internal,
            pkg::cache::OpenBlobError::Fidl(_) => pkg::ResolveError::Internal,
        }
    }
}

impl ToResolveError for pkg::cache::ListMissingBlobsError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        pkg::ResolveError::Internal
    }
}

impl ToResolveError for pkg::cache::TruncateBlobError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        use pkg::cache::TruncateBlobError::*;
        match self {
            NoSpace => pkg::ResolveError::NoSpace,
            Fidl(_) | CreateBlobWriter(_) => pkg::ResolveError::Io,
            BadState => pkg::ResolveError::Internal,
        }
    }
}

impl ToResolveError for pkg::cache::WriteBlobError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        use pkg::cache::WriteBlobError::*;
        match self {
            Corrupt | Fidl(_) | FxBlob(_) => pkg::ResolveError::Io,
            NoSpace => pkg::ResolveError::NoSpace,
        }
    }
}

impl ToResolveError for pkg::cache::GetAlreadyCachedError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        if self.was_not_cached() {
            pkg::ResolveError::PackageNotFound
        } else {
            pkg::ResolveError::Internal
        }
    }
}

impl ToResolveError for FetchError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        use FetchError::*;
        match self {
            CreateBlob(e) => e.to_resolve_error(),
            BlobWritten(_) => pkg::ResolveError::Internal,
            BlobUrl(_) => pkg::ResolveError::Internal,
            DownloadBlobFidl(_) => pkg::ResolveError::Internal,
            BlobWrittenFidl(_) => pkg::ResolveError::Internal,
            DownloadBlob(fpkg_http::ClientDownloadBlobError::NoSpace) => pkg::ResolveError::NoSpace,
            DownloadBlob(fpkg_http::ClientDownloadBlobError::Network) => {
                pkg::ResolveError::UnavailableBlob
            }
            DownloadBlob(fpkg_http::ClientDownloadBlobError::NotFound) => {
                pkg::ResolveError::UnavailableBlob
            }
            DownloadBlob(fpkg_http::ClientDownloadBlobError::NetworkRateLimit) => {
                pkg::ResolveError::Io
            }
            DownloadBlob(fpkg_http::ClientDownloadBlobError::Other) => pkg::ResolveError::Io,
        }
    }
}

impl From<&MerkleForError> for metrics::MerkleForUrlMigratedMetricDimensionResult {
    fn from(e: &MerkleForError) -> metrics::MerkleForUrlMigratedMetricDimensionResult {
        use metrics::MerkleForUrlMigratedMetricDimensionResult as EventCodes;
        match e {
            MerkleForError::MetadataNotFound { .. } => EventCodes::TufError,
            MerkleForError::TargetNotFound(_) => EventCodes::NotFound,
            MerkleForError::FetchTargetDescription(..) => EventCodes::TufError,
            MerkleForError::InvalidTargetPath(_) => EventCodes::InvalidTargetPath,
            MerkleForError::NoCustomMetadata => EventCodes::NoCustomMetadata,
            MerkleForError::SerdeError(_) => EventCodes::SerdeError,
        }
    }
}

pub async fn merkle_for_url(
    repo: Arc<AsyncMutex<Repository>>,
    url: &AbsolutePackageUrl,
    mut cobalt_sender: ProtocolSender<MetricEvent>,
) -> Result<BlobId, MerkleForError> {
    // TODO(https://fxbug.dev/338012491): Stop adding variants to package URLs.
    let target_path = TargetPath::new(format!(
        "{}/{}",
        url.name(),
        url.variant().map(|v| v.as_ref()).unwrap_or(PackageVariant::ZERO_STR)
    ))
    .map_err(MerkleForError::InvalidTargetPath)?;
    let mut repo = repo.lock().await;
    let res = repo.get_merkle_at_path(&target_path).await;
    cobalt_sender.send(
        MetricEvent::builder(metrics::MERKLE_FOR_URL_MIGRATED_METRIC_ID)
            .with_event_codes(match &res {
                Ok(_) => metrics::MerkleForUrlMigratedMetricDimensionResult::Success,
                Err(res) => res.into(),
            })
            .as_occurrence(1),
    );
    res.map(|custom| custom.merkle())
}

#[derive(Debug, thiserror::Error)]
pub enum MerkleForError {
    #[error("the repository metadata {path} at version {version} was not found in the repository")]
    MetadataNotFound { path: MetadataPath, version: MetadataVersion },

    #[error("the package {0} was not found in the repository")]
    TargetNotFound(TargetPath),

    #[error("unexpected tuf error when fetching target description for {0:?}")]
    FetchTargetDescription(String, #[source] tuf::error::Error),

    #[error("the target path is not safe")]
    InvalidTargetPath(#[source] tuf::error::Error),

    #[error("the target description does not have custom metadata")]
    NoCustomMetadata,

    #[error("serde value could not be converted")]
    SerdeError(#[source] serde_json::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub struct FetchBlobContext {
    opener: pkg::cache::DeferredOpenBlob,
    blob_base_url: http::Uri,
    parent_trace_id: ftrace::Id,
}

impl FetchBlobContext {
    pub fn new(
        opener: pkg::cache::DeferredOpenBlob,
        blob_base_url: http::Uri,
        parent_trace_id: ftrace::Id,
    ) -> Self {
        Self { opener, blob_base_url, parent_trace_id }
    }
}

impl work_queue::TryMerge for FetchBlobContext {
    fn try_merge(&mut self, other: Self) -> Result<(), Self> {
        // The NeededBlobs protocol requires pkg-resolver to attempt to open each blob associated
        // with a Get() request, and attempting to open a blob being written by another Get()
        // operation would fail. So, this queue is needed to enforce a concurrency limit and ensure
        // a blob is not written by more than one fetch at a time.

        // Only requests that are the same request on the same channel can be merged, and the
        // packageresolver should never enqueue such duplicate requests, so, realistically,
        // try_merge always returns Err(other).
        if self.opener != other.opener {
            return Err(other);
        }

        // Merge these contexts if the blob_base_urls are equivalent.
        if self.blob_base_url != other.blob_base_url {
            return Err(other);
        }

        // Contexts are mergeable.
        Ok(())
    }
}

/// A clonable handle to the blob fetch queue.  When all clones of
/// [`BlobFetcher`] are dropped, the queue will fetch all remaining blobs in
/// the queue and terminate its output stream.
#[derive(Clone)]
pub struct BlobFetcher {
    sender: work_queue::WorkSender<BlobId, FetchBlobContext, Result<u64, Arc<FetchError>>>,
}

impl BlobFetcher {
    /// Creates an unbounded queue that will fetch up to `max_concurrency` blobs at once.
    /// Returns:
    ///   1. a Future to be awaited that processes the queue
    ///   2. a Self that enables pushing work onto the queue
    pub fn new(
        client: fpkg_http::ClientProxy,
        node: fuchsia_inspect::Node,
        max_concurrency: usize,
        stats: Arc<Mutex<Stats>>,
        blob_fetch_params: BlobFetchParams,
    ) -> (impl Future<Output = ()>, Self) {
        let weak_node = node.clone_weak();
        let inspect = inspect::BlobFetcher::from_node_and_params(node, &blob_fetch_params);

        let (queue, sender) = work_queue::work_queue(
            max_concurrency,
            move |merkle: BlobId, context: FetchBlobContext| {
                let inspect = inspect.fetch(&merkle);
                let client = client.clone();
                let stats = Arc::clone(&stats);

                async move {
                    fetch_blob(inspect, &client, stats, merkle, context, blob_fetch_params)
                        .map_err(Arc::new)
                        .await
                }
            },
        );
        weak_node.record_lazy_child("raw_queue", queue.record_lazy_inspect());

        (queue.into_future(), BlobFetcher { sender })
    }

    /// Enqueue the given blob to be fetched, or attach to an existing request to
    /// fetch the blob.
    pub fn push(
        &self,
        blob_id: BlobId,
        context: FetchBlobContext,
    ) -> impl Future<Output = Result<Result<u64, Arc<FetchError>>, work_queue::Closed>> {
        self.sender.push(blob_id, context)
    }

    /// Enqueue all the given blobs to be fetched, merging them with existing
    /// known tasks if possible, returning an iterator of the futures that will
    /// resolve to the results.
    ///
    /// This method is similar to, but more efficient than, mapping an iterator
    /// to `BlobFetcher::push`.
    pub fn push_all(
        &self,
        entries: impl Iterator<Item = (BlobId, FetchBlobContext)>,
    ) -> impl Iterator<
        Item = impl Future<Output = Result<Result<u64, Arc<FetchError>>, work_queue::Closed>>,
    > {
        self.sender.push_all(entries)
    }
}

async fn fetch_blob(
    inspect: inspect::NeedsRemoteType,
    client: &fpkg_http::ClientProxy,
    stats: Arc<Mutex<Stats>>,
    merkle: BlobId,
    context: FetchBlobContext,
    blob_fetch_params: BlobFetchParams,
) -> Result<u64, FetchError> {
    let trace_id = ftrace::Id::new();
    let FetchBlobContext { blob_base_url, parent_trace_id, opener } = context;
    let guard = ftrace::async_enter!(
        trace_id,
        c"app",
        c"fetch_blob_http",
        // Async tracing does not support multiple concurrent child durations, so we create
        // a new top-level duration and attach the parent duration as metadata.
        "parent_trace_id" => u64::from(parent_trace_id),
        "hash" => merkle.to_string().as_str()
    );
    let inspect = inspect.http();
    let res = fetch_blob_http(
        &inspect,
        client,
        blob_base_url,
        merkle,
        &opener,
        blob_fetch_params,
        &stats,
        trace_id,
    )
    .await;
    if let Some(o) = guard {
        o.end(&[ftrace::ArgValue::of("result", format!("{res:?}").as_str())])
    }
    res
}

async fn fetch_blob_http(
    inspect: &inspect::TriggerAttempt<inspect::Http>,
    client: &fpkg_http::ClientProxy,
    blob_base_url: http::Uri,
    merkle: BlobId,
    opener: &pkg::cache::DeferredOpenBlob,
    blob_fetch_params: BlobFetchParams,
    stats: &Mutex<Stats>,
    trace_id: ftrace::Id,
) -> Result<u64, FetchError> {
    let mirror_stats = &stats.lock().for_mirror(blob_base_url.to_string());
    let blob_url = &make_blob_url(blob_base_url, &merkle).map_err(FetchError::BlobUrl)?;
    inspect.set_mirror(&blob_url.to_string());
    let flaked = &AtomicBool::new(false);

    fuchsia_backoff::retry_or_first_error(retry::blob_fetch(), || {
        async move {
            let res = async {
                let inspect = inspect.attempt();
                inspect.state(inspect::Http::CreateBlob);
                if let Some(pkg::cache::NeededBlob { blob }) =
                    opener.open().await.map_err(FetchError::CreateBlob)?
                {
                    inspect.state(inspect::Http::DownloadBlob);
                    let guard = ftrace::async_enter!(
                        trace_id,
                        c"app",
                        c"download_blob",
                        "hash" => merkle.to_string().as_str()
                    );
                    let res = download_blob(client, blob_url, blob, blob_fetch_params).await;
                    let (size_str, status_str) = match &res {
                        Ok(size) => (size.to_string(), "success".to_string()),
                        Err(e) => ("no size because download failed".to_string(), e.to_string()),
                    };
                    if let Some(o) = guard {
                        o.end(&[
                            ftrace::ArgValue::of("size", size_str.as_str()),
                            ftrace::ArgValue::of("status", status_str.as_str()),
                        ])
                    }
                    inspect.state(inspect::Http::CloseBlob);
                    // `blob` is dropped when download_blob returns which cancels the creation.
                    res
                } else {
                    Ok(0)
                }
            }
            .await;

            match res.as_ref().map_err(FetchError::kind) {
                Err(FetchErrorKind::NetworkRateLimit) => {
                    mirror_stats.network_rate_limits().increment();
                }
                Err(FetchErrorKind::Network) => {
                    flaked.store(true, Ordering::SeqCst);
                }
                Err(FetchErrorKind::NotFound | FetchErrorKind::Other) => {}
                Ok(_) => {
                    if flaked.load(Ordering::SeqCst) {
                        mirror_stats.network_blips().increment();
                    }
                }
            }

            res
        }
    })
    .await
}

fn make_blob_url(
    blob_base_url: http::Uri,
    merkle: &BlobId,
) -> Result<hyper::Uri, http_uri_ext::Error> {
    blob_base_url.extend_dir_with_path(&merkle.to_string())
}

// On success, returns the size of the downloaded blob in bytes (useful for tracing).
async fn download_blob(
    client: &fpkg_http::ClientProxy,
    uri: &http::Uri,
    dest: pkg::cache::Blob<pkg::cache::NeedsTruncate>,
    blob_fetch_params: BlobFetchParams,
) -> Result<u64, FetchError> {
    let (needed_blobs, blob_id, writer) = dest.deconstruct();
    let size = client
        .download_blob(
            &uri.to_string(),
            writer,
            blob_fetch_params.header_network_timeout().into_nanos(),
            blob_fetch_params.body_network_timeout().into_nanos(),
            blob_fetch_params.download_resumption_attempts_limit(),
        )
        .await
        .map_err(FetchError::DownloadBlobFidl)?
        .map_err(FetchError::DownloadBlob)?;
    let () = needed_blobs
        .blob_written(&blob_id.into())
        .await
        .map_err(FetchError::BlobWrittenFidl)?
        .map_err(FetchError::BlobWritten)?;
    Ok(size)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    #[error("could not create blob")]
    CreateBlob(#[source] pkg::cache::OpenBlobError),

    #[error("blob url error")]
    BlobUrl(#[source] http_uri_ext::Error),

    #[error("FIDL error while calling fuchsia.pkg.http.Client.DownloadBlob")]
    DownloadBlobFidl(#[source] fidl::Error),

    #[error("error while calling fuchsia.pkg.http.Client.DownloadBlob {0:?}")]
    DownloadBlob(fpkg_http::ClientDownloadBlobError),

    #[error("FIDL error while calling fuchsia.pkg.cache.NeededBlobs.BlobWritten")]
    BlobWrittenFidl(#[source] fidl::Error),

    #[error("error while calling fuchsia.pkg.cache.NeededBlobs.BlobWritten {0:?}")]
    BlobWritten(fpkg::BlobWrittenError),
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
            CreateBlob { .. }
            | BlobWritten { .. }
            | BlobUrl { .. }
            | DownloadBlobFidl { .. }
            | BlobWrittenFidl { .. } => FetchErrorKind::Other,
        }
    }

    fn to_pkg_cache_fidl_error(&self) -> Option<&fidl::Error> {
        use FetchError::*;
        match self {
            CreateBlob(pkg::cache::OpenBlobError::Fidl(e))
            | DownloadBlobFidl(e)
            | BlobWrittenFidl(e) => Some(e),
            CreateBlob(_) | DownloadBlob(_) | BlobWritten(_) | BlobUrl { .. } => None,
        }
    }

    fn is_unexpected_pkg_cache_closed(&self) -> bool {
        matches!(self.to_pkg_cache_fidl_error(), Some(fidl::Error::ClientChannelClosed { .. }))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FetchErrorKind {
    NetworkRateLimit,
    Network,
    NotFound,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Uri;

    #[test]
    fn test_make_blob_url() {
        let merkle = "00112233445566778899aabbccddeeffffeeddccbbaa99887766554433221100"
            .parse::<BlobId>()
            .unwrap();

        assert_eq!(
            make_blob_url("http://example.com".parse::<Uri>().unwrap(), &merkle,).unwrap(),
            format!("http://example.com/{merkle}").parse::<Uri>().unwrap()
        );

        assert_eq!(
            make_blob_url("http://example.com/noslash".parse::<Uri>().unwrap(), &merkle,).unwrap(),
            format!("http://example.com/noslash/{merkle}").parse::<Uri>().unwrap()
        );

        assert_eq!(
            make_blob_url("http://example.com/slash/".parse::<Uri>().unwrap(), &merkle,).unwrap(),
            format!("http://example.com/slash/{merkle}").parse::<Uri>().unwrap()
        );

        assert_eq!(
            make_blob_url("http://example.com/twoslashes//".parse::<Uri>().unwrap(), &merkle,)
                .unwrap(),
            format!("http://example.com/twoslashes//{merkle}").parse::<Uri>().unwrap()
        );

        // IPv6 zone id
        assert_eq!(
            make_blob_url(
                "http://[fe80::e022:d4ff:fe13:8ec3%252]:8083/blobs/".parse::<Uri>().unwrap(),
                &merkle,
            )
            .unwrap(),
            format!("http://[fe80::e022:d4ff:fe13:8ec3%252]:8083/blobs/{merkle}")
                .parse::<Uri>()
                .unwrap()
        );
    }
}
