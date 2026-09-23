// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg as fpkg;
use fuchsia_inspect as finspect;
use fuchsia_inspect::NumericProperty as _;
use std::collections::HashSet;
use std::sync::Arc;

/// Work-queue based package fetcher. When all clones of [`PackageFetcher`] are dropped, the queue
/// will fetch all remaining packages and terminate its output stream.
#[derive(Clone, Debug)]
pub struct PackageFetcher {
    sender: work_queue::WorkSender<
        fuchsia_hash::Hash,
        QueueContext,
        Result<Arc<crate::RootDir>, Arc<Error>>,
    >,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueContext {
    blob_source: http::Uri,
    gc_protection: fpkg::GcProtection,
}

impl work_queue::TryMerge for QueueContext {
    // Do not merge Contexts with differing GC protection. Clients depend on the different GC
    // protection behaviors.
    // Do not merge Contexts with differing blob sources, the blob sources may have different
    // versions of the blobs, e.g. different compression levels.
    fn try_merge(&mut self, other: Self) -> Result<(), Self> {
        if self.gc_protection == other.gc_protection && self.blob_source == other.blob_source {
            Ok(())
        } else {
            Err(other)
        }
    }
}

impl PackageFetcher {
    /// Creates an unbounded queue that will fetch up to `max_concurrency` packages at once.
    /// Returns:
    ///   1. a Future to be awaited that processes the queue
    ///   2. a Self that enables pushing work onto the queue
    pub(crate) fn new(
        max_concurrency: usize,
        package_index: Arc<async_lock::RwLock<crate::index::PackageIndex>>,
        blobfs_client: blobfs::Client,
        blob_fetcher: crate::blob_fetcher::BlobFetcher,
        root_dir_factory: crate::root_dir::RootDirFactory,
        open_packages: crate::RootDirCache,
        inspect: finspect::Node,
    ) -> (impl Future<Output = ()>, Self) {
        let fetch_count = std::sync::atomic::AtomicU64::new(0);
        let (queue, sender) = work_queue::work_queue(
            max_concurrency,
            move |pkg_id: fuchsia_hash::Hash, context: QueueContext| {
                let package_index = package_index.clone();
                let blobfs_client = blobfs_client.clone();
                let blob_fetcher = blob_fetcher.clone();
                let root_dir_factory = root_dir_factory.clone();
                let open_packages = open_packages.clone();
                let inspect = inspect.create_child(
                    fetch_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string(),
                );
                async move {
                    fetch(
                        pkg_id,
                        context.blob_source,
                        context.gc_protection,
                        package_index.as_ref(),
                        &blobfs_client,
                        &blob_fetcher,
                        &root_dir_factory,
                        &open_packages,
                        inspect,
                    )
                    .await
                }
            },
        );
        (queue.into_future(), Self { sender })
    }

