// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{QueuedResolver, ResolverServiceInspectState};
use crate::eager_package_manager::EagerPackageManager;
use anyhow::anyhow;
use fuchsia_url::fuchsia_pkg::PackageUrl;
use log::error;
use {
    fidl_fuchsia_io as fio, fidl_fuchsia_metrics as fmetrics, fidl_fuchsia_pkg as fpkg,
    fidl_fuchsia_pkg_ext as pkg,
};

pub(super) async fn resolve_with_context(
    package_url: String,
    context: fpkg::ResolutionContext,
    gc_protection: fpkg::GcProtection,
    dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    package_resolver: &QueuedResolver,
    pkg_cache: &pkg::cache::Client,
    eager_package_manager: Option<&async_lock::RwLock<EagerPackageManager<QueuedResolver>>>,
    cobalt_sender: fidl_contrib::protocol_connector::ProtocolSender<fmetrics::MetricEvent>,
    inspect: &ResolverServiceInspectState,
) -> Result<fpkg::ResolutionContext, pkg::ResolveError> {
    match PackageUrl::parse(&package_url)
        .map_err(|e| super::handle_bad_package_url_error(e, &package_url))?
    {
        PackageUrl::Absolute(url) => {
            if !context.bytes.is_empty() {
                error!(
                    "ResolveWithContext context must be empty if url is absolute {} {:?}",
                    package_url, context,
                );
                return Err(pkg::ResolveError::InvalidContext);
            }
            super::resolve_absolute_url_and_send_cobalt_metrics(
                url,
                gc_protection,
                dir,
                package_resolver,
                eager_package_manager,
                cobalt_sender,
            )
            .await
        }
        PackageUrl::Relative(url) => {
            resolve_relative(&url, &context, dir, pkg_cache, inspect).await
        }
    }
}

async fn resolve_relative(
    url: &fuchsia_url::RelativePackageUrl,
    context: &fpkg::ResolutionContext,
    dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    pkg_cache: &pkg::cache::Client,
    inspect: &ResolverServiceInspectState,
) -> Result<fpkg::ResolutionContext, pkg::ResolveError> {
    let start_ts = zx::BootInstant::get();
    let context = pkg::ResolutionContext::try_from(context).map_err(|e| {
        error!("failed to parse relative url {} context {:?}: {:#}", url, context, anyhow!(e));
        pkg::ResolveError::InvalidContext
    })?;

    let child_context =
        resolve_relative_impl(url, &context, dir, pkg_cache).await.map_err(|e| {
            let fidl_err = e.to_fidl_err();
            error!(
                "failed to resolve relative url {} with parent {:?} {:#}",
                url,
                context.blob_id(),
                anyhow!(e)
            );
            fidl_err
        })?;

    inspect.successful_subpackage_resolve(url, context.blob_id(), start_ts);

    Ok(child_context.into())
}

async fn resolve_relative_impl(
    url: &fuchsia_url::RelativePackageUrl,
    context: &pkg::ResolutionContext,
    dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    pkg_cache: &pkg::cache::Client,
) -> Result<pkg::ResolutionContext, ResolveWithContextError> {
    let super_blob = if let Some(blob) = context.blob_id() {
        blob
    } else {
        return Err(ResolveWithContextError::EmptyContext);
    };
    let subpackage = pkg_cache
        .get_subpackage(*super_blob, url)
        .await
        .map_err(ResolveWithContextError::GetSubpackage)?;
    let hash =
        subpackage.merkle_root().await.map_err(ResolveWithContextError::ReadSubpackageHash)?;
    let () = subpackage.reopen(dir).map_err(ResolveWithContextError::Reopen)?;
    Ok(hash.into())
}

#[derive(thiserror::Error, Debug)]
enum ResolveWithContextError {
    #[error("invalid context")]
    InvalidContext(#[from] pkg::ResolutionContextError),

    #[error("resolving a relative url requires a populated resolution context")]
    EmptyContext,

    #[error("reading the subpackage hash")]
    ReadSubpackageHash(#[source] fuchsia_pkg::ReadHashError),

    #[error("error getting subpackage")]
    GetSubpackage(#[source] pkg::cache::GetSubpackageError),

    #[error("reopening subpackage onto the request handle")]
    Reopen(#[source] fuchsia_pkg::package_directory::CloneError),
}

impl ResolveWithContextError {
    fn to_fidl_err(&self) -> pkg::ResolveError {
        use ResolveWithContextError::*;
        use pkg::ResolveError as pErr;
        match self {
            GetSubpackage(pkg::cache::GetSubpackageError::DoesNotExist) => pErr::PackageNotFound,
            InvalidContext(_) | EmptyContext => pErr::InvalidContext,
            ReadSubpackageHash(_) | Reopen(_) | GetSubpackage(_) => pErr::Internal,
        }
    }
}
