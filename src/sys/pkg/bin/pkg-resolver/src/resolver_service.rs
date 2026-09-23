// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cache::CacheError::*;
use crate::cache::MerkleForError::*;
use crate::cache::{
    BasePackageIndex, BlobFetcher, MerkleForError, ToResolveError, ToResolveStatus as _,
};
use crate::eager_package_manager::EagerPackageManager;
use crate::error::to_resolve_tool_error;
use crate::repository_manager::GetPackageError::*;
use crate::repository_manager::{GetPackageError, GetPackageHashError, RepositoryManager};
use crate::rewrite_manager::RewriteManager;
use anyhow::{Context as _, Error, anyhow};
use async_lock::RwLock as AsyncRwLock;
use async_trait::async_trait;
use cobalt_sw_delivery_registry as metrics;
use fidl::endpoints::ServerEnd;
use fidl::marker::SourceBreaking;
use fidl_contrib::protocol_connector::ProtocolSender;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics::MetricEvent;
use fidl_fuchsia_pkg::{self as fpkg, PackageResolverRequest, PackageResolverRequestStream};
use fidl_fuchsia_pkg_ext::{self as pkg, BlobId};
use fidl_fuchsia_pkg_resolution::{self as fpkg_resolution};
use fuchsia_cobalt_builders::MetricEventExt as _;
use fuchsia_pkg::PackageDirectory;
use fuchsia_trace as ftrace;
use fuchsia_url::ParseError;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use futures::future::Future;
use futures::stream::TryStreamExt as _;
use log::{error, info, warn};
use std::sync::Arc;
use std::time::Instant;
use system_image::CachePackages;
use zx::Status;

mod inspect;
pub use inspect::ResolverService as ResolverServiceInspectState;

mod resolve_with_context;

const SLOW_CACHE_FALLBACK_WARN_DURATION: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(10);
const SLOW_CACHE_FALLBACK_WARN_SQUELCH_DURATION: zx::MonotonicDuration =
    zx::MonotonicDuration::from_minutes(10);

/// Work-queue based package resolver. When all clones of
/// [`QueuedResolver`] are dropped, the queue will resolve all remaining
/// packages and terminate its output stream.
#[derive(Clone, Debug)]
pub struct QueuedResolver {
    queue: work_queue::WorkSender<
        AbsolutePackageUrl,
        ResolveQueueContext,
        Result<(BlobId, PackageDirectory), Arc<GetPackageError>>,
    >,
    cache: pkg::cache::Client,
    base_package_index: Arc<BasePackageIndex>,
    rewriter: Arc<AsyncRwLock<RewriteManager>>,
    system_cache_list: Arc<CachePackages>,
    inspect: Arc<ResolverServiceInspectState>,
    last_slow_cache_fallback_log_time: Arc<fuchsia_sync::Mutex<zx::MonotonicInstant>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolveQueueContext {
    gc_protection: fpkg::GcProtection,
    trace_id: ftrace::Id,
}

impl ResolveQueueContext {
    fn new(gc_protection: fpkg::GcProtection, trace_id: ftrace::Id) -> Self {
        Self { gc_protection, trace_id }
    }
}

impl work_queue::TryMerge for ResolveQueueContext {
    // Merges Contexts with different trace ids. This will not leak trace durations because the
    // active duration associated with this id is started and stopped in QueuedResolver::resolve.
    // If a resolve is merged, then to tell which blob fetches the merged resolve is waiting for you
    // need to find the trace id of the first active resolve with the same package URL.
    //
    // Does not merge Contexts with differing GC protection. Clients depend on the different
    // GC protection behaviors.
    fn try_merge(&mut self, other: Self) -> Result<(), Self> {
        if self.gc_protection == other.gc_protection { Ok(()) } else { Err(other) }
    }
}

/// This trait represents an instance which can be used to resolve package URLs, which typically
/// ensures that packages are available on the local filesystem and/or fetch them from remote
/// repository if necessary and possible.
#[async_trait]
pub trait Resolver: std::fmt::Debug + Sync + Sized {
    /// Resolves the given absolute package URL and returns the package
    /// directory and a resolution context for resolving subpackages.
    async fn resolve(
        &self,
        url: AbsolutePackageUrl,
        gc_protection: fpkg::GcProtection,
        eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<Self>>>,
    ) -> Result<(PackageDirectory, pkg::ResolutionContext), pkg::ResolveError>;
}

// How the package directory was resolved.
struct PackageWithSourceAndBlobId {
    package: PackageDirectory,
    source: PackageSource,
    blob_id: BlobId,
}

impl PackageWithSourceAndBlobId {
    fn base(package: PackageDirectory, blob_id: BlobId) -> Self {
        Self { package, source: PackageSource::Base, blob_id }
    }

    fn eager(package: PackageDirectory, blob_id: BlobId) -> Self {
        Self { package, source: PackageSource::Eager, blob_id }
    }

    fn tuf(package: PackageDirectory, blob_id: BlobId) -> Self {
        Self { package, source: PackageSource::Tuf, blob_id }
    }

