// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::frozen_index::{BaseIndex, CacheIndex};
use crate::index::PackageIndex;
use crate::upgradable_packages::UpgradablePackages;
use anyhow::{Error, anyhow};
use cobalt_sw_delivery_registry as metrics;
use fidl::endpoints::ServerEnd;
use fidl::prelude::*;
use fidl_contrib::protocol_connector::ProtocolSender;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics::MetricEvent;
use fidl_fuchsia_pkg::{
    self as fpkg, NeededBlobsMarker, NeededBlobsRequest, NeededBlobsRequestStream,
    PackageCacheRequest, PackageCacheRequestStream, PackageIndexEntry,
    PackageIndexIteratorRequestStream,
};
use fidl_fuchsia_pkg_ext::{
    BlobId, BlobInfo, serve_fidl_iterator_from_slice, serve_fidl_iterator_from_stream,
};
use fuchsia_async::Task;
use fuchsia_cobalt_builders::MetricEventExt as _;
use fuchsia_hash::Hash;
use fuchsia_inspect::{self as finspect, NumericProperty as _, Property as _};
use fuchsia_trace as ftrace;
use fuchsia_url::fuchsia_pkg::UnpinnedAbsolutePackageUrl;
use futures::{FutureExt as _, TryFutureExt as _, TryStreamExt as _};
use log::{error, warn};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use zx::Status;

mod missing_blobs;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    package_index: Arc<async_lock::RwLock<PackageIndex>>,
    blobfs: blobfs::Client,
    root_dir_factory: crate::RootDirFactory,
    base_packages: Arc<BaseIndex>,
    cache_packages: Arc<CacheIndex>,
    upgradable_packages: Option<Arc<UpgradablePackages>>,
    executability_restrictions: system_image::ExecutabilityRestrictions,
    scope: package_directory::ExecutionScope,
    open_packages: crate::RootDirCache,
    stream: PackageCacheRequestStream,
    cobalt_sender: ProtocolSender<MetricEvent>,
    serve_id: Arc<AtomicU32>,
    get_node: Arc<finspect::Node>,
) -> Result<(), Error> {
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, |event| async {
            let cobalt_sender = cobalt_sender.clone();
            match event {
                PackageCacheRequest::Get {
                    meta_far_blob,
                    gc_protection,
                    needed_blobs,
                    dir,
                    responder,
                } => {
                    let id = serve_id.fetch_add(1, Ordering::SeqCst);
                    let meta_far_blob: BlobInfo = meta_far_blob.into();
                    let node = get_node.create_child(id.to_string());
                    let trace_id = ftrace::Id::new();
                    let guard = ftrace::async_enter!(
                        trace_id,
                        c"app",
                        c"cache_get",
                        "meta_far_blob_id" => meta_far_blob.blob_id.to_string().as_str(),
                        "gc_protection" => format!("{gc_protection:?}").as_str(),
                        // An async duration cannot have multiple concurrent child async durations
                        // so we include the id as metadata to manually determine the
                        // relationship.
                        "trace_id" => u64::from(trace_id)
                    );
                    let response = get(
                        package_index.as_ref(),
                        base_packages.as_ref(),
                        cache_packages.as_ref(),
                        executability_restrictions,
                        &blobfs,
                        &root_dir_factory,
                        &open_packages,
                        meta_far_blob,
                        gc_protection,
                        needed_blobs,
                        dir,
                        scope.clone(),
                        cobalt_sender,
                        &node,
                    )
                    .await;
                    if let Some(o) = guard {
                        match &response {
                            Ok(()) => o.end(&[ftrace::ArgValue::of("status", "OK")]),
                            Err(s) => o.end(&[ftrace::ArgValue::of("status", s.to_string().as_str())]),
                        }
                    }
                    drop(node);
                    responder.send(response.map_err(|status| status.into_raw()))?;
                }
                PackageCacheRequest::GetSubpackage { superpackage, subpackage, dir, responder } => {
                    let fpkg::PackageUrl { url } = subpackage;
                    let () = responder.send(
                        get_subpackage(
                            base_packages.as_ref(),
                            executability_restrictions,
                            &open_packages,
                            BlobId::from(superpackage).into(),
                            url,
                            dir,
                            scope.clone(),
                        )
                        .await,
                    )?;
                }
                PackageCacheRequest::BasePackageIndex { iterator, control_handle: _ } => {
                    let stream = iterator.into_stream();
                    let () =
                        serve_package_index(base_packages.root_package_urls_and_hashes(), stream)
                            .await;
                }
                PackageCacheRequest::CachePackageIndex { iterator, control_handle: _ } => {
                    let stream = iterator.into_stream();
                    let () =
                        serve_package_index(cache_packages.root_package_urls_and_hashes(), stream)
                            .await;
                }
                PackageCacheRequest::Sync { responder } => {
                    responder.send(blobfs.sync().await.map_err(|e| {
                        error!("error syncing blobfs: {:#}", anyhow!(e));
                        Status::INTERNAL.into_raw()
                    }))?;
                }
                PackageCacheRequest::SetUpgradableUrls { pinned_urls, responder } => {
                    let res = if let Some(upgradable_packages) = &upgradable_packages {
                        upgradable_packages.set_upgradable_urls(pinned_urls, base_packages.as_ref())
                    } else {
                        error!("SetUpgradableUrls called but upgradable packages are not enabled in pkg-cache");
                        Err(fpkg::SetUpgradableUrlsError::Internal)
                    };
                    responder.send(res)?;
                }
                PackageCacheRequest::WriteBlobs { needed_blobs, control_handle: _  } => {
                    let id = serve_id.fetch_add(1, Ordering::SeqCst);
                    let node = get_node.create_child(id.to_string());
                    if let Err(e) = write_blobs(needed_blobs.into_stream(), &blobfs, &node).await {
                        error!("error while writing blobs: {:#}", anyhow!(e));
                    }
                }
                PackageCacheRequest::_UnknownMethod { ordinal, .. } => {
                    warn!("unknown method called, ordinal: {ordinal}")
                }
            }

            Ok(())
        })
        .await
}

#[derive(Debug)]
enum PackageAvailability {
    Always,
    Open(Arc<crate::RootDir>),
    Unknown,
}

impl PackageAvailability {
    fn get(
        base_packages: &BaseIndex,
        cache_packages: &CacheIndex,
        open_packages: &crate::RootDirCache,
        package: &fuchsia_hash::Hash,
    ) -> Self {
        if base_packages.is_package(*package) {
            return Self::Always;
        }
        if cache_packages.is_package(*package) {
            return Self::Always;
        }
        match open_packages.get(package) {
            Some(root_dir) => Self::Open(root_dir),
            None => Self::Unknown,
        }
    }
}

enum ExecutabilityStatus {
    Allowed,
    Forbidden,
}

fn executability_status(
    executability_restrictions: system_image::ExecutabilityRestrictions,
    base_packages: &BaseIndex,
    package: fuchsia_hash::Hash,
) -> ExecutabilityStatus {
    use ExecutabilityStatus::*;
    use system_image::ExecutabilityRestrictions::*;
    let is_base = base_packages.is_package(package);
    match (is_base, executability_restrictions) {
        (true, _) => Allowed,
        (false, Enforce) => Forbidden,
        (false, DoNotEnforce) => Allowed,
    }
}

