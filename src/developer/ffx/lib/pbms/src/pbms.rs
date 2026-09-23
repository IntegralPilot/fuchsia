// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Private functionality for pbms lib.

use crate::AuthFlowChoice;
use crate::gcs::fetch_from_gcs;
use crate::transfer_manifest::safe_join;
use ::gcs::client::{
    Client, DirectoryProgress, FileProgress, ProgressResponse, ProgressResult, Throttle,
};

use async_fs::File;
use futures::{AsyncWriteExt as _, StreamExt as _, TryStreamExt as _};
use hyper::StatusCode;
use hyper::header::CONTENT_LENGTH;
use std::path::{Path, PathBuf};

pub(crate) const GS_SCHEME: &str = "gs";

/// Retrieve the path portion of a "file:/" url. Non-file-paths return None.
///
/// If the url has no scheme, the whole string is returned.
/// E.g.
/// - "/foo/bar" -> Some("/foo/bar")
/// - "file:///foo/bar" -> Some("/foo/bar")
/// - "http://foo/bar" -> None
pub(crate) fn path_from_file_url(product_url: &url::Url) -> Option<PathBuf> {
    if product_url.scheme() == "file" { product_url.to_file_path().ok() } else { None }
}

/// Download data from any of the supported schemes listed in RFC-100, Product
/// Bundle, "bundle_uri".
///
/// Currently: "pattern": "^(?:http|https|gs|file):\/\/"
pub(crate) async fn fetch_from_url<F, I>(
    product_url: &url::Url,
    local_dir: PathBuf,
    auth_flow: &AuthFlowChoice,
    progress: &F,
    ui: &I,
    client: &Client,
) -> std::result::Result<(), crate::PbmsError>
where
    F: Fn(DirectoryProgress<'_>, FileProgress<'_>) -> ProgressResult,
    I: structured_ui::Interface,
{
    log::debug!("fetch_from_url {:?}", product_url);
    if product_url.scheme() == GS_SCHEME {
        fetch_from_gcs(product_url.as_str(), &local_dir, auth_flow, progress, ui, client).await?;
    } else if product_url.scheme() == "http" || product_url.scheme() == "https" {
        fetch_from_web(product_url, &local_dir, progress, ui).await?;
    } else if let Some(_) = &path_from_file_url(product_url) {
        // Since the file is already local, no fetch is necessary.
        log::debug!("Found local file path {:?}", product_url);
    } else {
        return Err(crate::PbmsError::UnexpectedScheme(product_url.scheme().to_string()));
    }
    Ok(())
}