    fn cache(package: PackageDirectory, blob_id: BlobId) -> Self {
        Self { package, source: PackageSource::Cache, blob_id }
    }
}

enum PackageSource {
    Base,
    Eager,
    Tuf,
    Cache,
}

impl PackageSource {
    fn str_for_trace(&self) -> &'static str {
        use PackageSource::*;
        match self {
            Base => "base pinned",
            Eager => "eager package manager",
            Tuf => "tuf ephemeral resolution",
            Cache => "cache fallback",
        }
    }
}

#[async_trait]
impl Resolver for QueuedResolver {
    async fn resolve(
        &self,
        pkg_url: AbsolutePackageUrl,
        gc_protection: fpkg::GcProtection,
        eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<Self>>>,
    ) -> Result<(PackageDirectory, pkg::ResolutionContext), pkg::ResolveError> {
        let trace_id = ftrace::Id::new();
        let guard = ftrace::async_enter!(
            trace_id, c"app", c"resolve",
            "url" => pkg_url.to_string().as_str(),
            // An async duration cannot have multiple concurrent child async durations
            // so we include the id as metadata to manually determine the
            // relationship.
            "trace_id" => u64::from(trace_id)
        );
        let resolve_res =
            self.resolve_with_source(pkg_url, gc_protection, eager_package_manager, trace_id).await;
        let error_string;
        if let Some(inner) = guard {
            inner.end(&[
                ftrace::ArgValue::of(
                    "status",
                    match resolve_res {
                        Ok(_) => "success",
                        Err(ref e) => {
                            error_string = e.to_string();
                            error_string.as_str()
                        }
                    },
                ),
                ftrace::ArgValue::of(
                    "source",
                    match resolve_res {
                        Ok(ref package_with_source) => package_with_source.source.str_for_trace(),
                        Err(_) => "no source because resolve failed",
                    },
                ),
            ]);
        }
        resolve_res.map(|pkg_with_source| (pkg_with_source.package, pkg_with_source.blob_id.into()))
    }
}

impl QueuedResolver {
    /// Creates an unbounded queue that will resolve up to `max_concurrency` packages at once.
    /// Returns:
    ///   1. a Future to be awaited that processes the queue
    ///   2. a Self that enables pushing work onto the queue
    pub fn new(
        cache_client: pkg::cache::Client,
        base_package_index: Arc<BasePackageIndex>,
        system_cache_list: Arc<CachePackages>,
        repo_manager: Arc<AsyncRwLock<RepositoryManager>>,
        rewriter: Arc<AsyncRwLock<RewriteManager>>,
        blob_fetcher: BlobFetcher,
        max_concurrency: usize,
        inspect: Arc<ResolverServiceInspectState>,
    ) -> (impl Future<Output = ()>, Self) {
        let cache = cache_client.clone();
        let (package_fetch_queue, queue) = work_queue::work_queue(
            max_concurrency,
            move |rewritten_url: AbsolutePackageUrl, context: ResolveQueueContext| {
                let cache = cache_client.clone();
                let repo_manager = Arc::clone(&repo_manager);
                let blob_fetcher = blob_fetcher.clone();
                async move {
                    package_from_repo(
                        &repo_manager,
                        &rewritten_url,
                        context.gc_protection,
                        cache,
                        blob_fetcher,
                        context.trace_id,
                    )
                    .await
                    .map_err(Arc::new)
                }
            },
        );
        let () = inspect.record_raw_queue(package_fetch_queue.record_lazy_inspect());
        let fetcher = Self {
            queue,
            inspect,
            base_package_index,
            cache,
            rewriter,
            system_cache_list,
            last_slow_cache_fallback_log_time: Arc::new(fuchsia_sync::Mutex::new(
                zx::MonotonicInstant::INFINITE_PAST,
            )),
        };
        (package_fetch_queue.into_future(), fetcher)
    }