impl From<ExecutabilityStatus> for fio::Flags {
    fn from(status: ExecutabilityStatus) -> Self {
        match status {
            ExecutabilityStatus::Allowed => fio::PERM_READABLE | fio::PERM_EXECUTABLE,
            ExecutabilityStatus::Forbidden => fio::PERM_READABLE,
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Fetch a package and optionally open it.
async fn get(
    package_index: &async_lock::RwLock<PackageIndex>,
    base_packages: &BaseIndex,
    cache_packages: &CacheIndex,
    executability_restrictions: system_image::ExecutabilityRestrictions,
    blobfs: &blobfs::Client,
    root_dir_factory: &crate::RootDirFactory,
    open_packages: &crate::RootDirCache,
    meta_far_blob: BlobInfo,
    gc_protection: fpkg::GcProtection,
    needed_blobs: ServerEnd<NeededBlobsMarker>,
    dir: ServerEnd<fio::DirectoryMarker>,
    scope: package_directory::ExecutionScope,
    cobalt_sender: ProtocolSender<MetricEvent>,
    node: &finspect::Node,
) -> Result<(), Status> {
    let guard =
        package_index.write().await.start_writing(meta_far_blob.blob_id.into(), gc_protection);
    let get_ret = get_impl(
        package_index,
        base_packages,
        cache_packages,
        executability_restrictions,
        blobfs,
        root_dir_factory,
        open_packages,
        meta_far_blob,
        gc_protection,
        needed_blobs,
        dir,
        scope,
        cobalt_sender,
        node,
    )
    .await;
    let stop_ret = package_index.write().await.stop_writing(guard);
    match (get_ret, stop_ret) {
        (get_ret, Ok(())) => get_ret,
        (Ok(()), Err(e)) => {
            error!("stopping the write of {meta_far_blob:?}: {:#}", anyhow!(e));
            Err(Status::INTERNAL)
        }
        (Err(get_err), Err(stop_err)) => {
            error!(
                "erroring stopping the write of {meta_far_blob:?}: {:#}, \
                the get failed first so that error will be returned",
                anyhow!(stop_err)
            );
            Err(get_err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Fetch a package and optionally open it.
async fn get_impl(
    package_index: &async_lock::RwLock<PackageIndex>,
    base_packages: &BaseIndex,
    cache_packages: &CacheIndex,
    executability_restrictions: system_image::ExecutabilityRestrictions,
    blobfs: &blobfs::Client,
    root_dir_factory: &crate::RootDirFactory,
    open_packages: &crate::RootDirCache,
    meta_far_blob: BlobInfo,
    gc_protection: fpkg::GcProtection,
    needed_blobs: ServerEnd<NeededBlobsMarker>,
    dir: ServerEnd<fio::DirectoryMarker>,
    scope: package_directory::ExecutionScope,
    mut cobalt_sender: ProtocolSender<MetricEvent>,
    node: &finspect::Node,
) -> Result<(), Status> {
    let () = node.record_int("started-time", zx::MonotonicInstant::get().into_nanos());
    let () = node.record_string("meta-far-id", meta_far_blob.blob_id.to_string());
    let () = node.record_uint("meta-far-length", meta_far_blob.length);
    let () = node.record_string("gc-protection", format!("{gc_protection:?}"));

    let needed_blobs = needed_blobs.into_stream();
    let pkg: Hash = meta_far_blob.blob_id.into();

    let root_dir = match gc_protection {
        // During OTA (which is the only client of Retained protection) do not short-circuit
        // fetches of packages expected to be resident, so that the system can recover from
        // unexpectedly absent blobs.
        fpkg::GcProtection::Retained => {
            let root_dir = serve_needed_blobs(
                needed_blobs,
                meta_far_blob,
                gc_protection,
                package_index,
                blobfs,
                root_dir_factory,
                node,
            )
            .await
            .map_err(|e| {
                error!("error while caching package {}: {:#}", pkg, anyhow!(e));
                cobalt_sender.open_io_error();
                Status::UNAVAILABLE
            })?;
            Arc::new(root_dir)
        }
        fpkg::GcProtection::OpenPackageTracking => {
            match PackageAvailability::get(base_packages, cache_packages, open_packages, &pkg) {
                PackageAvailability::Unknown => {
                    let root_dir = serve_needed_blobs(
                        needed_blobs,
                        meta_far_blob,
                        gc_protection,
                        package_index,
                        blobfs,
                        root_dir_factory,
                        node,
                    )
                    .await
                    .map_err(|e| {
                        error!("error while caching package {}: {:#}", pkg, anyhow!(e));
                        cobalt_sender.open_io_error();
                        Status::UNAVAILABLE
                    })?;
                    open_packages.get_or_insert(pkg, Some(root_dir)).await.map_err(|e| {
                        error!("get: open_packages.get_or_insert {}: {:#}", pkg, anyhow!(e));
                        cobalt_sender.open_io_error();
                        Status::INTERNAL
                    })?
                }
                PackageAvailability::Open(root_dir) => {
                    let () = needed_blobs.control_handle().shutdown_with_epitaph(Ok(()));
                    root_dir
                }
                PackageAvailability::Always => {
                    let () = needed_blobs.control_handle().shutdown_with_epitaph(Ok(()));
                    open_packages.get_or_insert(pkg, None).await.map_err(|e| {
                        error!("get: open_packages.get_or_insert {}: {:#}", pkg, anyhow!(e));
                        cobalt_sender.open_io_error();
                        Status::INTERNAL
                    })?
                }
            }
        }
    };

    let flags = executability_status(executability_restrictions, base_packages, pkg).into();
    vfs::directory::serve_on(root_dir, flags, scope, dir);

    cobalt_sender.open_success();
    Ok(())
}

async fn write_blobs(
    mut stream: NeededBlobsRequestStream,
    blobfs: &blobfs::Client,
    node: &finspect::Node,
) -> Result<(), ServeNeededBlobsError> {
    let mut open_blobs = HashSet::new();
    let open_counter = node.create_uint("open", 0);
    let written_counter = node.create_uint("written", 0);

    while let Some(request) =
        stream.try_next().await.map_err(ServeNeededBlobsError::ReceiveRequest)?
    {
        match request {
            NeededBlobsRequest::OpenBlob { blob_id, allow_existing, responder } => {
                let blob_id = Hash::from(BlobId::from(blob_id));
                match open_blob(responder, blobfs, blob_id, allow_existing).await {
                    Ok(OpenBlobSuccess::AlreadyCached) => {
                        // A prior call to OpenBlob may have added the blob to the set.
                        open_blobs.remove(&blob_id);
                        open_counter.set(open_blobs.len() as u64);
                    }
                    Ok(OpenBlobSuccess::Needed) => {
                        open_blobs.insert(blob_id);
                        open_counter.set(open_blobs.len() as u64);
                    }
                    Err(e) => {
                        warn!("Error while opening individual blob: {} {:#}", blob_id, anyhow!(e))
                    }
                }
            }
            NeededBlobsRequest::BlobWritten { blob_id, responder } => {
                let blob_id = Hash::from(BlobId::from(blob_id));
                if !open_blobs.remove(&blob_id) {
                    let _: Result<(), _> =
                        responder.send(Err(fpkg::BlobWrittenError::UnopenedBlob));
                    return Err(ServeNeededBlobsError::BlobWrittenBeforeOpened(blob_id.into()));
                }
                open_counter.set(open_blobs.len() as u64);
                if !blobfs.blob_present_and_up_to_date(&blob_id).await {
                    let _: Result<(), _> = responder.send(Err(fpkg::BlobWrittenError::NotWritten));
                    return Err(ServeNeededBlobsError::BlobWrittenButMissing(blob_id.into()));
                }
                written_counter.add(1);
                responder.send(Ok(())).map_err(ServeNeededBlobsError::SendResponse)?;
            }
            other => {
                return Err(ServeNeededBlobsError::UnexpectedRequest {
                    received: other.method_name(),
                    expected: if open_blobs.is_empty() {
                        "open_blob"
                    } else {
                        "open_blob or blob_written"
                    },
                });
            }
        }
    }
    Ok(())
}

#[derive(thiserror::Error, Debug)]
enum ServeNeededBlobsError {
    #[error("protocol violation: request stream terminated unexpectedly in {0}")]
    UnexpectedClose(&'static str),

    #[error("protocol violation: expected {expected} request, got {received}")]
    UnexpectedRequest { received: &'static str, expected: &'static str },

    #[error("protocol violation: while reading next request")]
    ReceiveRequest(#[source] fidl::Error),

    #[error("protocol violation: while responding to last request")]
    SendResponse(#[source] fidl::Error),

    #[error("the blob {0} is not needed")]
    BlobNotNeeded(Hash),

    #[error("the operation was aborted by the caller")]
    Aborted,

    #[error("while adding needed content blobs to the iterator")]
    SendNeededContentBlobs(#[source] futures::channel::mpsc::SendError),

    #[error("while adding needed subpackage meta.fars to the iterator")]
    SendNeededSubpackageBlobs(#[source] futures::channel::mpsc::SendError),

    #[error("while creating a RootDir for a subpackage")]
    CreateSubpackageRootDir(#[source] package_directory::Error),

    #[error("while reading the subpackages of a package")]
    ReadSubpackages(#[source] package_directory::SubpackagesError),

    #[error(
        "handle_open_blobs finished writing all the needed blobs but still had {count} \
             outstanding blob write futures. This should be impossible"
    )]
    OutstandingBlobWritesWhenHandleOpenBlobsFinished { count: usize },

    #[error("while recording some of a package's subpackage blobs")]
    RecordingSubpackageBlobs(#[source] anyhow::Error),

    #[error("while recording some of a package's content blobs")]
    RecordingContentBlobs(#[source] anyhow::Error),

    #[error("client signaled blob {0} was written before client opened said blob")]
    BlobWrittenBeforeOpened(BlobId),

    #[error("client signaled blob {0} was written but blobfs does not have it")]
    BlobWrittenButMissing(BlobId),

    #[error("client signaled blob {wrong_blob} was written but meta.far was {meta_far}")]
    WrongMetaFarBlobWritten { wrong_blob: BlobId, meta_far: BlobId },

    #[error("while creating a RootDir for the package")]
    CreatePackageRootDir(#[source] package_directory::Error),

    #[error("blobfs client was not configured to be writable")]
    CannotWriteBlobs,
}

/// Adds all of a package's content and subpackage blobs (discovered during the caching process by
/// MissingBlobs) to the PackageIndex, protecting them from GC. MissingBlobs waits for the Future
/// returned by `record` to complete (so waits until the blobs have been added to the PackageIndex)
/// before sending the blobs out over the paired receiver, which occurs before the blobs are sent
/// out over the missing blobs iterator, which occurs before the blobs are written via
/// NeededBlobs.OpenBlob, so the blobs of a package being cached should always be protected from
/// GC.
#[derive(Debug)]
struct IndexBlobRecorder<'a> {
    package_index: &'a async_lock::RwLock<PackageIndex>,
    meta_far: Hash,
    gc_protection: fpkg::GcProtection,
}

impl missing_blobs::BlobRecorder for IndexBlobRecorder<'_> {
    fn record(
        &self,
        blobs: HashSet<Hash>,
    ) -> futures::future::BoxFuture<'_, Result<(), anyhow::Error>> {
        async move {
            Ok(self.package_index.write().await.add_blobs(
                self.meta_far,
                blobs,
                self.gc_protection,
            )?)
        }
        .boxed()
    }
}

/// Implements the fuchsia.pkg.NeededBlobs protocol, which represents the transaction for caching a
/// particular package.
///
/// Clients should start by requesting to `OpenMetaBlob()`, and fetch and write the metadata blob
/// if needed. Once written, `GetMissingBlobs()` should be used to determine which content blobs
/// need fetched and written using `OpenBlob()`. Violating the expected protocol state will result
/// in the channel being closed by the package cache with a `ZX_ERR_BAD_STATE` epitaph and aborting
/// the package cache operation.
///
/// Once all needed blobs are written by the client, the package cache will complete the pending
/// [`PackageCache.Get`] request and close this channel with a `ZX_OK` epitaph.
async fn serve_needed_blobs(
    mut stream: NeededBlobsRequestStream,
    meta_far_info: BlobInfo,
    gc_protection: fpkg::GcProtection,
    package_index: &async_lock::RwLock<PackageIndex>,
    blobfs: &blobfs::Client,
    root_dir_factory: &crate::RootDirFactory,
    node: &finspect::Node,
) -> Result<crate::RootDir, ServeNeededBlobsError> {
    let state = node.create_string("state", "need-meta-far");
    let res = async {
        // Step 1: Open and write the meta.far, or determine it is not needed.
        let root_dir =
            handle_open_meta_blob(&mut stream, meta_far_info, blobfs, root_dir_factory, &state)
                .await?;

        let (missing_blobs, missing_blobs_recv) = missing_blobs::MissingBlobs::new(
            blobfs.clone(),
            root_dir_factory.clone(),
            &root_dir,
            Box::new(IndexBlobRecorder {
                package_index,
                meta_far: meta_far_info.blob_id.into(),
                gc_protection,
            }),
        )
        .await?;

        // Step 2: Determine which data blobs are needed and report them to the client.
        let serve_iterator = handle_get_missing_blobs(&mut stream, missing_blobs_recv).await?;

        state.set("need-content-blobs");

        // Step 3: Open and write all needed data blobs.
        let () = handle_open_blobs(&mut stream, missing_blobs, blobfs, node).await?;

        let () = serve_iterator.await;
        Ok(root_dir)
    }
    .await;

    // TODO in the Err(_) case, a responder was likely dropped, which would have already shutdown
    // the stream without our custom epitaph value.  Need to find a nice way to always shutdown
    // with a custom epitaph without copy/pasting something to every return site.

    let epitaph = match res {
        Ok(_) => Ok(()),
        Err(_) => Err(Status::BAD_STATE),
    };
    stream.control_handle().shutdown_with_epitaph(epitaph);

    res
}

async fn handle_open_meta_blob(
    stream: &mut NeededBlobsRequestStream,
    meta_far_info: BlobInfo,
    blobfs: &blobfs::Client,
    root_dir_factory: &crate::RootDirFactory,
    state: &finspect::StringProperty,
) -> Result<crate::RootDir, ServeNeededBlobsError> {
    let hash = meta_far_info.blob_id.into();
    let mut opened = false;

    loop {
        let () = match stream.try_next().await.map_err(ServeNeededBlobsError::ReceiveRequest)? {
            Some(NeededBlobsRequest::OpenMetaBlob { responder }) => {
                // Do not fail if already opened to allow retries.
                opened = true;
                match open_blob(responder, blobfs, hash, false).await? {
                    OpenBlobSuccess::AlreadyCached => break,
                    OpenBlobSuccess::Needed => Ok(()),
                }
            }
            Some(NeededBlobsRequest::BlobWritten { blob_id, responder }) => {
                let blob_id = BlobId::from(blob_id);
                if blob_id != meta_far_info.blob_id {
                    let _: Result<(), _> =
                        responder.send(Err(fpkg::BlobWrittenError::UnopenedBlob));
                    return Err(ServeNeededBlobsError::WrongMetaFarBlobWritten {
                        wrong_blob: blob_id,
                        meta_far: meta_far_info.blob_id,
                    });
                }
                if !opened {
                    let _: Result<(), _> =
                        responder.send(Err(fpkg::BlobWrittenError::UnopenedBlob));
                    return Err(ServeNeededBlobsError::BlobWrittenBeforeOpened(
                        meta_far_info.blob_id,
                    ));
                }
                if !blobfs.blob_present_and_up_to_date(&blob_id.into()).await {
                    let _: Result<(), _> = responder.send(Err(fpkg::BlobWrittenError::NotWritten));
                    return Err(ServeNeededBlobsError::BlobWrittenButMissing(
                        meta_far_info.blob_id,
                    ));
                }
                responder.send(Ok(())).map_err(ServeNeededBlobsError::SendResponse)?;
                break;
            }
            Some(NeededBlobsRequest::Abort { responder: _ }) => Err(ServeNeededBlobsError::Aborted),
            Some(other) => Err(ServeNeededBlobsError::UnexpectedRequest {
                received: other.method_name(),
                expected: if opened { "blob_written" } else { "open_meta_blob" },
            }),
            None => Err(ServeNeededBlobsError::UnexpectedClose("handle_open_meta_blob")),
        }?;
    }

    state.set("enumerate-missing-blobs");

    root_dir_factory.create(hash).await.map_err(ServeNeededBlobsError::CreatePackageRootDir)
}

async fn handle_get_missing_blobs(
    stream: &mut NeededBlobsRequestStream,
    missing_blobs: futures::channel::mpsc::UnboundedReceiver<Vec<fpkg::BlobInfo>>,
) -> Result<Task<()>, ServeNeededBlobsError> {
    let iterator = match stream.try_next().await.map_err(ServeNeededBlobsError::ReceiveRequest)? {
        Some(NeededBlobsRequest::GetMissingBlobs { iterator, control_handle: _ }) => Ok(iterator),
        Some(NeededBlobsRequest::Abort { responder: _ }) => Err(ServeNeededBlobsError::Aborted),
        Some(other) => Err(ServeNeededBlobsError::UnexpectedRequest {
            received: other.method_name(),
            expected: "get_missing_blobs",
        }),
        None => Err(ServeNeededBlobsError::UnexpectedClose("handle_get_missing_blobs")),
    }?;

    let iter_stream = iterator.into_stream();

    // Start serving the iterator in the background and internally move on to the next state. If
    // this foreground task decides to bail out, this spawned task will be dropped which will abort
    // the iterator serving task.
    Ok(Task::spawn(
        serve_fidl_iterator_from_stream(
            iter_stream,
            missing_blobs,
            // Unlikely that more than 10 Vec<BlobInfo> (e.g. 5 RootDirs with subpackages)
            // will be written to missing_blobs between calls to Iterator::Next by the FIDL client,
            // so no need to increase this which would use (a tiny amount) more memory.
            10,
        )
        .unwrap_or_else(|e| {
            error!("error serving BlobInfoIteratorRequestStream: {:#}", anyhow!(e))
        }),
    ))
}

async fn handle_open_blobs(
    stream: &mut NeededBlobsRequestStream,
    mut missing_blobs: missing_blobs::MissingBlobs<'_>,
    blobfs: &blobfs::Client,
    node: &finspect::Node,
) -> Result<(), ServeNeededBlobsError> {
    let known_remaining_counter =
        node.create_uint("known-remaining", missing_blobs.count_not_cached() as u64);
    let mut open_blobs = HashSet::new();
    let open_counter = node.create_uint("open", 0);
    let written_counter = node.create_uint("written", 0);

    while missing_blobs.count_not_cached() != 0 {
        match stream.try_next().await.map_err(ServeNeededBlobsError::ReceiveRequest)? {
            Some(NeededBlobsRequest::OpenBlob { blob_id, allow_existing, responder }) => {
                let blob_id = Hash::from(BlobId::from(blob_id));
                if !missing_blobs.should_cache(&blob_id) {
                    return Err(ServeNeededBlobsError::BlobNotNeeded(blob_id));
                }
                match open_blob(responder, blobfs, blob_id, allow_existing).await {
                    Ok(OpenBlobSuccess::AlreadyCached) => {
                        // A prior call to OpenBlob may have added the blob to the set.
                        open_blobs.remove(&blob_id);
                        open_counter.set(open_blobs.len() as u64);
                        let () = missing_blobs.cache(&blob_id).await?;
                        known_remaining_counter.set(missing_blobs.count_not_cached() as u64);
                    }
                    Ok(OpenBlobSuccess::Needed) => {
                        open_blobs.insert(blob_id);
                        open_counter.set(open_blobs.len() as u64);
                    }
                    Err(e) => {
                        warn!("Error while opening content blob: {} {:#}", blob_id, anyhow!(e))
                    }
                }
            }
            Some(NeededBlobsRequest::BlobWritten { blob_id, responder }) => {
                let blob_id = Hash::from(BlobId::from(blob_id));
                if !open_blobs.remove(&blob_id) {
                    let _: Result<(), _> =
                        responder.send(Err(fpkg::BlobWrittenError::UnopenedBlob));
                    return Err(ServeNeededBlobsError::BlobWrittenBeforeOpened(blob_id.into()));
                }
                open_counter.set(open_blobs.len() as u64);
                if !blobfs.blob_present_and_up_to_date(&blob_id).await {
                    let _: Result<(), _> = responder.send(Err(fpkg::BlobWrittenError::NotWritten));
                    return Err(ServeNeededBlobsError::BlobWrittenButMissing(blob_id.into()));
                }
                let () = missing_blobs.cache(&blob_id).await?;
                known_remaining_counter.set(missing_blobs.count_not_cached() as u64);
                written_counter.add(1);
                responder.send(Ok(())).map_err(ServeNeededBlobsError::SendResponse)?;
            }
            Some(NeededBlobsRequest::Abort { responder }) => {
                drop(responder);
                return Err(ServeNeededBlobsError::Aborted);
            }
            Some(other) => {
                return Err(ServeNeededBlobsError::UnexpectedRequest {
                    received: other.method_name(),
                    expected: if open_blobs.is_empty() {
                        "open_blob"
                    } else {
                        "open_blob or blob_written"
                    },
                });
            }
            None => {
                return Err(ServeNeededBlobsError::UnexpectedClose("handle_open_blobs"));
            }
        }
    }

    if !open_blobs.is_empty() {
        Err(ServeNeededBlobsError::OutstandingBlobWritesWhenHandleOpenBlobsFinished {
            count: open_blobs.len(),
        })
    } else {
        Ok(())
    }
}

// Allow a function to generically respond to either an OpenMetaBlob or OpenBlob request.
type OpenBlobResponse =
    Result<Option<fidl::endpoints::ClientEnd<ffxfs::BlobWriterMarker>>, fpkg::OpenBlobError>;
trait OpenBlobResponder {
    fn send(self, res: OpenBlobResponse) -> Result<(), fidl::Error>;
}
impl OpenBlobResponder for fpkg::NeededBlobsOpenBlobResponder {
    fn send(self, res: OpenBlobResponse) -> Result<(), fidl::Error> {
        self.send(res)
    }
}
impl OpenBlobResponder for fpkg::NeededBlobsOpenMetaBlobResponder {
    fn send(self, res: OpenBlobResponse) -> Result<(), fidl::Error> {
        self.send(res)
    }
}

#[derive(Debug)]
enum OpenBlobSuccess {
    AlreadyCached,
    Needed,
}

async fn open_blob(
    responder: impl OpenBlobResponder,
    blobfs: &blobfs::Client,
    blob_id: Hash,
    allow_existing: bool,
) -> Result<OpenBlobSuccess, ServeNeededBlobsError> {
    use OpenBlobSuccess::*;
    use blobfs::BlobStatus;
    use blobfs::CreateError::*;
    use fpkg::OpenBlobError as fErr;

    // If the caller already requested an overwrite, don't ask Blobfs.
    let allow_existing = allow_existing
        || match blobfs.blob_status(&blob_id).await {
            Ok(BlobStatus::UpToDate) => {
                // It is found and readable. Return early.
                let () = responder.send(Ok(None)).map_err(ServeNeededBlobsError::SendResponse)?;
                return Ok(AlreadyCached);
            }
            Ok(BlobStatus::NeedsOverwrite) => true,
            // Don't try to overwrite if it doesn't exist.
            Ok(BlobStatus::Absent) => false,
            Err(e) => {
                // If we can't properly check the overwrite status we should fail rather than make
                // assumptions. Missing an overwrite that is supposed to happen can leave the blob
                // storage in an unexpected state.
                warn!(blob_id:%; "Failed checking blob status {:#}", anyhow!(e));
                let () = responder
                    .send(Err(fErr::UnspecifiedIo))
                    .map_err(ServeNeededBlobsError::SendResponse)?;
                return Ok(Needed);
            }
        };
    let create_res = blobfs.open_blob_for_write(&blob_id, allow_existing).await;
    let is_readable = match &create_res {
        Err(blobfs::CreateError::AlreadyExists) => {
            // The blob may exist and be readable, or it may be in the process of being written.
            // Ensure we only indicate the blob is already present if we can actually open it for
            // read.
            blobfs.blob_present_and_up_to_date(&blob_id).await
        }
        _ => false,
    };

    let (fidl_resp, fn_ret) = match create_res {
        Ok(blob) => (Ok(Some(blob)), Ok(Needed)),
        Err(AlreadyExists) if is_readable => (Ok(None), Ok(AlreadyCached)),
        Err(AlreadyExists) => (Err(fErr::ConcurrentWrite), Ok(Needed)),
        Err(Io(e)) => {
            warn!(blob_id:%; "io error opening blob {:#}", anyhow!(e));
            (Err(fErr::UnspecifiedIo), Ok(Needed))
        }
        Err(ConvertToClientEnd) => {
            warn!(blob_id:%; "converting blob handle");
            (Err(fErr::UnspecifiedIo), Ok(Needed))
        }
        Err(Fidl(e)) => {
            warn!(blob_id:%; "fidl error opening blob {:#}", anyhow!(e));
            (Err(fErr::UnspecifiedIo), Ok(Needed))
        }
        Err(BlobCreator(error)) => {
            warn!(error:?, blob_id:%; "error calling blob creator");
            (Err(fErr::UnspecifiedIo), Ok(Needed))
        }
        Err(WritingNotConfigured) => {
            error!("blobfs client was not configured to be writable");
            (Err(fErr::Internal), Err(ServeNeededBlobsError::CannotWriteBlobs))
        }
    };
    let () = responder.send(fidl_resp).map_err(ServeNeededBlobsError::SendResponse)?;
    fn_ret
}

/// Serves the `PackageIndexIteratorRequestStream` with as many base package index entries per
/// request as will fit in a fidl message.
async fn serve_package_index(
    packages: impl IntoIterator<Item = (&UnpinnedAbsolutePackageUrl, &Hash)>,
    stream: PackageIndexIteratorRequestStream,
) {
    let mut package_entries = packages
        .into_iter()
        .map(|(url, hash)| PackageIndexEntry {
            package_url: fpkg::PackageUrl { url: url.to_string() },
            meta_far_blob_id: BlobId::from(*hash).into(),
        })
        .collect::<Vec<PackageIndexEntry>>();
    package_entries.sort_unstable_by(|a, b| a.package_url.url.cmp(&b.package_url.url));
    serve_fidl_iterator_from_slice(stream, package_entries).await.unwrap_or_else(|e| {
        error!("error serving PackageIndexIteratorRequestStream protocol: {:#}", anyhow!(e))
    })
}

async fn get_subpackage(
    base_packages: &BaseIndex,
    executability_restrictions: system_image::ExecutabilityRestrictions,
    open_packages: &crate::RootDirCache,
    superpackage: Hash,
    subpackage: String,
    dir: ServerEnd<fio::DirectoryMarker>,
    scope: package_directory::ExecutionScope,
) -> Result<(), fpkg::GetSubpackageError> {
    let subpackage = subpackage.parse::<fuchsia_url::RelativePackageUrl>().map_err(|e| {
        error!(
            superpackage:%,
            subpackage:%;
            "get_subpackage: invalid subpackage url: {:#}",
            anyhow!(e)
        );
        fpkg::GetSubpackageError::DoesNotExist
    })?;
    let Some(super_dir) = open_packages.get(&superpackage) else {
        error!(superpackage:%, subpackage:%; "get_subpackage: superpackage not open");
        return Err(fpkg::GetSubpackageError::SuperpackageClosed);
    };
    let subpackages = super_dir.subpackages().await.map_err(|e| {
        error!(
            superpackage:%,
            subpackage:%;
            "get_subpackage: determining subpackages: {:#}",
            anyhow!(e)
        );
        fpkg::GetSubpackageError::Internal
    })?;
    let Some(hash) = subpackages.subpackages().get(&subpackage) else {
        let superpackage_path = match super_dir.path().await {
            Ok(path) => Some(path.to_string()),
            Err(e) => {
                error!(superpackage:%; "failed to read package path: {:#}", anyhow!(e));
                None
            }
        };
        error!(
            "get_subpackage: '{subpackage}' is not a subpackage of {superpackage_path:?} \
             {superpackage}. Options are: '{}'",
            itertools::join(subpackages.subpackages().keys(), ", ")
        );
        return Err(fpkg::GetSubpackageError::DoesNotExist);
    };
    let root = open_packages.get_or_insert(*hash, None).await.map_err(|e| {
        error!(
            superpackage:%,
            subpackage:%;
            "get_subpackage: creating subpackage RootDir: {:#}",
            anyhow!(e)
        );
        fpkg::GetSubpackageError::Internal
    })?;
    let flags = executability_status(executability_restrictions, base_packages, *hash).into();
    vfs::directory::serve_on(root, flags, scope, dir);
    Ok(())
}

trait CobaltSenderExt {
    fn open_io_error(&mut self);
    fn open_success(&mut self);
}

impl CobaltSenderExt for ProtocolSender<MetricEvent> {
    fn open_io_error(&mut self) {
        self.send(
            MetricEvent::builder(metrics::PKG_CACHE_OPEN_MIGRATED_METRIC_ID)
                .with_event_codes(metrics::PkgCacheOpenMigratedMetricDimensionResult::Io)
                .as_occurrence(1),
        );
    }

    fn open_success(&mut self) {
        self.send(
            MetricEvent::builder(metrics::PKG_CACHE_OPEN_MIGRATED_METRIC_ID)
                .with_event_codes(metrics::PkgCacheOpenMigratedMetricDimensionResult::Success)
                .as_occurrence(1),
        );
    }
}

#[cfg(test)]
mod serve_needed_blobs_tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl_fuchsia_fxfs as ffxfs;
    use fidl_fuchsia_pkg::{BlobInfoIteratorMarker, BlobInfoIteratorProxy, NeededBlobsProxy};
    use fuchsia_hash::HashRangeFull;
    use fuchsia_inspect as finspect;
    use futures::stream::StreamExt as _;
    use futures::{future, stream};
    use test_case::test_case;

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn start_stop(gc_protection: fpkg::GcProtection) {
        let (_, stream) = fidl::endpoints::create_proxy_and_stream::<NeededBlobsMarker>();

        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };

        let (blobfs, _, _, _) = blobfs::Client::new_test();
        let inspector = finspect::Inspector::default();
        let package_index = Arc::new(async_lock::RwLock::new(PackageIndex::new()));

        assert_matches!(
            serve_needed_blobs(
                stream,
                meta_blob_info,
                gc_protection,
                &package_index,
                &blobfs,
                &crate::root_dir::new_test(blobfs.clone()).await.0,
                &inspector.root().create_child("test-node-name"),
            )
            .await,
            Err(ServeNeededBlobsError::UnexpectedClose("handle_open_meta_blob"))
        );
    }

    fn spawn_serve_needed_blobs_with_mocks(
        meta_blob_info: BlobInfo,
        gc_protection: fpkg::GcProtection,
    ) -> (Task<Result<(), ServeNeededBlobsError>>, NeededBlobsProxy, blobfs::Mock) {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<NeededBlobsMarker>();

        let (blobfs, blobfs_mock) = blobfs::Client::new_mock();
        let inspector = finspect::Inspector::default();
        let package_index = Arc::new(async_lock::RwLock::new(PackageIndex::new()));

        (
            Task::spawn(async move {
                let (root_dir_factory, _) = crate::root_dir::new_test(blobfs.clone()).await;

                let guard = package_index
                    .write()
                    .await
                    .start_writing(meta_blob_info.blob_id.into(), gc_protection);
                let res = serve_needed_blobs(
                    stream,
                    meta_blob_info,
                    gc_protection,
                    &package_index,
                    &blobfs,
                    &root_dir_factory,
                    &inspector.root().create_child("test-node-name"),
                )
                .await
                .map(|_| ());
                let () = package_index.write().await.stop_writing(guard).unwrap();
                res
            }),
            proxy,
            blobfs_mock,
        )
    }

    struct FakeOpenBlobResponse(Option<OpenBlobResponse>);

    struct FakeOpenBlobResponder<'a> {
        response: &'a mut FakeOpenBlobResponse,
    }

    impl FakeOpenBlobResponse {
        fn new() -> Self {
            Self(None)
        }
        fn responder(&mut self) -> FakeOpenBlobResponder<'_> {
            FakeOpenBlobResponder { response: self }
        }
        fn take(self) -> OpenBlobResponse {
            self.0.unwrap()
        }
    }

    impl OpenBlobResponder for FakeOpenBlobResponder<'_> {
        fn send(self, res: OpenBlobResponse) -> Result<(), fidl::Error> {
            self.response.0 = Some(res);
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn open_blob_handles_io_open_error() {
        // Provide open_write_blob a closed blobfs and file stream to trigger a PEER_CLOSED IO
        // error.
        let (blobfs, _, _, _) = blobfs::Client::new_test();

        let mut response = FakeOpenBlobResponse::new();
        let res = open_blob(response.responder(), &blobfs, [0; 32].into(), false).await;

        // The operation should succeed, to allow retries, but it should report the failure to the
        // fidl responder.
        assert_matches!(res, Ok(OpenBlobSuccess::Needed));
        assert_matches!(response.take(), Err(fpkg::OpenBlobError::UnspecifiedIo));
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn expects_open_meta_blob(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };

        let (task, proxy, blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let (iter, iter_server_end) = fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();
        proxy.get_missing_blobs(iter_server_end).unwrap();
        assert_matches!(
            iter.next().await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );

        assert_matches!(
            task.await,
            Err(ServeNeededBlobsError::UnexpectedRequest {
                received: "get_missing_blobs",
                expected: "open_meta_blob"
            })
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn expects_open_meta_blob_before_blob_written(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (task, proxy, blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        assert_matches!(
            proxy.blob_written(&BlobId::from([0; 32]).into()).await.unwrap(),
            Err(fpkg::BlobWrittenError::UnopenedBlob)
        );

        assert_matches!(
            task.await,
            Err(ServeNeededBlobsError::BlobWrittenBeforeOpened(hash)) if hash == [0; 32].into()
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn expects_open_meta_blob_once(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 4 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        // Open a needed meta FAR blob and write it.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs
                    .expect_create_blob([0; 32].into(), false)
                    .await
                    .expect_payload(b"test")
                    .await;
                blobfs.expect_readable_missing_checks(&[[0; 32].into()], &[]).await;

                // serve_needed_blobs parses the meta far after it is written.  Feed that logic a
                // valid, minimal far that doesn't actually correlate to what we just wrote.
                let () = serve_minimal_far(&mut blobfs, [0; 32].into()).await;
            },
            async {
                let blob = proxy
                    .open_meta_blob()
                    .await
                    .expect("open_meta_blob failed")
                    .expect("open_meta_blob error")
                    .expect("meta blob not cached")
                    .into_proxy();

                let vmo = blob
                    .get_vmo(4)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let () = vmo.write(b"test", 0).unwrap();
                let () = blob.bytes_ready(4).await.unwrap().unwrap();

                let () = proxy
                    .blob_written(&BlobId::from([0; 32]).into())
                    .await
                    .expect("blob_written failed")
                    .expect("blob_written error");
            },
        )
        .await;

        // Trying to open the meta FAR blob again after writing it successfully is a protocol
        // violation.
        assert_matches!(
            proxy.open_meta_blob().await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );

        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::UnexpectedRequest {
                received: "open_meta_blob",
                expected: "get_missing_blobs"
            })
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn meta_far_blob_written_wrong_hash(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 4 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs
                    .expect_create_blob([0; 32].into(), false)
                    .await
                    .expect_payload(b"test")
                    .await;
            },
            async {
                let blob = proxy
                    .open_meta_blob()
                    .await
                    .expect("open_meta_blob failed")
                    .expect("open_meta_blob error")
                    .expect("meta blob not cached")
                    .into_proxy();

                let vmo = blob
                    .get_vmo(4)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let () = vmo.write(b"test", 0).unwrap();
                let () = blob
                    .bytes_ready(4)
                    .await
                    .expect("bytes_ready failed")
                    .map_err(Status::err_from_raw)
                    .expect("bytes_ready error");

                assert_matches!(
                    proxy
                        .blob_written(&BlobId::from([1; 32]).into())
                        .await
                        .expect("blob_written failed"),
                    Err(fpkg::BlobWrittenError::UnopenedBlob)
                );
            },
        )
        .await;

        // Calling BlobWritten for the wrong hash should close the channel, so calling BlobWritten
        // for the correct hash now should fail.
        assert_matches!(
            proxy.blob_written(&BlobId::from([0; 32]).into()).await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. }) if epitaph == Status::BAD_STATE
        );

        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::WrongMetaFarBlobWritten {
                wrong_blob,
                meta_far
            }) if wrong_blob == [1; 32].into() && meta_far == [0; 32].into()
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn meta_far_blob_written_but_not_in_blobfs(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 4 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob([0; 32].into(), false).await.expect_done().await;
                blobfs.expect_readable_missing_checks(&[], &[[0; 32].into()]).await;
            },
            async {
                let _: fidl::endpoints::ClientEnd<ffxfs::BlobWriterMarker> = proxy
                    .open_meta_blob()
                    .await
                    .expect("open_meta_blob failed")
                    .expect("open_meta_blob error")
                    .expect("meta blob not cached");

                assert_matches!(
                    proxy
                        .blob_written(&BlobId::from([0; 32]).into())
                        .await
                        .expect("blob_written failed"),
                    Err(fpkg::BlobWrittenError::NotWritten)
                );
            },
        )
        .await;

        // The invalid BlobWritten call should close the channel, so trying to open the meta far
        // again should fail.
        assert_matches!(
            proxy.open_meta_blob().await,
            Err(fidl::Error::ClientChannelClosed { epitaph, .. }) if epitaph == Status::BAD_STATE
        );

        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::BlobWrittenButMissing(hash)) if hash == [0; 32].into()
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn handles_present_meta_blob(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        // Try to open the meta FAR blob, but report it is no longer needed.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Ok(false)).await;

                // serve_needed_blobs parses the meta far after it is written.  Feed that logic a
                // valid, minimal far that doesn't actually correlate to what we just wrote.
                let () = serve_minimal_far(&mut blobfs, [0; 32].into()).await;
            },
            async {
                assert_eq!(
                    proxy
                        .open_meta_blob()
                        .await
                        .expect("open_meta_blob failed")
                        .expect("open_meta_blob error"),
                    None
                );
            },
        )
        .await;

        // Trying to open the meta FAR blob again after being told it is not needed is a protocol
        // violation.
        assert_matches!(
            proxy.open_meta_blob().await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );

        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::UnexpectedRequest {
                received: "open_meta_blob",
                expected: "get_missing_blobs"
            })
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn allows_retrying_nonfatal_open_meta_blob_errors(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 1 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        // Try to open the meta FAR blob, but report it is already being written concurrently.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs.fail_create([0; 32].into(), ffxfs::CreateBlobError::AlreadyExists).await;
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
            },
            async {
                assert_matches!(
                    proxy.open_meta_blob().await,
                    Ok(Err(fidl_fuchsia_pkg::OpenBlobError::ConcurrentWrite))
                );
            },
        )
        .await;

        // Try to write the meta FAR blob, but report the written contents are corrupt.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs
                    .expect_create_blob([0; 32].into(), false)
                    .await
                    .expect_get_vmo(1)
                    .await
                    .fail_bytes_written()
                    .await;
            },
            async {
                let blob = proxy
                    .open_meta_blob()
                    .await
                    .expect("open_meta_blob failed")
                    .expect("open_meta_blob error")
                    .expect("blob already cached")
                    .into_proxy();

                let _: zx::Vmo = blob
                    .get_vmo(1)
                    .await
                    .expect("get_vmo fidl error")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let result = blob
                    .bytes_ready(1)
                    .await
                    .expect("bytes_ready failed")
                    .map_err(Status::err_from_raw);
                assert_eq!(result, Err(Status::IO_DATA_INTEGRITY));
            },
        )
        .await;

        // Open the meta FAR blob for write, but then close it (a non-fatal error)
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob([0; 32].into(), false).await.expect_done().await;
            },
            async {
                let blob: fidl::endpoints::ClientEnd<ffxfs::BlobWriterMarker> = proxy
                    .open_meta_blob()
                    .await
                    .expect("open_meta_blob failed")
                    .expect("open_meta_blob error")
                    .expect("blob already cached");
                drop(blob);
            },
        )
        .await;

