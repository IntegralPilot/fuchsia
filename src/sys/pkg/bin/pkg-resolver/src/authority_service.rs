// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::repository_manager::RepositoryManager;
use crate::rewrite_manager::RewriteManager;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_ext as fpkg_ext;
use futures::stream::TryStreamExt as _;
use std::sync::Arc;

pub async fn serve(
    stream: fpkg::AuthorityRequestStream,
    rewriter: Arc<async_lock::RwLock<RewriteManager>>,
    repo_manager: Arc<async_lock::RwLock<RepositoryManager>>,
) -> Result<(), anyhow::Error> {
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, async |request| {
            match request {
                fpkg::AuthorityRequest::Lookup { package_url, responder } => {
                    match lookup(&package_url.url, &rewriter, &repo_manager).await {
                        Ok((hash, url)) => {
                            let () = responder.send(Ok((&hash.into(), &url.to_string())))?;
                        }
                        Err(e) => {
                            let fidl_err = (&e).into();
                            log::warn!(url:% = &package_url.url; "failed lookup: {e:#}");
                            let () = responder.send(Err(fidl_err))?;
                        }
                    }
                }
            }
            Ok(())
        })
        .await
}

async fn lookup(
    url: &str,
    rewriter: &async_lock::RwLock<RewriteManager>,
    repo_manager: &async_lock::RwLock<RepositoryManager>,
) -> Result<(fpkg_ext::BlobId, http::Uri), LookupError> {
    let url: fuchsia_url::FuchsiaPkgAbsolutePackageUrl =
        url.parse().map_err(LookupError::ParseUrl)?;
    if url.hash().is_some() {
        return Err(LookupError::PinnedUrl);
    }
    let url = rewriter.read().await.rewrite(&url);
    repo_manager.read().await.get_package_hash(&url).await.map_err(LookupError::GetPackageHash)
}

#[derive(thiserror::Error, Debug)]
enum LookupError {
    #[error("parse url")]
    ParseUrl(#[source] fuchsia_url::ParseError),

    #[error("pinned urls not allowed")]
    PinnedUrl,

    #[error("get package hash")]
    GetPackageHash(#[source] crate::repository_manager::GetPackageHashError),
}

impl From<&LookupError> for fpkg::AuthorityLookupError {
    fn from(other: &LookupError) -> Self {
        use LookupError::*;
        use fpkg::AuthorityLookupError as Err;
        match other {
            ParseUrl { .. } => Err::InvalidUrl,
            PinnedUrl => Err::PinnedUrlNotAllowed,
            GetPackageHash(e) => {
                use crate::repository_manager::GetPackageHashError::*;
                match e {
                    RepoNotFound { .. } => Err::RepositoryNotFound,
                    OpenRepo { .. } => Err::UpstreamConnection,
                    MerkleFor(e) => {
                        use crate::cache::MerkleForError::*;
                        match e {
                            TargetNotFound { .. } => Err::PackageNotFound,
                            MetadataNotFound { .. }
                            | FetchTargetDescription { .. }
                            | InvalidTargetPath { .. }
                            | NoCustomMetadata
                            | SerdeError { .. } => Err::UpstreamConnection,
                        }
                    }
                    NoMirrors { .. } => Err::UpstreamConnection,
                    BlobUrl { .. } => Err::UpstreamConnection,
                }
            }
        }
    }
}
