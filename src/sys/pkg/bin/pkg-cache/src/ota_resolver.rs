// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_ext as fpkg_ext;
use fuchsia_inspect as finspect;
use fuchsia_sync::Mutex;
use fuchsia_url::fuchsia_pkg::{AbsolutePackageUrl, PackageUrl};
use futures::stream::TryStreamExt as _;
use log::{error, warn};
use std::sync::Arc;

// Packages resolved during OTA never need to be executed.
const FLAGS: fio::Flags = fio::PERM_READABLE;
const INSPECT_RECENT_RESOLVE_COUNT: usize = 50;

/// Used only by the system-updater to resolve packages during OTA, and so:
/// * assumes the retained index has been initialized with the to-be-resolved package
/// * to attempt to recover from a partially broken system:
///   * always uses the TUF package authority to resolve the package (e.g. does not short-circuit
///     the package resolve if the pinned URL is found in the base index)
///   * queries `fuchsia.fxfs/BlobCreator.NeedsOverwrite` for every blob (e.g. does not
///     short-circuit the blob write if a blob is readable via `fuchsia.fxfs/BlobReader.GetVmo`)
pub(crate) struct Resolver {
    authority: fpkg::AuthorityProxy,
    package_fetcher: crate::package_fetcher::PackageFetcher,
    authenticator: context_authenticator::ContextAuthenticator,
    root_dir_factory: crate::root_dir::RootDirFactory,
    scope: package_directory::ExecutionScope,

    // Only used for inspect.
    inspect_active: finspect::Node,
    inspect_recent: Mutex<fuchsia_inspect_contrib::nodes::BoundedListNode>,
    request_count: std::sync::atomic::AtomicU64,
    _inspect: finspect::Node,
}

impl Resolver {
    pub(crate) fn new(
        authority: fpkg::AuthorityProxy,
        package_fetcher: crate::package_fetcher::PackageFetcher,
        authenticator: context_authenticator::ContextAuthenticator,
        root_dir_factory: crate::root_dir::RootDirFactory,
        scope: package_directory::ExecutionScope,
        inspect: finspect::Node,
    ) -> Arc<Self> {
        Arc::new(Self {
            authority,
            package_fetcher,
            authenticator,
            root_dir_factory,
            scope,
            inspect_active: inspect.create_child("active"),
            inspect_recent: Mutex::new(fuchsia_inspect_contrib::nodes::BoundedListNode::new(
                inspect.create_child("recent"),
                INSPECT_RECENT_RESOLVE_COUNT,
            )),
            request_count: std::sync::atomic::AtomicU64::new(0),
            _inspect: inspect,
        })
    }

    pub(crate) async fn serve_request_stream(
        self: Arc<Self>,
        stream: fpkg::PackageResolverRequestStream,
    ) -> anyhow::Result<()> {
        stream
            .map_err(anyhow::Error::new)
            .try_for_each_concurrent(None, |req| async {
                match req {
                    fpkg::PackageResolverRequest::Resolve { package_url, dir, responder } => {
                        self.handle_resolve_request(package_url, dir, responder).await
                    }
                    fpkg::PackageResolverRequest::ResolveWithContext {
                        package_url,
                        context,
                        dir,
                        responder,
                    } => {
                        self.handle_resolve_with_context_request(
                            package_url,
                            context,
                            dir,
                            responder,
                        )
                        .await
                    }
                    fpkg::PackageResolverRequest::GetHash { package_url, responder } => {
                        error!(
                            "unsupported fuchsia.pkg/PackageResolver-ota.GetHash called with {:?}",
                            package_url
                        );
                        responder
                            .send(Err(zx::Status::NOT_SUPPORTED.into_raw()))
                            .context("sending fuchsia.pkg/PackageResolver.GetHash response")
                    }
                }
            })
            .await
    }

    async fn handle_resolve_request(
        &self,
        package_url: String,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
        responder: fpkg::PackageResolverResolveResponder,
    ) -> Result<(), anyhow::Error> {
        let inspect = self.inspect_active.create_child(
            self.request_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string(),
        );
        match self.resolve(&package_url, dir, &inspect).await {
            Ok(context) => {
                inspect.record_string("result", "success");
                self.move_inspect_node_to_recent(inspect);
                responder.send(Ok(&context))
            }
            Err(e) => {
                let fidl_error = (&e).into();
                let log_error = format!("{:#}", anyhow!(e));
                inspect.record_string("result", format!("error: {log_error}"));
                self.move_inspect_node_to_recent(inspect);
                error!("ota resolver failed to resolve {package_url}: {log_error}");
                responder.send(Err(fidl_error))
            }
        }
        .context("sending fuchsia.pkg/PackageResolver-ota.Resolve response")
    }