    async fn resolve_with_source(
        &self,
        pkg_url: AbsolutePackageUrl,
        gc_protection: fpkg::GcProtection,
        eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<Self>>>,
        trace_id: ftrace::Id,
    ) -> Result<PackageWithSourceAndBlobId, pkg::ResolveError> {
        // Use boot timeline for inspect to correlate with the syslog, use monotonic timeline to
        // warn about slow cache fallback to avoid warning just b/c the device suspended.
        let start_boot = zx::BootInstant::get();
        let start_mono = zx::MonotonicInstant::get();
        // Base pin.
        let package_inspect = self.inspect.resolve(&pkg_url, gc_protection);
        if let Some(blob) = self.base_package_index.is_unpinned_base_package(&pkg_url) {
            let dir = self.cache.get_already_cached(blob).await.map_err(|e| {
                let error = e.to_resolve_error();
                error!("failed to open base package url {:?}: {:#}", pkg_url, anyhow!(e));
                error
            })?;
            self.inspect.successful_resolve(
                "base",
                &pkg_url,
                None,
                gc_protection,
                None,
                &blob,
                start_boot,
            );
            return Ok(PackageWithSourceAndBlobId::base(dir, blob));
        }

        // Rewrite the url.
        let rewritten_url =
            rewrite_url(&self.rewriter, &pkg_url).await.map_err(|e| e.to_resolve_error())?;
        let _package_inspect = package_inspect.rewritten_url(&rewritten_url);

        // Attempt to use EagerPackageManager to resolve the package.
        if let Some(eager_package_manager) = eager_package_manager
            && let Some((dir, hash)) =
                eager_package_manager.read().await.get_package_dir(&rewritten_url).map_err(|e| {
                    error!(
                        "failed to resolve eager package at {} as {}: {:#}",
                        pkg_url, rewritten_url, e
                    );
                    pkg::ResolveError::PackageNotFound
                })?
        {
            self.inspect.successful_resolve(
                "eager package manager",
                &pkg_url,
                Some(&rewritten_url),
                gc_protection,
                None,
                &hash.into(),
                start_boot,
            );
            return Ok(PackageWithSourceAndBlobId::eager(dir, hash.into()));
        }

        // Fetch from TUF.
        let queued_fetch = self
            .queue
            .push(rewritten_url.clone(), ResolveQueueContext::new(gc_protection, trace_id));
        match queued_fetch.await.expect("expected queue to be open") {
            Ok((hash, dir)) => {
                self.inspect.successful_resolve(
                    "TUF",
                    &pkg_url,
                    Some(&rewritten_url),
                    gc_protection,
                    None,
                    &hash,
                    start_boot,
                );
                Ok(PackageWithSourceAndBlobId::tuf(dir, hash))
            }
            Err(tuf_err) => {
                match self.handle_cache_fallbacks(&tuf_err, &pkg_url, &rewritten_url).await {
                    Ok(Some((hash, pkg))) => {
                        self.inspect.successful_resolve(
                            "cache",
                            &pkg_url,
                            Some(&rewritten_url),
                            gc_protection,
                            Some(anyhow!(tuf_err)),
                            &hash,
                            start_boot,
                        );
                        let () = self.log_slow_cache_fallback(start_mono, &pkg_url, &rewritten_url);
                        Ok(PackageWithSourceAndBlobId::cache(pkg, hash))
                    }
                    Ok(None) => {
                        let fidl_err = tuf_err.to_resolve_error();
                        warn!(
                            "failed to resolve {} as {} with TUF: {:#}",
                            pkg_url,
                            rewritten_url,
                            anyhow!(tuf_err)
                        );
                        Err(fidl_err)
                    }
                    Err(fallback_err) => {
                        let fidl_err = fallback_err.to_resolve_error();
                        error!(
                            "failed to resolve {} as {} with cache packages fallback: {:#}. \
                            fallback was attempted because TUF failed with {:#}",
                            pkg_url,
                            rewritten_url,
                            anyhow!(fallback_err),
                            anyhow!(tuf_err)
                        );
                        Err(fidl_err)
                    }
                }
            }
        }
    }

    fn log_slow_cache_fallback(
        &self,
        start_ts: zx::MonotonicInstant,
        pkg_url: &AbsolutePackageUrl,
        rewritten_url: &AbsolutePackageUrl,
    ) {
        let now = zx::MonotonicInstant::get();
        let resolve_duration = now - start_ts;
        if resolve_duration < SLOW_CACHE_FALLBACK_WARN_DURATION {
            return;
        }
        {
            let mut last = self.last_slow_cache_fallback_log_time.lock();
            if now - *last < SLOW_CACHE_FALLBACK_WARN_SQUELCH_DURATION {
                return;
            } else {
                *last = now;
            }
        }
        warn!(
            "Resolve of {} as {} via cache fallback took {} seconds. This could be slowing down \
             your system, and may be due to trouble connecting to a remote repository. This log \
             will only print every {} minutes, so the issue may be occuring more often. See \
             inspect for more information.",
            pkg_url,
            rewritten_url,
            resolve_duration.into_seconds(),
            SLOW_CACHE_FALLBACK_WARN_SQUELCH_DURATION.into_minutes(),
        );
    }