        // Operation succeeds after blobfs cooperates.
        let ((), ()) = future::join(
            #[allow(clippy::async_yields_async)]
            async {
                blobfs.expect_needs_overwrite([0; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob([0; 32].into(), false).await.expect_payload(&[0]).await;
                blobfs.expect_readable_missing_checks(&[[0; 32].into()], &[]).await;

                // serve_needed_blobs parses the meta far after it is written.  Feed that logic a
                // valid, minimal far that doesn't actually correlate to what we just wrote.
                let () = serve_minimal_far(&mut blobfs, [0; 32].into()).await;
            },
            async {
                let blob = proxy.open_meta_blob().await.unwrap().unwrap().unwrap().into_proxy();

                let vmo = blob
                    .get_vmo(1)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let () = vmo.write(&[0], 0).unwrap();
                let () = blob.bytes_ready(1).await.unwrap().unwrap();
                let () = proxy
                    .blob_written(&BlobId::from([0; 32]).into())
                    .await
                    .expect("blob_written failed")
                    .expect("blob_written error");
            },
        )
        .await;

        // Task moves to next state after retried write operation succeeds.
        assert_matches!(
            proxy.open_meta_blob().await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::UnexpectedRequest {
                received: "open_meta_blob",
                expected: "get_missing_blobs"
            })
        );
        blobfs.expect_done().await;
    }