    async fn handle_resolve_with_context_request(
        &self,
        package_url: String,
        context: fpkg::ResolutionContext,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
        responder: fpkg::PackageResolverResolveWithContextResponder,
    ) -> Result<(), anyhow::Error> {
        let inspect = self.inspect_active.create_child(
            self.request_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string(),
        );
        match self.resolve_with_context(&package_url, context, dir, &inspect).await {
            Ok(context) => {
                inspect.record_string("result", "success");
                self.move_inspect_node_to_recent(inspect);
                responder.send(Ok(&context))
            }
            Err(e) => {
                let fidl_error = (&e).into();
                let log_error = format!("{:#}", anyhow!(e));
                inspect.record_string("result", &log_error);
                self.move_inspect_node_to_recent(inspect);
                error!("ota resolver failed to resolve with context {package_url}: {log_error}");
                responder.send(Err(fidl_error))
            }
        }
        .context("sending fuchsia.pkg/PackageResolver-ota.ResolveWithContext response")
    }

    async fn resolve_with_context(
        &self,
        package_url: &str,
        context: fpkg::ResolutionContext,
        dir: ServerEnd<fio::DirectoryMarker>,
        inspect: &finspect::Node,
    ) -> Result<fpkg::ResolutionContext, Error> {
        self.resolve_with_context_impl(
            &PackageUrl::parse(package_url).map_err(Error::InvalidUrl)?,
            context,
            dir,
            inspect,
        )
        .await
    }

    async fn resolve_with_context_impl(
        &self,
        package_url: &PackageUrl,
        context: fpkg::ResolutionContext,
        dir: ServerEnd<fio::DirectoryMarker>,
        inspect: &finspect::Node,
    ) -> Result<fpkg::ResolutionContext, Error> {
        match package_url {
            PackageUrl::Absolute(url) => {
                if !context.bytes.is_empty() {
                    return Err(Error::ContextWithAbsoluteUrl);
                }
                self.resolve_impl(url, dir, inspect).await
            }
            PackageUrl::Relative(url) => self.resolve_subpackage(url, context, dir, inspect).await,
        }
    }

    async fn resolve(
        &self,
        url: &str,
        dir: ServerEnd<fio::DirectoryMarker>,
        inspect: &finspect::Node,
    ) -> Result<fpkg::ResolutionContext, Error> {
        self.resolve_impl(&url.parse().map_err(Error::InvalidUrl)?, dir, inspect).await
    }

    pub(crate) async fn resolve_impl(
        &self,
        url: &AbsolutePackageUrl,
        dir: ServerEnd<fio::DirectoryMarker>,
        inspect: &finspect::Node,
    ) -> Result<fpkg::ResolutionContext, Error> {
        inspect.record_int("start_boot_ns", zx::BootInstant::get().into_nanos());
        let _end_inspect = scopeguard::guard(&inspect, |inspect| {
            inspect.record_int("end_boot_ns", zx::BootInstant::get().into_nanos());
        });
        inspect.record_string("url", url.to_string());
        let (fpkg::BlobId { merkle_root }, http_blob_dir) = self
            .authority
            .lookup(&fpkg::PackageUrl { url: url.as_unpinned().to_string() })
            .await
            .map_err(Error::AuthorityFidl)?
            .map_err(Error::Authority)?;
        // TODO(https://fxbug.dev/519687989): Stop allowing pinned URLs to override authorities.
        let pkg_id = url.hash().unwrap_or_else(|| merkle_root.into());
        inspect.record_string("hash", pkg_id.to_string());
        inspect.record_string("blob_source", &http_blob_dir);
        let root_dir = self
            .package_fetcher
            .fetch(
                pkg_id,
                http_blob_dir.parse().map_err(Error::InvalidBlobDirUri)?,
                fpkg::GcProtection::Retained,
            )
            .await
            .map_err(Error::PackageFetcher)?;
        let hash = *root_dir.hash();
        vfs::directory::serve_on(root_dir, FLAGS, self.scope.clone(), dir);
        Ok(self.authenticator.clone().create(&hash))
    }