    // On success returns the package directory and the package's hash for easier logging (the
    // package's hash could be obtained from the package directory but that would require reading
    // the package's meta file).
    async fn handle_cache_fallbacks(
        &self,
        tuf_error: &GetPackageError,
        pkg_url: &AbsolutePackageUrl,
        rewritten_url: &AbsolutePackageUrl,
    ) -> Result<Option<(BlobId, PackageDirectory)>, pkg::cache::GetAlreadyCachedError> {
        match tuf_error {
            Cache(MerkleFor(TargetNotFound(_))) => {
                // If we can get metadata but the repo doesn't know about the package,
                // it shouldn't be in the cache, BUT some SDK customers currently rely on this
                // behavior.
                // TODO(https://fxbug.dev/42127880): remove this behavior.
                match missing_cache_package_disk_fallback(
                    rewritten_url,
                    pkg_url,
                    &self.system_cache_list,
                    &self.inspect,
                ) {
                    Some(hash) => {
                        self.cache.get_already_cached(hash).await.map(|pkg| Some((hash, pkg)))
                    }
                    None => Ok(None),
                }
            }
            RepoNotFound(..)
            | NoMirrors(..)
            | BlobUrl(..)
            | OpenRepo(..)
            | Cache(Fidl(..))
            | Cache(ListNeeds(..))
            | Cache(MerkleFor(MetadataNotFound { .. }))
            | Cache(MerkleFor(FetchTargetDescription(..)))
            | Cache(MerkleFor(InvalidTargetPath(..)))
            | Cache(MerkleFor(NoCustomMetadata))
            | Cache(MerkleFor(SerdeError(..))) => {
                // If we couldn't get TUF metadata, we might not have networking. Check the
                // cache packages manifest obtained from pkg-cache.
                // The manifest pkg URLs are for fuchsia.com, so do not use the rewritten URL.
                match hash_from_cache_packages_manifest(pkg_url, &self.system_cache_list) {
                    Some(hash) => {
                        self.cache.get_already_cached(hash).await.map(|pkg| Some((hash, pkg)))
                    }
                    None => Ok(None),
                }
            }
            OpenPackage(..)
            | Cache(FetchContentBlob(..))
            | Cache(FetchMetaFar(..))
            | Cache(Get(_))
            | Cache(Open(_)) => {
                // We could talk to TUF and we know there's a new version of this package,
                // but we couldn't retrieve its blobs for some reason. Refuse to fall back to
                // cache_packages and instead return an error for the resolve, which is consistent
                // with the path for packages which are not in cache_packages.
                //
                // We don't use cache_packages in production, and when developers resolve a package
                // on a bench they expect the newest version, or a failure. cache_packages are great
                // for running packages before networking is up, but for these error conditions,
                // we know we have networking because we could talk to TUF.
                Ok(None)
            }
        }
    }
}

#[derive(Clone, Debug)]
#[cfg(test)]
/// Creates a mocked PackageResolver that resolves any url using the given callback.
pub struct MockResolver {
    #[allow(clippy::type_complexity)]
    queue: work_queue::WorkSender<
        (AbsolutePackageUrl, fpkg::GcProtection),
        (),
        Result<(PackageDirectory, pkg::ResolutionContext), Arc<GetPackageError>>,
    >,
}

#[cfg(test)]
impl MockResolver {
    pub fn new<W, F>(callback: W) -> Self
    where
        W: Fn(AbsolutePackageUrl, fpkg::GcProtection) -> F + Send + 'static,
        F: Future<
                Output = Result<(PackageDirectory, pkg::ResolutionContext), Arc<GetPackageError>>,
            > + Send,
    {
        let (package_fetch_queue, queue) =
            work_queue::work_queue(1, move |(url, gc_protection), _: ()| {
                callback(url, gc_protection)
            });
        fuchsia_async::Task::spawn(package_fetch_queue.into_future()).detach();
        Self { queue }
    }
}

#[cfg(test)]
#[async_trait]
impl Resolver for MockResolver {
    async fn resolve(
        &self,
        url: AbsolutePackageUrl,
        gc_protection: fpkg::GcProtection,
        _eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<Self>>>,
    ) -> Result<(PackageDirectory, pkg::ResolutionContext), pkg::ResolveError> {
        let queued_fetch = self.queue.push((url, gc_protection), ());
        queued_fetch.await.expect("expected queue to be open").map_err(|e| e.to_resolve_error())
    }
}

pub async fn run_resolver_service(
    repo_manager: Arc<AsyncRwLock<RepositoryManager>>,
    rewriter: Arc<AsyncRwLock<RewriteManager>>,
    package_resolver: QueuedResolver,
    pkg_cache: pkg::cache::Client,
    base_package_index: Arc<BasePackageIndex>,
    system_cache_list: Arc<CachePackages>,
    stream: PackageResolverRequestStream,
    gc_protection: fpkg::GcProtection,
    cobalt_sender: ProtocolSender<MetricEvent>,
    inspect: Arc<ResolverServiceInspectState>,
    eager_package_manager: Arc<Option<AsyncRwLock<EagerPackageManager<QueuedResolver>>>>,
) -> Result<(), Error> {
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, |event| async {
            match event {
                PackageResolverRequest::Resolve { package_url, dir, responder } => {
                    let response = resolve_unparsed_absolute_url_and_send_cobalt_metrics(
                        &package_url,
                        gc_protection,
                        dir,
                        &package_resolver,
                        eager_package_manager.as_ref().as_ref(),
                        cobalt_sender.clone(),
                    )
                    .await;
                    let () =
                        responder.send(response.as_ref().map_err(|e| e.clone().into())).with_context(|| {
                            format!(
                                "sending fuchsia.pkg/PackageResolver.Resolve response for {package_url:?}"
                            )
                        })?;
                    Ok(())
                }
                PackageResolverRequest::ResolveWithContext {
                    package_url,
                    context,
                    dir,
                    responder,
                } => {
                    let response = resolve_with_context::resolve_with_context(
                        package_url.clone(),
                        context,
                        gc_protection,
                        dir,
                        &package_resolver,
                        &pkg_cache,
                        eager_package_manager.as_ref().as_ref(),
                        cobalt_sender.clone(),
                        &inspect,
                    )
                    .await;
                    let () =
                        responder.send(response.as_ref().map_err(|e| e.clone().into())).with_context(|| {
                            format!(
                                "sending fuchsia.pkg/PackageResolver.ResolveWithContext response \
                                 for {package_url:?}"
                            )
                        })?;
                    Ok(())
                }
                PackageResolverRequest::GetHash { package_url, responder } => {
                    match get_hash(
                        &rewriter,
                        &repo_manager,
                        &base_package_index,
                        &system_cache_list,
                        &package_url.url,
                        &inspect,
                        eager_package_manager.as_ref().as_ref(),
                    )
                    .await
                    {
                        Ok(blob_id) => {
                            responder.send(Ok(&blob_id.into())).with_context(|| {
                                format!(
                                    "sending fuchsia.pkg/PackageResolver.GetHash success \
                                         response for {:?}",
                                    package_url.url
                                )
                            })?;
                        }
                        Err(status) => {
                            responder.send(Err(status.into_raw())).with_context(|| {
                                format!(
                                    "sending fuchsia.pkg/PackageResolver.GetHash failure \
                                         response for {:?}",
                                    package_url.url
                                )
                            })?;
                        }
                    }
                    Ok(())
                }
            }
        })
        .await
}