    /// The returned task completes when the connection to the meta blob closes.
    pub(super) async fn serve_minimal_far(blobfs: &mut blobfs::Mock, meta_hash: Hash) {
        let far_data = crate::test_utils::get_meta_far("fake-package", [], []);
        let () = blobfs.expect_open_blob(meta_hash, Ok(far_data.clone())).await;
    }

    pub(super) async fn write_meta_blob(
        proxy: &NeededBlobsProxy,
        blobfs: &mut blobfs::Mock,
        meta_blob_info: BlobInfo,
        needed_blobs: impl IntoIterator<Item = Hash>,
    ) {
        let far_data = crate::test_utils::get_meta_far("fake-package", needed_blobs, []);

        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite(meta_blob_info.blob_id.into(), Ok(false)).await;

                // This open is used to read the meta.far
                blobfs.expect_open_blob(meta_blob_info.blob_id.into(), Ok(far_data.clone())).await;
            },
            async {
                assert_matches!(proxy.open_meta_blob().await, Ok(Ok(None)));
            },
        )
        .await;
    }

    async fn collect_blob_info_iterator(proxy: BlobInfoIteratorProxy) -> Vec<BlobInfo> {
        let mut res = vec![];

        loop {
            let chunk = proxy.next().await.unwrap();

            if chunk.is_empty() {
                break;
            }

            res.extend(chunk.into_iter().map(BlobInfo::from));
        }

        res
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn discovers_and_reports_missing_blobs(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let expected = HashRangeFull::default().skip(1).take(2000).collect::<Vec<_>>();

        let () =
            write_meta_blob(&proxy, &mut blobfs, meta_blob_info, expected.iter().copied()).await;

        let ((), ()) = future::join(
            async {
                blobfs
                    .expect_filter_to_missing_blobs_with_readable_missing_ids(&[], &expected[..])
                    .await;
            },
            async {
                let (missing_blobs_iter, missing_blobs_iter_server_end) =
                    fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();

                assert_matches!(proxy.get_missing_blobs(missing_blobs_iter_server_end), Ok(()));

                let missing_blobs = collect_blob_info_iterator(missing_blobs_iter).await;

                let expected = expected
                    .iter()
                    .cloned()
                    .map(|hash| BlobInfo { blob_id: hash.into(), length: 0 })
                    .collect::<Vec<_>>();
                assert_eq!(missing_blobs, expected);
            },
        )
        .await;

        drop(proxy);
        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::UnexpectedClose("handle_open_blobs"))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn handles_no_missing_blobs(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![]).await;

        let (missing_blobs_iter, missing_blobs_iter_server_end) =
            fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();
        assert_matches!(proxy.get_missing_blobs(missing_blobs_iter_server_end), Ok(()));
        let missing_blobs = collect_blob_info_iterator(missing_blobs_iter).await;
        assert_eq!(missing_blobs, vec![]);

        assert_matches!(serve_needed_task.await, Ok(()));
        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Ok(())),
                ..
            }))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn fails_on_invalid_meta_far(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let bogus_far_data = b"this is not a far file";

        let ((), ()) = future::join(
            async {
                // Fail the create request, then succeed an open request that checks if the blob is
                // readable. The already_exists error could indicate that the blob is being
                // written, so pkg-cache need to disambiguate the 2 cases.
                blobfs
                    .expect_needs_overwrite(meta_blob_info.blob_id.into(), Err(Status::NOT_FOUND))
                    .await;
                blobfs
                    .fail_create(
                        meta_blob_info.blob_id.into(),
                        ffxfs::CreateBlobError::AlreadyExists,
                    )
                    .await;
                blobfs.expect_needs_overwrite(meta_blob_info.blob_id.into(), Ok(false)).await;

                blobfs
                    .expect_open_blob(meta_blob_info.blob_id.into(), Ok(bogus_far_data.to_vec()))
                    .await;
            },
            async {
                assert_matches!(proxy.open_meta_blob().await, Ok(Ok(None)));
            },
        )
        .await;

        drop(proxy);
        assert_matches!(
            task.await,
            Err(ServeNeededBlobsError::CreatePackageRootDir(
                package_directory::Error::ArchiveReader(fuchsia_archive::Error::InvalidMagic(_))
            ))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn dropping_needed_blobs_stops_missing_blob_iterator(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let missing = HashRangeFull::default().take(10).collect::<Vec<_>>();
        let () =
            write_meta_blob(&proxy, &mut blobfs, meta_blob_info, missing.iter().copied()).await;

        let ((), ()) = future::join(
            async {
                blobfs
                    .expect_filter_to_missing_blobs_with_readable_missing_ids(&[], &missing[..])
                    .await;
            },
            async {
                let (missing_blobs_iter, missing_blobs_iter_server_end) =
                    fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();

                assert_matches!(proxy.get_missing_blobs(missing_blobs_iter_server_end), Ok(()));

                // Closing the needed blobs request stream terminates any spawned tasks.
                drop(proxy);
                assert_matches!(
                    missing_blobs_iter.next().await,
                    Err(fidl::Error::ClientChannelClosed {
                        epitaph: fidl::Epitaph::PeerClosed,
                        ..
                    })
                );
            },
        )
        .await;

        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::UnexpectedClose("handle_open_blobs"))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn expects_get_missing_blobs_once(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let missing = HashRangeFull::default().take(10).collect::<Vec<_>>();
        let () =
            write_meta_blob(&proxy, &mut blobfs, meta_blob_info, missing.iter().copied()).await;

        // Enumerate the needs successfully once.
        let ((), ()) = future::join(
            async {
                blobfs
                    .expect_filter_to_missing_blobs_with_readable_missing_ids(&[], &missing[..])
                    .await;
            },
            async {
                let (missing_blobs_iter, missing_blobs_iter_server_end) =
                    fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();

                assert_matches!(proxy.get_missing_blobs(missing_blobs_iter_server_end), Ok(()));

                collect_blob_info_iterator(missing_blobs_iter).await;
            },
        )
        .await;

        // Trying to enumerate the missing blobs again is a protocol violation.
        let (_missing_blobs_iter, missing_blobs_iter_server_end) =
            fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();
        assert_matches!(proxy.get_missing_blobs(missing_blobs_iter_server_end), Ok(()));

        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::UnexpectedRequest {
                received: "get_missing_blobs",
                expected: "open_blob"
            })
        );
        blobfs.expect_done().await;
    }

    pub(super) async fn enumerate_readable_missing_blobs(
        proxy: &NeededBlobsProxy,
        blobfs: &mut blobfs::Mock,
        readable: impl Iterator<Item = Hash>,
        missing: impl Iterator<Item = Hash>,
    ) {
        let readable = readable.collect::<Vec<_>>();
        let missing = missing.collect::<Vec<_>>();

        let ((), ()) = future::join(
            async {
                blobfs
                    .expect_filter_to_missing_blobs_with_readable_missing_ids(
                        &readable[..],
                        &missing[..],
                    )
                    .await;
            },
            async {
                let (missing_blobs_iter, missing_blobs_iter_server_end) =
                    fidl::endpoints::create_proxy::<BlobInfoIteratorMarker>();

                assert_matches!(proxy.get_missing_blobs(missing_blobs_iter_server_end), Ok(()));

                let infos = collect_blob_info_iterator(missing_blobs_iter).await;
                let mut actual =
                    infos.into_iter().map(|info| info.blob_id.into()).collect::<Vec<Hash>>();
                actual.sort_unstable();
                assert_eq!(missing, actual);
            },
        )
        .await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn single_need(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [1; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        let payload = b"single blob";

        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([2; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs
                    .expect_create_blob([2; 32].into(), false)
                    .await
                    .expect_payload(payload)
                    .await;
                blobfs.expect_readable_missing_checks(&[[2; 32].into()], &[]).await;
            },
            async {
                let blob = proxy
                    .open_blob(&BlobId::from([2; 32]).into(), false)
                    .await
                    .expect("open_blob failed")
                    .expect("open_blob error")
                    .expect("blob not cached")
                    .into_proxy();

                let vmo = blob
                    .get_vmo(payload.len().try_into().unwrap())
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let () = vmo.write(payload, 0).unwrap();
                let () = blob
                    .bytes_ready(payload.len().try_into().unwrap())
                    .await
                    .unwrap()
                    .map_err(Status::err_from_raw)
                    .expect("bytes_ready error");

                drop(blob);

                let () = proxy
                    .blob_written(&BlobId::from([2; 32]).into())
                    .await
                    .expect("blob_written failed")
                    .expect("blob_written error");
            },
        )
        .await;

        assert_matches!(serve_needed_task.await, Ok(()));
        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Ok(())),
                ..
            }))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn open_blob_blob_present_on_second_call(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [1; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([2; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob([2; 32].into(), false).await.expect_done().await;
                blobfs.expect_needs_overwrite([2; 32].into(), Ok(false)).await;
            },
            async {
                let _: fidl::endpoints::ClientEnd<ffxfs::BlobWriterMarker> = proxy
                    .open_blob(&BlobId::from([2; 32]).into(), false)
                    .await
                    .expect("open_blob failed")
                    .expect("open_blob error")
                    .expect("blob not cached");

                assert_eq!(
                    proxy
                        .open_blob(&BlobId::from([2; 32]).into(), false)
                        .await
                        .expect("open_blob failed")
                        .expect("open_blob error"),
                    None
                );
            },
        )
        .await;

        assert_matches!(serve_needed_task.await, Ok(()));
        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Ok(())),
                ..
            }))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn handles_many_content_blobs_that_need_written(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let content_blobs = || HashRangeFull::default().skip(1).take(100);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, content_blobs()).await;
        let () = enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            content_blobs(),
        )
        .await;

        fn payload(hash: Hash) -> Vec<u8> {
            let hash_bytes = || hash.as_bytes().iter().copied();
            let len = hash_bytes().map(|n| n as usize).sum();
            assert!(len <= fio::MAX_BUF as usize);

            std::iter::repeat(hash_bytes()).flatten().take(len).collect()
        }

        let ((), ()) = future::join(
            async {
                for hash in content_blobs() {
                    blobfs.expect_needs_overwrite(hash, Err(Status::NOT_FOUND)).await;
                    blobfs
                        .expect_create_blob(hash, false)
                        .await
                        .expect_payload(&payload(hash))
                        .await;
                }
                blobfs
                    .expect_readable_missing_checks(
                        content_blobs().collect::<Vec<_>>().as_slice(),
                        &[],
                    )
                    .await;
            },
            async {
                let () = stream::iter(content_blobs())
                    .for_each_concurrent(None, |hash| {
                        let open_fut = proxy.open_blob(&BlobId::from(hash).into(), false);
                        let proxy = &proxy;

                        async move {
                            let blob = open_fut.await.unwrap().unwrap().unwrap().into_proxy();

                            let payload = payload(hash);
                            let vmo = blob
                                .get_vmo(payload.len().try_into().unwrap())
                                .await
                                .expect("get_vmo failed")
                                .map_err(Status::err_from_raw)
                                .expect("get_vmo error");
                            let () = vmo.write(&payload, 0).unwrap();
                            let () = blob
                                .bytes_ready(payload.len().try_into().unwrap())
                                .await
                                .unwrap()
                                .unwrap();

                            let () = proxy
                                .blob_written(&BlobId::from(hash).into())
                                .await
                                .expect("blob_written failed")
                                .expect("blob_written error");
                        }
                    })
                    .await;
            },
        )
        .await;

        assert_matches!(serve_needed_task.await, Ok(()));
        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Ok(())),
                ..
            }))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn handles_many_content_blobs_that_are_already_present(
        gc_protection: fpkg::GcProtection,
    ) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let content_blobs = || HashRangeFull::default().skip(1).take(100);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, content_blobs()).await;
        enumerate_readable_missing_blobs(&proxy, &mut blobfs, std::iter::empty(), content_blobs())
            .await;

        let ((), ()) = future::join(
            async {
                for hash in content_blobs() {
                    blobfs.expect_needs_overwrite(hash, Ok(false)).await;
                }
            },
            async {
                let () = stream::iter(content_blobs())
                    .for_each(|hash| {
                        let open_fut = proxy.open_blob(&BlobId::from(hash).into(), false);

                        async move {
                            assert_eq!(open_fut.await.unwrap().unwrap(), None);
                        }
                    })
                    .await;
            },
        )
        .await;

        assert_matches!(serve_needed_task.await, Ok(()));
        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Ok(())),
                ..
            }))
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn content_blob_caller_requests_overwrite(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [1; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        future::join(
            async {
                // No NeedsOverwrite call, since the caller requested it.
                blobfs.expect_create_blob([2; 32].into(), true).await.expect_payload(&[0]).await;
                blobfs.expect_readable_missing_checks(&[[2; 32].into()], &[]).await;
            },
            async {
                let blob = proxy
                    .open_blob(&BlobId::from([2; 32]).into(), true)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_proxy();

                let vmo = blob
                    .get_vmo(1)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                vmo.write(&[0], 0).unwrap();
                blob.bytes_ready(1).await.unwrap().unwrap();
                drop(blob);

                proxy
                    .blob_written(&BlobId::from([2; 32]).into())
                    .await
                    .expect("blob_written failed")
                    .expect("blob_written error");
            },
        )
        .await;

        assert_matches!(serve_needed_task.await, Ok(()));
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn content_blob_blobfs_requests_overwrite(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [1; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        // Error the first time.
        future::join(
            async {
                blobfs.expect_needs_overwrite([2; 32].into(), Err(Status::PEER_CLOSED)).await;
            },
            async {
                assert_matches!(
                    proxy
                        .open_blob(&BlobId::from([2; 32]).into(), false)
                        .await
                        .expect("open_blob failed"),
                    Err(fpkg::OpenBlobError::UnspecifiedIo),
                    "Bad open_blob response",
                );
            },
        )
        .await;

        // Succeed on the second try. Requests overwrite.
        future::join(
            async {
                blobfs.expect_needs_overwrite([2; 32].into(), Ok(true)).await;
                blobfs.expect_create_blob([2; 32].into(), true).await.expect_payload(&[0]).await;
                blobfs.expect_readable_missing_checks(&[[2; 32].into()], &[]).await;
            },
            async {
                let blob = proxy
                    .open_blob(&BlobId::from([2; 32]).into(), false)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_proxy();

                let vmo = blob
                    .get_vmo(1)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                vmo.write(&[0], 0).unwrap();
                blob.bytes_ready(1).await.unwrap().unwrap();
                drop(blob);

                proxy
                    .blob_written(&BlobId::from([2; 32]).into())
                    .await
                    .expect("blob_written failed")
                    .expect("blob_written error");
            },
        )
        .await;

        assert_matches!(serve_needed_task.await, Ok(()));
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn content_blob_written_but_not_in_blobfs(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [1; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        let () = enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite([2; 32].into(), Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob([2; 32].into(), false).await.expect_done().await;
                blobfs.expect_readable_missing_checks(&[], &[[2; 32].into()]).await;
            },
            async {
                let _: fidl::endpoints::ClientEnd<ffxfs::BlobWriterMarker> = proxy
                    .open_blob(&BlobId::from([2; 32]).into(), false)
                    .await
                    .expect("open_blob failed")
                    .expect("open_blob error")
                    .expect("blob not cached");

                assert_matches!(
                    proxy
                        .blob_written(&BlobId::from([2; 32]).into())
                        .await
                        .expect("blob_written failed"),
                    Err(fpkg::BlobWrittenError::NotWritten)
                );
            },
        )
        .await;

        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. }))
                if epitaph == Status::BAD_STATE
        );
        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::BlobWrittenButMissing(hash)) if hash == [2; 32].into()
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn content_blob_written_before_open_blob(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        assert_matches!(
            proxy.blob_written(&BlobId::from([2; 32]).into()).await.expect("blob_written failed"),
            Err(fpkg::BlobWrittenError::UnopenedBlob)
        );

        assert_matches!(
            proxy.take_event_stream().next().await,
            Some(Err(fidl::Error::ClientChannelClosed { epitaph, .. }))
                if epitaph == Status::BAD_STATE
        );
        assert_matches!(
            serve_needed_task.await,
            Err(ServeNeededBlobsError::BlobWrittenBeforeOpened(hash)) if hash == [2; 32].into()
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn allows_retrying_nonfatal_open_blob_errors(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let content_blob = Hash::from([1; 32]);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![content_blob]).await;
        let () = enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![content_blob].into_iter(),
        )
        .await;

        // Try to open the blob, but report it is already being written concurrently.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite(content_blob, Err(Status::NOT_FOUND)).await;
                blobfs.fail_create(content_blob, ffxfs::CreateBlobError::AlreadyExists).await;
                blobfs.expect_needs_overwrite(content_blob, Err(Status::NOT_FOUND)).await;
            },
            async {
                assert_matches!(
                    proxy.open_blob(&BlobId::from(content_blob).into(), false).await,
                    Ok(Err(fpkg::OpenBlobError::ConcurrentWrite))
                );
            },
        )
        .await;

        // Try to write the blob, but report the written contents are corrupt.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite(content_blob, Err(Status::NOT_FOUND)).await;
                blobfs
                    .expect_create_blob(content_blob, false)
                    .await
                    .expect_get_vmo(1)
                    .await
                    .fail_bytes_written()
                    .await;
            },
            async {
                let blob = proxy
                    .open_blob(&BlobId::from(content_blob).into(), false)
                    .await
                    .expect("open_blob failed")
                    .expect("open_blob error")
                    .expect("blob not cached")
                    .into_proxy();

                let _: zx::Vmo = blob
                    .get_vmo(1)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let result =
                    blob.bytes_ready(1).await.expect("write failed").map_err(Status::err_from_raw);
                assert_eq!(result, Err(Status::IO_DATA_INTEGRITY));
            },
        )
        .await;

        // Open the blob for write, but then close it (a non-fatal error)
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite(content_blob, Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob(content_blob, false).await.expect_done().await;
            },
            async {
                let blob: fidl::endpoints::ClientEnd<ffxfs::BlobWriterMarker> = proxy
                    .open_blob(&BlobId::from(content_blob).into(), false)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                drop(blob);
            },
        )
        .await;

        // Operation succeeds after blobfs cooperates.
        let ((), ()) = future::join(
            async {
                blobfs.expect_needs_overwrite(content_blob, Err(Status::NOT_FOUND)).await;
                blobfs.expect_create_blob(content_blob, false).await.expect_payload(&[0]).await;
                blobfs.expect_readable_missing_checks(&[content_blob], &[]).await;
            },
            async {
                let blob = proxy
                    .open_blob(&BlobId::from(content_blob).into(), false)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_proxy();

                let vmo = blob
                    .get_vmo(1)
                    .await
                    .expect("get_vmo failed")
                    .map_err(Status::err_from_raw)
                    .expect("get_vmo error");
                let () = vmo.write(&[0], 0).unwrap();
                let () = blob.bytes_ready(1).await.unwrap().unwrap();
                drop(blob);

                let () = proxy
                    .blob_written(&BlobId::from(content_blob).into())
                    .await
                    .expect("blob_written failed")
                    .expect("blob_written error");
            },
        )
        .await;

        // That was the only data blob, so the operation is now done.
        assert_matches!(serve_needed_task.await, Ok(()));
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn abort_aborts_while_waiting_for_open_meta_blob(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (task, proxy, blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let abort_fut = proxy.abort();

        assert_matches!(task.await, Err(ServeNeededBlobsError::Aborted));
        assert_matches!(
            abort_fut.await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn abort_aborts_while_waiting_for_get_missing_blobs(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![]).await;

        let abort_fut = proxy.abort();

        assert_matches!(serve_needed_task.await, Err(ServeNeededBlobsError::Aborted));
        assert_matches!(
            abort_fut.await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
        blobfs.expect_done().await;
    }

    #[test_case(fpkg::GcProtection::OpenPackageTracking; "open-package-tracking")]
    #[test_case(fpkg::GcProtection::Retained; "retained")]
    #[fuchsia::test]
    async fn abort_aborts_while_waiting_for_open_blobs(gc_protection: fpkg::GcProtection) {
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (serve_needed_task, proxy, mut blobfs) =
            spawn_serve_needed_blobs_with_mocks(meta_blob_info, gc_protection);

        let () = write_meta_blob(&proxy, &mut blobfs, meta_blob_info, vec![[2; 32].into()]).await;
        enumerate_readable_missing_blobs(
            &proxy,
            &mut blobfs,
            std::iter::empty(),
            vec![[2; 32].into()].into_iter(),
        )
        .await;

        let abort_fut = proxy.abort();

        assert_matches!(serve_needed_task.await, Err(ServeNeededBlobsError::Aborted));
        assert_matches!(
            abort_fut.await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
        blobfs.expect_done().await;
    }
}

#[cfg(test)]
mod get_handler_tests {
    use super::*;
    use crate::{COBALT_CONNECTOR_BUFFER_SIZE, CobaltConnectedService, ProtocolConnector};

    #[fuchsia::test]
    async fn everything_closed() {
        let (_, stream) = fidl::endpoints::create_proxy::<NeededBlobsMarker>();
        let meta_blob_info = BlobInfo { blob_id: [0; 32].into(), length: 0 };
        let (blobfs, _, _, _) = blobfs::Client::new_test();
        let inspector = fuchsia_inspect::Inspector::default();
        let package_index = Arc::new(async_lock::RwLock::new(PackageIndex::new()));
        let (root_dir_factory, open_packages) = crate::root_dir::new_test(blobfs.clone()).await;

        assert_matches::assert_matches!(
            get(
                &package_index,
                &BaseIndex::new_test_only(HashSet::new(), vec![]),
                &CacheIndex::new_test_only(HashSet::new(), vec![]),
                system_image::ExecutabilityRestrictions::DoNotEnforce,
                &blobfs,
                &root_dir_factory,
                &open_packages,
                meta_blob_info,
                fpkg::GcProtection::OpenPackageTracking,
                stream,
                fidl::endpoints::create_endpoints().1,
                package_directory::ExecutionScope::new(),
                ProtocolConnector::new_with_buffer_size(
                    CobaltConnectedService,
                    COBALT_CONNECTOR_BUFFER_SIZE,
                )
                .serve_and_log_errors()
                .0,
                &inspector.root().create_child("get"),
            )
            .await,
            Err(Status::UNAVAILABLE)
        );
    }
}

#[cfg(test)]
mod serve_package_index_tests {
    use super::*;
    use fpkg::PackageIndexIteratorMarker;

    #[fuchsia::test]
    async fn entries_converted_correctly() {
        let cache_packages = [
            (
                UnpinnedAbsolutePackageUrl::new(
                    "fuchsia-pkg://fuchsia.test".parse().unwrap(),
                    "name0".parse().unwrap(),
                    None,
                ),
                Hash::from([0u8; 32]),
            ),
            (
                UnpinnedAbsolutePackageUrl::new(
                    "fuchsia-pkg://fuchsia.test".parse().unwrap(),
                    "name1".parse().unwrap(),
                    None,
                ),
                Hash::from([1u8; 32]),
            ),
        ];

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<PackageIndexIteratorMarker>();
        let task = Task::local(async move {
            serve_package_index(cache_packages.iter().map(|(url, hash)| (url, hash)), stream).await
        });

        let entries = proxy.next().await.unwrap();
        assert_eq!(
            entries,
            vec![
                fpkg::PackageIndexEntry {
                    package_url: fpkg::PackageUrl {
                        url: "fuchsia-pkg://fuchsia.test/name0".to_string()
                    },
                    meta_far_blob_id: fpkg::BlobId { merkle_root: [0u8; 32] }
                },
                fpkg::PackageIndexEntry {
                    package_url: fpkg::PackageUrl {
                        url: "fuchsia-pkg://fuchsia.test/name1".to_string()
                    },
                    meta_far_blob_id: fpkg::BlobId { merkle_root: [1u8; 32] }
                },
            ]
        );

        let entries = proxy.next().await.unwrap();
        assert_eq!(entries, vec![]);

        let () = task.await;
    }
}
