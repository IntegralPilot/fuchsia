// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cache::ToResolveError as _;
use anyhow::Context as _;
use fidl_fuchsia_pkg_ext as pkg;
use fidl_fuchsia_pkg_ext::ResolveError;
use fidl_fuchsia_pkg_internal as fpkg_internal;
use fuchsia_trace as ftrace;
use futures::TryStreamExt as _;

pub(crate) async fn serve(
    stream: fpkg_internal::OtaDownloaderRequestStream,
    blob_fetcher: crate::cache::BlobFetcher,
    pkg_cache: pkg::cache::Client,
) -> anyhow::Result<()> {
    let write_blobs = pkg_cache.write_blobs().context("pkg-cache write_blobs")?;
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, async |request| {
            let fpkg_internal::OtaDownloaderRequest::FetchBlob {
                hash,
                base_url,
                overwrite_existing,
                responder,
            } = request;
            let () = responder.send(
                fetch_blob(
                    &blob_fetcher,
                    write_blobs.clone(),
                    hash.into(),
                    base_url,
                    overwrite_existing,
                )
                .await
                .map_err(|(anyhow_error, resolve_error)| {
                    log::error!("OtaDownloader: fetch_blob failed: {anyhow_error:#}");
                    resolve_error.into()
                }),
            )?;
            Ok(())
        })
        .await
}

async fn fetch_blob(
    blob_fetcher: &crate::cache::BlobFetcher,
    mut write_blobs: pkg::cache::WriteBlobs,
    hash: pkg::BlobId,
    base_url: String,
    overwrite_existing: bool,
) -> Result<u64, (anyhow::Error, ResolveError)> {
    let base_url = base_url
        .parse::<http::Uri>()
        .with_context(|| format!("parsing url {base_url:?}"))
        .map_err(|e| (e, ResolveError::InvalidUrl))?;
    let context = crate::cache::FetchBlobContext::new(
        write_blobs.make_open_blob(hash, overwrite_existing),
        base_url,
        ftrace::Id::new(),
    );
    let blob_size =
        blob_fetcher.push(hash, context).await.expect("processor exists").map_err(|e| {
            let resolve_error = e.to_resolve_error();
            (e.into(), resolve_error)
        })?;
    Ok(blob_size)
}