pub async fn run_resolver_toolbox_service(
    package_resolver: QueuedResolver,
    stream: fpkg_resolution::PackageResolverRequestStream,
    gc_protection: fpkg::GcProtection,
    cobalt_sender: ProtocolSender<MetricEvent>,
    eager_package_manager: Arc<Option<AsyncRwLock<EagerPackageManager<QueuedResolver>>>>,
) -> Result<(), Error> {
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, |event| async {
            match event {
                fpkg_resolution::PackageResolverRequest::Resolve { payload, responder } => {
                    let package_url = payload.package_url.ok_or_else(|| anyhow!("No package URL given"))?;
                    let (_dir, dir_server_end) = fidl::endpoints::create_proxy();
                    let response = resolve_unparsed_absolute_url_and_send_cobalt_metrics(
                        &package_url,
                        gc_protection,
                        dir_server_end,
                        &package_resolver,
                        eager_package_manager.as_ref().as_ref(),
                        cobalt_sender.clone(),
                    )
                    .await
                    .map(|_| { fpkg_resolution::ResolveResult {
                        __source_breaking: SourceBreaking
                    }});
                    let () =
                        responder.send(response.map_err(to_resolve_tool_error)).with_context(|| {
                            format!(
                                "sending fuchsia.pkg/PackageResolverToolbox.Resolve response for {package_url:?}"
                            )
                        })?;
                    Ok(())
                },
                fpkg_resolution::PackageResolverRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal:?; "Unknown PackageResolverRequest method");
                    Ok(())
                },
            }
        })
        .await
}

async fn resolve_unparsed_absolute_url_and_send_cobalt_metrics(
    url: &str,
    gc_protection: fpkg::GcProtection,
    dir: ServerEnd<fio::DirectoryMarker>,
    package_resolver: &QueuedResolver,
    eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<QueuedResolver>>>,
    cobalt_sender: ProtocolSender<MetricEvent>,
) -> Result<fpkg::ResolutionContext, pkg::ResolveError> {
    let url = AbsolutePackageUrl::parse(url).map_err(|e| handle_bad_package_url_error(e, url))?;
    resolve_absolute_url_and_send_cobalt_metrics(
        url,
        gc_protection,
        dir,
        package_resolver,
        eager_package_manager,
        cobalt_sender,
    )
    .await
}

async fn resolve_absolute_url_and_send_cobalt_metrics(
    package_url: AbsolutePackageUrl,
    gc_protection: fpkg::GcProtection,
    dir: ServerEnd<fio::DirectoryMarker>,
    package_resolver: &QueuedResolver,
    eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<QueuedResolver>>>,
    mut cobalt_sender: ProtocolSender<MetricEvent>,
) -> Result<fpkg::ResolutionContext, pkg::ResolveError> {
    let start_time = Instant::now();
    let response = resolve_and_reopen(
        package_resolver,
        package_url,
        gc_protection,
        dir,
        eager_package_manager,
    )
    .await;

    cobalt_sender.send(
        MetricEvent::builder(metrics::RESOLVE_STATUS_MIGRATED_METRIC_ID)
            .with_event_codes(resolve_result_to_resolve_status_code(&response))
            .as_occurrence(1),
    );

    cobalt_sender.send(
        MetricEvent::builder(metrics::RESOLVE_DURATION_MIGRATED_METRIC_ID)
            .with_event_codes((
                resolve_result_to_resolve_duration_code(&response),
                metrics::ResolveDurationMigratedMetricDimensionResolverType::Regular,
            ))
            .as_integer(Instant::now().duration_since(start_time).as_micros() as i64),
    );

    response
}