    pub(crate) async fn fetch(
        &self,
        pkg_id: fuchsia_hash::Hash,
        blob_source: http::Uri,
        gc_protection: fpkg::GcProtection,
    ) -> Result<Arc<crate::RootDir>, Arc<Error>> {
        self.sender
            .push(pkg_id, QueueContext { blob_source, gc_protection })
            .await
            .map_err(Error::PushQueue)?
    }
}

async fn fetch(
    pkg_id: fuchsia_hash::Hash,
    blob_source: http::Uri,
    gc_protection: fpkg::GcProtection,
    package_index: &async_lock::RwLock<crate::index::PackageIndex>,
    blobfs_client: &blobfs::Client,
    blob_fetcher: &crate::blob_fetcher::BlobFetcher,
    root_dir_factory: &crate::root_dir::RootDirFactory,
    open_packages: &crate::RootDirCache,
    inspect: finspect::Node,
) -> Result<Arc<crate::RootDir>, Arc<Error>> {
    let gc_guard = package_index.write().await.start_writing(pkg_id, gc_protection);
    let fetch_ret = fetch_impl(
        pkg_id,
        blob_source,
        package_index,
        gc_protection,
        blobfs_client,
        blob_fetcher,
        root_dir_factory,
        open_packages,
        inspect,
    )
    .await;
    let stop_ret = package_index.write().await.stop_writing(gc_guard);
    match (fetch_ret, stop_ret) {
        (fetch_ret, Ok(())) => fetch_ret,
        (Ok(_), Err(e)) => Err(Error::ClearWritingIndex(e)),
        (Err(fetch_err), Err(stop_err)) => {
            Err(Error::FetchAndClearFailed { source: Box::new(fetch_err), stop_err })
        }
    }
    .map_err(Arc::new)
}

async fn fetch_impl(
    pkg_id: fuchsia_merkle::Hash,
    blob_source: http::Uri,
    package_index: &async_lock::RwLock<crate::index::PackageIndex>,
    gc_protection: fpkg::GcProtection,
    blobfs_client: &blobfs::Client,
    blob_fetcher: &crate::blob_fetcher::BlobFetcher,
    root_dir_factory: &crate::root_dir::RootDirFactory,
    open_packages: &crate::RootDirCache,
    inspect: finspect::Node,
) -> Result<Arc<crate::RootDir>, Error> {
    inspect.record_int("start_boot_ns", zx::BootInstant::get().into_nanos());
    inspect.record_string("package_hash", pkg_id.to_string());
    inspect.record_string("gc_protection", format!("{gc_protection:?}"));
    inspect.record_string("blob_source", blob_source.to_string());
    let inspect_outstanding = inspect.create_uint("known_outstanding_blobs", 1);
    let mut queue = std::collections::VecDeque::from([pkg_id]);
    let mut queued = HashSet::from([pkg_id]);
    let context = crate::blob_fetcher::QueueContext::new(
        blob_source,
        crate::blob_fetcher::ConflictBehavior::AskBlobfs,
    );
    let mut ret = None;
    while let Some(blob_id) = queue.pop_front() {
        // The blob fetcher performs this check as well, but check here to avoid blocking the fetch
        // of an already cached package on a full blob fetch queue.
        if !blobfs_client.blob_present_and_up_to_date(&blob_id).await {
            let _: Option<u64> = blob_fetcher
                .push(blob_id.into(), context.clone())
                .await
                .map_err(Error::BlobPush)?
                .map_err(Error::BlobFetch)?;
        }
        inspect_outstanding.subtract(1);
        let root_dir = root_dir_factory
            .create(blob_id)
            .await
            .map_err(|source| Error::CreatingRootDir { source, pkg: blob_id })?;
        let subpackages = root_dir
            .subpackages()
            .await
            .map_err(|source| Error::ReadingSubpackages { source, pkg: blob_id })?
            .into_hashes_undeduplicated()
            .collect::<Vec<_>>();
        let () = package_index
            .write()
            .await
            .add_blobs(
                pkg_id,
                HashSet::from_iter(subpackages.as_slice().iter().copied()),
                gc_protection,
            )
            .map_err(Error::ProtectBlobs)?;
        for sub in subpackages {
            if queued.insert(sub) {
                queue.push_back(sub);
                inspect_outstanding.add(1);
            }
        }
        let content = root_dir.external_file_hashes().copied().collect::<HashSet<_>>();
        let () = package_index
            .write()
            .await
            .add_blobs(pkg_id, content.clone(), gc_protection)
            .map_err(Error::ProtectBlobs)?;
        let mut missing_content = vec![];
        for content_id in content.into_iter() {
            if !blobfs_client.blob_present_and_up_to_date(&content_id).await {
                missing_content.push(content_id);
            }
        }
        inspect_outstanding.add(missing_content.len() as u64);
        for fut in
            blob_fetcher.push_all(missing_content.into_iter().map(|h| (h.into(), context.clone())))
        {
            let _: Option<u64> = fut.await.map_err(Error::BlobPush)?.map_err(Error::BlobFetch)?;
            inspect_outstanding.subtract(1);
        }
        ret.get_or_insert(root_dir);
    }
    let root_dir = ret.expect("queue starts with an entry");
    Ok(match gc_protection {
        fpkg::GcProtection::Retained => Arc::new(root_dir),
        fpkg::GcProtection::OpenPackageTracking => open_packages
            .get_or_insert(pkg_id, Some(root_dir))
            .await
            .map_err(Error::CreatingTrackedRootDir)?,
    })
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("pushing a blob onto the fetch queue")]
    BlobPush(#[source] work_queue::Closed),

    #[error("fetching blob")]
    BlobFetch(#[source] Arc<crate::blob_fetcher::FetchError>),

    #[error("creating root dir for {pkg}")]
    CreatingRootDir {
        #[source]
        source: package_directory::Error,
        pkg: fuchsia_merkle::Hash,
    },

    #[error("reading subpackages for {pkg}")]
    ReadingSubpackages {
        #[source]
        source: package_directory::SubpackagesError,
        pkg: fuchsia_merkle::Hash,
    },

    #[error("adding blobs to package index")]
    ProtectBlobs(#[source] crate::index::AddBlobsError),

    #[error("creating root dir with open package tracking")]
    CreatingTrackedRootDir(#[source] package_directory::Error),

    #[error("clearing the writing index after fetch complete")]
    ClearWritingIndex(#[source] crate::index::StopError),

    #[error("clearing the writing index failed {stop_err:?} after the fetch failed")]
    FetchAndClearFailed {
        #[source]
        source: Box<Error>,
        stop_err: crate::index::StopError,
    },

    #[error("pushing request into queue")]
    PushQueue(#[source] work_queue::Closed),
}

impl From<&Error> for fpkg::ResolveError {
    fn from(err: &Error) -> Self {
        use Error::*;
        use fpkg::ResolveError as Err;
        match err {
            BlobPush(_) => Err::Internal,
            BlobFetch(e) => e.as_ref().into(),
            CreatingRootDir { .. } => Err::Io,
            ReadingSubpackages { .. } => Err::Io,
            ProtectBlobs(_) => Err::Internal,
            CreatingTrackedRootDir(_) => Err::Io,
            ClearWritingIndex(_) => Err::Internal,
            FetchAndClearFailed { source, .. } => (&**source).into(),
            PushQueue(_) => Err::Internal,
        }
    }
}

impl From<&Error> for fidl_fuchsia_component_resolution::ResolverError {
    fn from(err: &Error) -> Self {
        use Error::*;
        use fidl_fuchsia_component_resolution::ResolverError as Err;
        match err {
            BlobPush(_) => Err::Internal,
            BlobFetch(e) => fetch_to_component_resolve_err(e),
            CreatingRootDir { .. } => Err::Io,
            ReadingSubpackages { .. } => Err::Io,
            ProtectBlobs(_) => Err::Internal,
            CreatingTrackedRootDir(_) => Err::Io,
            ClearWritingIndex(_) => Err::Internal,
            FetchAndClearFailed { source, .. } => (&**source).into(),
            PushQueue(_) => Err::Internal,
        }
    }
}

fn fetch_to_component_resolve_err(
    err: &crate::blob_fetcher::FetchError,
) -> fidl_fuchsia_component_resolution::ResolverError {
    use crate::blob_fetcher::FetchError::*;
    use fidl_fuchsia_component_resolution::ResolverError as Err;
    match err {
        CreateBlob { .. } => Err::Io,
        BlobUrl { .. } => Err::Internal,
        DownloadBlobFidl { .. } => Err::Internal,
        DownloadBlob(e) => {
            use fidl_fuchsia_pkg_http::ClientDownloadBlobError::*;
            match e {
                NoSpace => Err::NoSpace,
                Network => Err::Io,
                NotFound => Err::ResourceUnavailable,
                NetworkRateLimit => Err::Io,
                Other => Err::Io,
            }
        }
    }
}
