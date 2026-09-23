// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use futures::prelude::*;
use std::collections::HashMap;
use update_package::{UpdateImagePackage, UpdatePackage};

/// Error encountered while resolving a package.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("fidl error while resolving {1}")]
    Fidl(#[source] fidl::Error, Box<AbsolutePackageUrl>),

    #[error("error while resolving {1}")]
    Error(#[source] fidl_fuchsia_pkg_ext::ResolveError, Box<AbsolutePackageUrl>),
}

/// Resolves the update package given by `url` through the pkg_resolver.
pub(super) async fn resolve_update_package(
    pkg_resolver: &fpkg::PackageResolverProxy,
    url: &AbsolutePackageUrl,
) -> Result<UpdatePackage, ResolveError> {
    let dir = resolve_package(pkg_resolver, url).await?;
    Ok(UpdatePackage::new(dir))
}

/// Resolves each package URL through the package resolver with some concurrency, returning a mapping of the package urls to the resolved image package directories.
pub(super) async fn resolve_image_packages<I>(
    pkg_resolver: &fpkg::PackageResolverProxy,
    urls: I,
    concurrent_package_resolves: usize,
) -> Result<HashMap<AbsolutePackageUrl, UpdateImagePackage>, ResolveError>
where
    I: Iterator<Item = AbsolutePackageUrl>,
{
    stream::iter(urls)
        .map(async |url| {
            let pkg_dir = resolve_package(pkg_resolver, &url).await?;
            Ok((url, UpdateImagePackage::new(pkg_dir)))
        })
        .buffer_unordered(concurrent_package_resolves)
        .try_collect()
        .await
}

pub(super) async fn resolve_package(
    pkg_resolver: &fpkg::PackageResolverProxy,
    url: &AbsolutePackageUrl,
) -> Result<fio::DirectoryProxy, ResolveError> {
    let (dir, dir_server_end) = fidl::endpoints::create_proxy();
    let _: fpkg::ResolutionContext = pkg_resolver
        .resolve(&url.to_string(), dir_server_end)
        .await
        .map_err(|e| ResolveError::Fidl(e, Box::new(url.clone())))?
        .map_err(|raw| ResolveError::Error(raw.into(), Box::new(url.clone())))?;
    Ok(dir)
}