async fn rewrite_url(
    rewriter: &AsyncRwLock<RewriteManager>,
    url: &AbsolutePackageUrl,
) -> Result<AbsolutePackageUrl, Status> {
    Ok(rewriter.read().await.rewrite(url))
}

fn missing_cache_package_disk_fallback(
    rewritten_url: &AbsolutePackageUrl,
    pkg_url: &AbsolutePackageUrl,
    system_cache_list: &CachePackages,
    inspect: &ResolverServiceInspectState,
) -> Option<BlobId> {
    let possible_fallback = hash_from_cache_packages_manifest(pkg_url, system_cache_list);
    if possible_fallback.is_some() {
        warn!(
            "Did not find {} at URL {}, but did find a matching package name in the \
            built-in cache packages set, so falling back to it. Your package \
            repository may not be configured to serve the package correctly, or may \
            be overriding the domain for the repository which would normally serve \
            this package. This will be an error in a future version of Fuchsia, see \
            https://fxbug.dev/42127862.",
            rewritten_url.name(),
            rewritten_url
        );
        inspect.cache_fallback_due_to_not_found();
    }
    possible_fallback
}

enum HashSource<TufError> {
    Tuf(BlobId),
    SystemImageCachePackages(BlobId, TufError),
}

async fn hash_from_base_or_repo_or_cache(
    repo_manager: &AsyncRwLock<RepositoryManager>,
    rewriter: &AsyncRwLock<RewriteManager>,
    base_package_index: &BasePackageIndex,
    system_cache_list: &CachePackages,
    pkg_url: &AbsolutePackageUrl,
    inspect_state: &ResolverServiceInspectState,
    eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<QueuedResolver>>>,
) -> Result<BlobId, Status> {
    if let Some(blob) = base_package_index.is_unpinned_base_package(pkg_url) {
        info!("get_hash for {} to {} with base pin", pkg_url, blob);
        return Ok(blob);
    }

    let rewritten_url = rewrite_url(rewriter, pkg_url).await?;

    // Attempt to use EagerPackageManager to resolve the package.
    if let Some(eager_package_manager) = eager_package_manager
        && let Some((_, hash)) =
            eager_package_manager.read().await.get_package_dir(&rewritten_url).map_err(|err| {
                error!(
                    "retrieval error eager package url {} as {}: {:#}",
                    pkg_url,
                    rewritten_url,
                    anyhow!(err)
                );
                Status::NOT_FOUND
            })?
    {
        info!(
            "get_hash for {} as {} to {} with eager package manager",
            pkg_url, rewritten_url, hash
        );
        return Ok(hash.into());
    }

    hash_from_repo_or_cache(repo_manager, system_cache_list, pkg_url, &rewritten_url, inspect_state)
        .await
        .map_err(|e| {
            let status = e.to_resolve_status();
            warn!("error getting hash {} as {}: {:#}", pkg_url, rewritten_url, anyhow!(e));
            status
        })
        .map(|hash| match hash {
            HashSource::Tuf(blob) => {
                info!("get_hash for {} as {} to {} with TUF", pkg_url, rewritten_url, blob);
                blob
            }
            HashSource::SystemImageCachePackages(blob, tuf_err) => {
                info!(
                    "get_hash for {} as {} to {} with cache_packages due to {:#}",
                    pkg_url,
                    rewritten_url,
                    blob,
                    anyhow!(tuf_err)
                );
                blob
            }
        })
}
async fn hash_from_repo_or_cache(
    repo_manager: &AsyncRwLock<RepositoryManager>,
    system_cache_list: &CachePackages,
    pkg_url: &AbsolutePackageUrl,
    rewritten_url: &AbsolutePackageUrl,
    inspect_state: &ResolverServiceInspectState,
) -> Result<HashSource<GetPackageHashError>, GetPackageHashError> {
    // The RwLock created by `.read()` must not exist across the `.await` (to e.g. prevent
    // deadlock). Rust temporaries are kept alive for the duration of the innermost enclosing
    // statement, so the following two lines should not be combined.
    let fut = repo_manager.read().await.get_package_hash(rewritten_url);
    match fut.await {
        Ok((b, _)) => Ok(HashSource::Tuf(b)),
        Err(e @ GetPackageHashError::MerkleFor(MerkleForError::TargetNotFound(_))) => {
            // If we can get metadata but the repo doesn't know about the package,
            // it shouldn't be in the cache, BUT some SDK customers currently rely on this behavior.
            // TODO(https://fxbug.dev/42127880): remove this behavior.
            match missing_cache_package_disk_fallback(
                rewritten_url,
                pkg_url,
                system_cache_list,
                inspect_state,
            ) {
                Some(blob) => Ok(HashSource::SystemImageCachePackages(blob, e)),
                None => Err(e),
            }
        }
        Err(e) => {
            // If we couldn't get TUF metadata, we might not have networking. Check in
            // the cache packages manifest (not to be confused with pkg-cache). The
            // cache packages manifest pkg URLs are for fuchsia.com, so do not use the
            // rewritten URL.
            match hash_from_cache_packages_manifest(pkg_url, system_cache_list) {
                Some(blob) => Ok(HashSource::SystemImageCachePackages(blob, e)),
                None => Err(e),
            }
        }
    }
}