    async fn resolve_subpackage(
        &self,
        url: &fuchsia_url::RelativePackageUrl,
        context: fpkg::ResolutionContext,
        dir: ServerEnd<fio::DirectoryMarker>,
        inspect: &finspect::Node,
    ) -> Result<fpkg::ResolutionContext, Error> {
        inspect.record_int("start_boot_ns", zx::BootInstant::get().into_nanos());
        let _end_inspect = scopeguard::guard(&inspect, |inspect| {
            inspect.record_int("end_boot_ns", zx::BootInstant::get().into_nanos());
        });
        inspect.record_string("url", url.to_string());
        let super_hash = self
            .authenticator
            .clone()
            .authenticate(context)
            .map_err(Error::ContextAuthenticator)?;
        let super_package = self.root_dir_factory.create(super_hash).await.map_err(|source| {
            Error::CreatingSuperpackageRootDir { source, superpackage: super_hash }
        })?;
        let sub_hash = *super_package
            .subpackages()
            .await
            .map_err(Error::ReadingSubpackages)?
            .subpackages()
            .get(url)
            .ok_or_else(|| Error::SubpackageNotFound {
                subpackage: url.clone(),
                superpackage: super_hash,
            })?;
        inspect.record_string("hash", sub_hash.to_string());
        let sub_package =
            self.root_dir_factory.create(sub_hash).await.map_err(|source| {
                Error::CreatingSubpackageRootDir { source, subpackage: sub_hash }
            })?;
        vfs::directory::serve_on(Arc::new(sub_package), FLAGS, self.scope.clone(), dir);
        Ok(self.authenticator.clone().create(&sub_hash))
    }

    fn move_inspect_node_to_recent(&self, node: finspect::Node) {
        self.inspect_recent.lock().add_entry(|parent| {
            let () = parent.adopt(&node).unwrap_or_else(|e| {
                warn!("failed to move inspect node to recent: {:#}", anyhow!(e))
            });
            let () = parent.record(node);
        });
    }
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("invalid url")]
    InvalidUrl(#[source] fuchsia_url::ParseError),

    #[error("absolute package URLs must have an empty context")]
    ContextWithAbsoluteUrl,

    #[error("authority call failed")]
    AuthorityFidl(#[source] fidl::Error),

    #[error("authority lookup failed: {0:?}")]
    Authority(fpkg::AuthorityLookupError),

    #[error("invalid blob dir URI")]
    InvalidBlobDirUri(#[source] http::uri::InvalidUri),

    #[error("forwarding to the package fetcher")]
    PackageFetcher(#[source] Arc<crate::package_fetcher::Error>),

    #[error("authenticating context")]
    ContextAuthenticator(#[source] context_authenticator::ContextAuthenticatorError),

    #[error("creating superpackage root dir")]
    CreatingSuperpackageRootDir {
        #[source]
        source: package_directory::Error,
        superpackage: fuchsia_merkle::Hash,
    },

    #[error("reading subpackage manifest")]
    ReadingSubpackages(#[source] package_directory::SubpackagesError),

    #[error("subpackage {subpackage} of {superpackage} not found")]
    SubpackageNotFound {
        subpackage: fuchsia_url::RelativePackageUrl,
        superpackage: fuchsia_merkle::Hash,
    },

    #[error("creating subpackage root dir")]
    CreatingSubpackageRootDir {
        #[source]
        source: package_directory::Error,
        subpackage: fuchsia_merkle::Hash,
    },
}

impl From<&Error> for fpkg::ResolveError {
    fn from(err: &Error) -> Self {
        use Error::*;
        use fpkg::ResolveError as Err;
        match err {
            InvalidUrl(_) => Err::InvalidUrl,
            ContextWithAbsoluteUrl => Err::InvalidContext,
            AuthorityFidl(_) => Err::Io,
            Authority(e) => fpkg_ext::errors::authority_to_resolve_err(e),
            InvalidBlobDirUri(_) => Err::Internal,
            PackageFetcher(source) => source.as_ref().into(),
            ContextAuthenticator(_) => Err::InvalidContext,
            CreatingSuperpackageRootDir { .. } => Err::Io,
            ReadingSubpackages(_) => Err::Io,
            SubpackageNotFound { .. } => Err::PackageNotFound,
            CreatingSubpackageRootDir { .. } => Err::Io,
        }
    }
}