async fn fetch_from_web<F, I>(
    product_uri: &url::Url,
    local_dir: &Path,
    progress: &F,
    _ui: &I,
) -> std::result::Result<(), crate::PbmsError>
where
    F: Fn(DirectoryProgress<'_>, FileProgress<'_>) -> ProgressResult,
    I: structured_ui::Interface,
{
    log::debug!("fetch_from_web");
    let name = if let Some((_, name)) = product_uri.path().rsplit_once('/') {
        name
    } else {
        return Err(crate::PbmsError::MissingNameInUri);
    };

    if name.is_empty() {
        return Err(crate::PbmsError::WebDirNotSupported);
    }

    let path = safe_join(local_dir, Path::new(name))?;

    let res = fuchsia_hyper::new_client()
        .get(hyper::Uri::from_maybe_shared(product_uri.to_string())?)
        .await?;

    match res.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND => {
            return Err(crate::PbmsError::HttpFailed(
                StatusCode::NOT_FOUND,
                product_uri.to_string(),
            ));
        }
        status => {
            return Err(crate::PbmsError::HttpFailed(status, product_uri.to_string()));
        }
    }

    let mut at: u64 = 0;
    let length = if res.headers().contains_key(CONTENT_LENGTH) {
        res.headers()
            .get(CONTENT_LENGTH)
            .ok_or_else(|| crate::PbmsError::MissingContentLength)?
            .to_str()
            .map_err(|e| crate::PbmsError::InvalidContentLength(e.to_string()))?
            .parse::<u64>()
            .map_err(|e| crate::PbmsError::InvalidContentLength(e.to_string()))?
    } else {
        0
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    } else {
        std::fs::create_dir_all(local_dir)?;
    }

    let mut file = File::create(&path).await?;

    let mut stream = std::pin::pin!(http_body_util::BodyStream::new(res.into_body()).filter_map(
        |res| async move {
            match res {
                Ok(frame) => frame.into_data().ok().map(Ok),
                Err(e) => Some(Err(e)),
            }
        }
    ));

    let mut of = length;
    // Throttle the progress UI updates to avoid burning CPU on changes
    // the user will have trouble seeing anyway. Without throttling,
    // around 20% of the execution time can be spent updating the
    // progress UI. The throttle makes the overhead negligible.
    let mut throttle = Throttle::from_duration(std::time::Duration::from_millis(500));
    let url = product_uri.to_string();
    while let Some(chunk) = stream.try_next().await? {
        file.write_all(&chunk).await?;
        at += chunk.len() as u64;
        if at > of {
            of = at;
        }
        if throttle.is_ready() {
            match progress(
                DirectoryProgress { name: &url, at: 0, of: 1, units: "files" },
                FileProgress { name: &url, at, of, units: "bytes" },
            )
            .map_err(crate::PbmsError::RenderProgress)?
            {
                ProgressResponse::Cancel => break,
                _ => (),
            }
        }
    }

    file.close().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_path_from_file_url() {
        let input = url::Url::parse("fake://foo#bar").expect("url");
        let output = path_from_file_url(&input);
        assert!(output.is_none());

        let input = url::Url::parse("file:///../../foo#bar").expect("url");
        let output = path_from_file_url(&input);
        assert_eq!(output, Some(Path::new("/foo").to_path_buf()));

        let input = url::Url::parse("file://foo#bar").expect("url");
        let output = path_from_file_url(&input);
        assert!(output.is_none());

        let input = url::Url::parse("file:///foo#bar").expect("url");
        let output = path_from_file_url(&input);
        assert_eq!(output, Some(Path::new("/foo").to_path_buf()));

        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let base_url = url::Url::from_directory_path(temp_dir.path().join("a/b/c/d")).expect("url");
        let input =
            url::Url::options().base_url(Some(&base_url)).parse("../../foo#bar").expect("url");
        let output = path_from_file_url(&input);
        assert_eq!(output, Some(temp_dir.path().join("a/b/foo").to_path_buf()));
    }

    #[fuchsia::test]
    async fn test_fetch_from_url() {
        let url = url::Url::parse("fake://foo").expect("url");
        let ui = structured_ui::MockUi::new();
        let client = Client::initial().expect("creating client");
        let result = fetch_from_url(
            &url,
            Path::new("unused").to_path_buf(),
            &AuthFlowChoice::Default,
            &|_d, _f| Ok(ProgressResponse::Continue),
            &ui,
            &client,
        )
        .await;
        assert!(
            matches!(result.unwrap_err(), crate::PbmsError::UnexpectedScheme(s) if s == "fake")
        );
    }

    #[fuchsia::test]
    async fn test_fetch_from_url_web_unsafe_path() {
        let ui = structured_ui::MockUi::new();
        let client = Client::initial().expect("creating client");

        let url = url::Url::parse("http://example.com/").expect("url");
        let result = fetch_from_url(
            &url,
            Path::new("unused").to_path_buf(),
            &AuthFlowChoice::Default,
            &|_d, _f| Ok(ProgressResponse::Continue),
            &ui,
            &client,
        )
        .await;
        assert!(matches!(result.unwrap_err(), crate::PbmsError::WebDirNotSupported));

        #[cfg(target_os = "windows")]
        {
            let url = url::Url::parse("http://example.com/C:").expect("url");
            let result = fetch_from_url(
                &url,
                Path::new("unused").to_path_buf(),
                &AuthFlowChoice::Default,
                &|_d, _f| Ok(ProgressResponse::Continue),
                &ui,
                &client,
            )
            .await;
            assert!(matches!(result.unwrap_err(), crate::PbmsError::UnsafeConcat(..)));
        }
    }
}