// On success returns the resolved package directory and the package's hash for convenient
// logging (the package hash could be obtained from the package directory but that would
// require reading the package's meta file).
async fn package_from_repo(
    repo_manager: &AsyncRwLock<RepositoryManager>,
    rewritten_url: &AbsolutePackageUrl,
    gc_protection: fpkg::GcProtection,
    cache: pkg::cache::Client,
    blob_fetcher: BlobFetcher,
    trace_id: ftrace::Id,
) -> Result<(BlobId, PackageDirectory), GetPackageError> {
    // Rust temporaries are kept alive for the duration of the innermost enclosing statement, and
    // we don't want to hold the repo_manager lock while we fetch the package, so the following two
    // lines should not be combined.
    let fut = repo_manager.read().await.get_package(
        rewritten_url,
        gc_protection,
        &cache,
        &blob_fetcher,
        trace_id,
    );
    fut.await
}

// Attempts to lookup the hash of a package from `system_cache_list`, which is populated from the
// cache_packages manifest of the system_image package.
fn hash_from_cache_packages_manifest(
    url: &AbsolutePackageUrl,
    system_cache_list: &CachePackages,
) -> Option<BlobId> {
    // TODO(https://fxbug.dev/335388895)
    // We are in the process of removing the concept of package variant
    // (generalizing fuchsia-pkg URL paths to be `(first-segment)(/more-segments)*`
    // instead of requiring that paths are `(name)/(variant)`. Towards this goal,
    // the URLs the pkg-resolver gets from the pkg-cache from `PackageCache.CachePackageIndex`
    // do not have variants. However, they are intended to match only URLs with variant of "0".
    // Additionally, pkg-resolver allows clients to not specify a variant, in which case a
    // variant of "0" will be assumed. This means that if the URL we are resolving has a
    // variant that is not "0", it should never match anything in the cache packages manifest,
    // and if the URL has a variant of "0", we should remove it before checking the cache manifest.
    let mut no_variant;
    let url = match url.variant() {
        None => url,
        Some(variant) if !variant.is_zero() => {
            return None;
        }
        Some(_) => {
            no_variant = url.clone();
            no_variant.clear_variant();
            &no_variant
        }
    };

    system_cache_list.hash_for_package(url).map(Into::into)
}

async fn get_hash(
    rewriter: &AsyncRwLock<RewriteManager>,
    repo_manager: &AsyncRwLock<RepositoryManager>,
    base_package_index: &BasePackageIndex,
    system_cache_list: &CachePackages,
    url: &str,
    inspect_state: &ResolverServiceInspectState,
    eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<QueuedResolver>>>,
) -> Result<BlobId, Status> {
    let pkg_url = AbsolutePackageUrl::parse(url).map_err(|e| handle_bad_package_url(e, url))?;

    ftrace::duration_begin!(c"app", c"get-hash", "url" => pkg_url.to_string().as_str());
    let hash_or_status = hash_from_base_or_repo_or_cache(
        repo_manager,
        rewriter,
        base_package_index,
        system_cache_list,
        &pkg_url,
        inspect_state,
        eager_package_manager,
    )
    .await;
    let err_str;
    let status_str = match &hash_or_status {
        Ok(_) => "OK",
        Err(s) => {
            err_str = s.to_string();
            err_str.as_str()
        }
    };
    ftrace::duration_end!(c"app", c"get-hash", "status" => status_str);
    hash_or_status
}

async fn resolve_and_reopen(
    package_resolver: &QueuedResolver,
    url: AbsolutePackageUrl,
    gc_protection: fpkg::GcProtection,
    dir_request: ServerEnd<fio::DirectoryMarker>,
    eager_package_manager: Option<&AsyncRwLock<EagerPackageManager<QueuedResolver>>>,
) -> Result<fpkg::ResolutionContext, pkg::ResolveError> {
    let (pkg, resolution_context) =
        package_resolver.resolve(url.clone(), gc_protection, eager_package_manager).await?;
    let () = pkg.reopen(dir_request).map_err(|e| {
        error!("failed to re-open directory for package url {}: {:#}", url, anyhow!(e));
        pkg::ResolveError::Internal
    })?;
    Ok(resolution_context.into())
}

fn handle_bad_package_url_error(parse_error: ParseError, pkg_url: &str) -> pkg::ResolveError {
    error!("failed to parse package url {:?}: {:#}", pkg_url, anyhow!(parse_error));
    pkg::ResolveError::InvalidUrl
}

fn handle_bad_package_url(parse_error: ParseError, pkg_url: &str) -> Status {
    error!("failed to parse package url {:?}: {:#}", pkg_url, anyhow!(parse_error));
    Status::INVALID_ARGS
}

fn resolve_result_to_resolve_duration_code<T>(
    res: &Result<fpkg::ResolutionContext, T>,
) -> metrics::ResolveDurationMigratedMetricDimensionResult {
    use metrics::ResolveDurationMigratedMetricDimensionResult as EventCodes;
    match res {
        Ok(_) => EventCodes::Success,
        Err(_) => EventCodes::Failure,
    }
}

fn resolve_result_to_resolve_status_code(
    result: &Result<fpkg::ResolutionContext, pkg::ResolveError>,
) -> metrics::ResolveStatusMigratedMetricDimensionResult {
    use metrics::ResolveStatusMigratedMetricDimensionResult as EventCodes;
    match *result {
        Ok(_) => EventCodes::Success,
        Err(pkg::ResolveError::Internal) => EventCodes::Internal,
        Err(pkg::ResolveError::AccessDenied) => EventCodes::AccessDenied,
        Err(pkg::ResolveError::Io) => EventCodes::Io,
        Err(pkg::ResolveError::BlobNotFound) => EventCodes::BlobNotFound,
        Err(pkg::ResolveError::PackageNotFound) => EventCodes::PackageNotFound,
        Err(pkg::ResolveError::RepoNotFound) => EventCodes::RepoNotFound,
        Err(pkg::ResolveError::NoSpace) => EventCodes::NoSpace,
        Err(pkg::ResolveError::UnavailableBlob) => EventCodes::UnavailableBlob,
        Err(pkg::ResolveError::UnavailableRepoMetadata) => EventCodes::UnavailableRepoMetadata,
        Err(pkg::ResolveError::InvalidUrl) => EventCodes::InvalidUrl,
        Err(pkg::ResolveError::InvalidContext) => EventCodes::InvalidContext,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;

    #[test]
    fn test_hash_from_cache_packages_manifest() {
        let hash =
            "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap();
        let cache_packages = CachePackages::from_entries(vec![
            PinnedAbsolutePackageUrl::new_with_path(
                "fuchsia-pkg://fuchsia.com".parse().unwrap(),
                "potato",
                hash,
            )
            .unwrap(),
            PinnedAbsolutePackageUrl::new_with_path(
                "fuchsia-pkg://other.com".parse().unwrap(),
                "potato",
                hash,
            )
            .unwrap(),
        ]);
        let empty_cache_packages = CachePackages::from_entries(vec![]);

        let fuchsia_url = AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/potato").unwrap();
        let variant_nonzero_fuchsia_url =
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/potato/1").unwrap();
        let variant_zero_fuchsia_url =
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/potato/0").unwrap();
        let other_repo_url = AbsolutePackageUrl::parse("fuchsia-pkg://other.com/potato").unwrap();
        assert_eq!(
            hash_from_cache_packages_manifest(&fuchsia_url, &cache_packages),
            Some(hash.into())
        );
        assert_eq!(
            hash_from_cache_packages_manifest(&variant_zero_fuchsia_url, &cache_packages),
            Some(hash.into())
        );
        assert_eq!(
            hash_from_cache_packages_manifest(&variant_nonzero_fuchsia_url, &cache_packages),
            None
        );
        assert_eq!(
            hash_from_cache_packages_manifest(&other_repo_url, &cache_packages),
            Some(hash.into())
        );
        assert_eq!(hash_from_cache_packages_manifest(&fuchsia_url, &empty_cache_packages), None);
    }

    #[test]
    fn test_hash_from_cache_packages_manifest_with_zero_variant() {
        let hash =
            "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap();
        let cache_packages = CachePackages::from_entries(vec![PinnedAbsolutePackageUrl::new(
            "fuchsia-pkg://fuchsia.com".parse().unwrap(),
            "potato".parse().unwrap(),
            Some(fuchsia_url::PackageVariant::zero()),
            hash,
        )]);
        let empty_cache_packages = CachePackages::from_entries(vec![]);

        let fuchsia_url = AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/potato").unwrap();
        let variant_nonzero_fuchsia_url =
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/potato/1").unwrap();
        let variant_zero_fuchsia_url =
            AbsolutePackageUrl::parse("fuchsia-pkg://fuchsia.com/potato/0").unwrap();
        let other_repo_url = AbsolutePackageUrl::parse("fuchsia-pkg://nope.com/potato/0").unwrap();
        // hash_from_cache_packages_manifest removes variant from URL provided, and
        // since CachePackages is initialized with a variant and will only resolve url to a hash
        // if the /0 variant is provided.
        assert_eq!(hash_from_cache_packages_manifest(&fuchsia_url, &cache_packages), None);
        assert_eq!(
            hash_from_cache_packages_manifest(&variant_zero_fuchsia_url, &cache_packages),
            None
        );
        assert_eq!(
            hash_from_cache_packages_manifest(&variant_nonzero_fuchsia_url, &cache_packages),
            None
        );
        assert_eq!(hash_from_cache_packages_manifest(&other_repo_url, &cache_packages), None);
        assert_eq!(hash_from_cache_packages_manifest(&fuchsia_url, &empty_cache_packages), None);
    }
}
