// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Access utilities for product metadata.
//!
//! This is a collection of helper functions wrapping the FMS and GCS libs.
//!
//! The metadata can be loaded from a variety of sources. The initial places are
//! GCS and the local build.
//!
//! Call `product_bundle_urls()` to get a set of URLs for each product bundle.
//!
//! Call `fms_entries_from()` to get FMS entries from a particular repo. The
//! entries include product bundle metadata, physical device specifications, and
//! virtual device specifications. Each FMS entry has a unique name to identify
//! that entry.
//!
//! These FMS entry names are suitable to present to the user. E.g. the name of
//! a product bundle is also the name of the product bundle metadata entry.

use crate::gcs::string_from_gcs;
use crate::pbms::{GS_SCHEME, path_from_file_url};
use ::gcs::client::{Client, FileProgress, ProgressResult};
use ::gcs::error::GcsError;
use camino::{Utf8Path, Utf8PathBuf};
use ffx_config::EnvironmentContext;
use http_body_util::BodyExt as _;
use hyper::{Method, Request};
use std::path::{Path, PathBuf};
use std::str::FromStr;

// Re-export for convenience.
pub use product_bundle::{LoadedProductBundle, ProductBundle};

mod gcs;
mod pbms;
mod transfer_manifest;

pub use crate::gcs::{handle_new_access_token, list_from_gcs};
pub use crate::transfer_manifest::transfer_download;

/// Select an Oauth2 authorization flow.
#[derive(PartialEq, Debug, Clone)]
pub enum AuthFlowChoice {
    /// Fail rather than using authentication.
    NoAuth,
    Default,
    Device,
    Exec(PathBuf),
    Pkce,
    /// Authenticate using `gcloud auth print-access-token`.
    /// This natively supports headless environments (e.g., when a
    /// developer is connected via SSH).
    Gcloud,
}

const PRODUCT_BUNDLE_PATH_KEY: &str = "product.path";

#[derive(Debug, thiserror::Error)]
pub enum PbmsError {
    #[error("No product bundle path configured, nor specified.")]
    NoPathConfigured,

    #[error("Could not find product bundle in {0}")]
    NotFound(String),

    #[error("Could not find product bundle in {0} nor {1}")]
    NotFoundAtPaths(String, String),

    #[error("Failed to load product bundle: {0}")]
    Load(#[from] product_bundle::ProductBundleLoadError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Url parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Hyper error: {0}")]
    Hyper(#[from] hyper::Error),

    #[error("Hyper client error: {0}")]
    HyperClient(#[from] hyper_util::client::legacy::Error),

    #[error("Http error failed with status {0} for {1}")]
    HttpFailed(hyper::StatusCode, String),

    #[error("Unexpected URI scheme in {0}")]
    UnexpectedScheme(String),

    #[error("GCS error: {0}")]
    Gcs(#[from] GcsError),

    #[error(
        "The output directory is {0:?} which looks like a mistake. Please try a different output directory path."
    )]
    UnsafeOutputDir(PathBuf),

    #[error(
        "The directory does not resemble an old product bundle. For caution's sake, please remove the output directory {0:?} by hand and try again."
    )]
    NotAProductBundle(PathBuf),

    #[error(
        "The output directory already exists. Please provide another directory to write to, or use --force to overwrite the contents of {0:?}."
    )]
    OutputDirAlreadyExists(PathBuf),

    #[error("Cannot safely concat {0} onto {1}")]
    UnsafeConcat(String, String),

    #[error("Failed to parse JSON: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("Invalid GS URL: {0}")]
    InvalidGsUrl(String),

    #[error("Path has no parent: {0}")]
    NoParent(PathBuf),

    #[error("Missing name in product URI")]
    MissingNameInUri,

    #[error("Downloading directory from web server is not implemented")]
    WebDirNotSupported,

    #[error("Invalid URI: {0}")]
    InvalidUri(#[from] hyper::http::uri::InvalidUri),

    #[error("Missing content length header")]
    MissingContentLength,

    #[error("Invalid content length: {0}")]
    InvalidContentLength(String),

    #[error("Failed to render progress: {0}")]
    RenderProgress(#[source] anyhow::Error),

    #[error("HTTP request builder failed: {0}")]
    HttpBuilder(#[from] hyper::http::Error),

    #[error("Invalid file URL: {0}")]
    InvalidFileUrl(String),

    #[error("GCS operation failed: {0}")]
    GcsOperation(#[source] anyhow::Error),

    #[error("Failed to get new access token")]
    GetAccessTokenFailed,

    #[error("Refresh token not supported for this auth flow")]
    RefreshTokenNotSupported,

    #[error("Failed to save credentials: {0}")]
    SaveCredentials(#[source] anyhow::Error),

    #[error("Failed to get refresh token: {0}")]
    GetRefreshToken(#[source] anyhow::Error),

    #[error("Progress cancelled by user")]
    ProgressCancelled,
}

pub fn get_product_bundle_path(context: &EnvironmentContext) -> Result<String, PbmsError> {
    Ok(context.get(PRODUCT_BUNDLE_PATH_KEY).unwrap_or_default())
}

/// Convert CLI arg or config strings to AuthFlowChoice.
impl FromStr for AuthFlowChoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.as_ref() {
            "no-auth" => Ok(AuthFlowChoice::NoAuth),
            "default" => Ok(AuthFlowChoice::Default),
            "device-experimental" => Ok(AuthFlowChoice::Device),
            "pkce" => Ok(AuthFlowChoice::Pkce),
            "gcloud" => Ok(AuthFlowChoice::Gcloud),
            exec => {
                let path = Path::new(exec);
                if path.is_file() {
                    Ok(AuthFlowChoice::Exec(path.to_path_buf()))
                } else {
                    Err("Unknown auth flow choice. Use one of \
                        device-experimental, pkce, default, gcloud, a path to an \
                        executable which prints an access token to stdout, or \
                        no-auth to enforce that no auth flow will be used."
                        .to_string())
                }
            }
        }
    }
}

pub fn is_local_product_bundle<P: AsRef<Path>>(product_bundle: P) -> bool {
    product_bundle.as_ref().exists()
}

/// Load a product bundle by name, uri, or local path.
///
/// If a build config value is set for product.path and the product bundle
/// is None, this method will use the value that is set in the build config as
/// the path.
/// If the path is absolute it will be used as is.
/// otherwise the relative path is checked to exist, and if it does not,
/// it is joined to the build_dir. This way, command line references to relative
/// paths work as developer's expect, and still maintain the legacy behavior.
pub async fn load_product_bundle(
    env: &EnvironmentContext,
    product_bundle: impl AsRef<Utf8Path>,
) -> Result<LoadedProductBundle, PbmsError> {
    // Can't use unwrap_or_else here since ffx config get is async.
    let bundle_path: &Utf8Path = product_bundle.as_ref();
    if bundle_path.as_std_path() == Path::new("") {
        return Err(PbmsError::NoPathConfigured);
    }

    log::debug!("Loading a product bundle: {:?}", bundle_path);

    if is_local_product_bundle(&bundle_path) {
        return Ok(LoadedProductBundle::try_load_from(&bundle_path)?);
    } else if bundle_path.is_relative() {
        if let Some(base_path) = env.build_dir().map(Utf8Path::from_path).flatten() {
            let base_dir_based_path: Utf8PathBuf = base_path.join(&bundle_path);

            if is_local_product_bundle(&base_dir_based_path) {
                return Ok(LoadedProductBundle::try_load_from(&base_dir_based_path)?);
            } else {
                return Err(PbmsError::NotFoundAtPaths(
                    bundle_path.to_string(),
                    base_dir_based_path.to_string(),
                ));
            }
        }
    }
    return Err(PbmsError::NotFound(bundle_path.to_string()));
}

/// Remove prior output directory, if necessary.
pub async fn make_way_for_output(local_dir: &Path, force: bool) -> Result<(), PbmsError> {
    log::debug!("make_way_for_output {:?}, force {}", local_dir, force);
    if local_dir.exists() {
        log::debug!("local_dir.exists {:?}", local_dir);
        if std::fs::read_dir(&local_dir).expect("reading dir").next().is_none() {
            log::debug!("local_dir is empty (which is good) {:?}", local_dir);
            return Ok(());
        } else if force {
            if local_dir == Path::new("") || local_dir == Path::new("/") {
                return Err(PbmsError::UnsafeOutputDir(local_dir.to_path_buf()));
            }
            if !local_dir.join("product_bundle.json").exists() {
                return Err(PbmsError::NotAProductBundle(local_dir.to_path_buf()));
            }
            async_fs::remove_dir_all(&local_dir).await?;
            log::debug!("Removed all of {:?}", local_dir);
        } else {
            return Err(PbmsError::OutputDirAlreadyExists(local_dir.to_path_buf()));
        }
    }
    log::debug!("local_dir dir clear.");
    Ok(())
}

/// Download data from any of the supported schemes listed in RFC-100, Product
/// Bundle, "bundle_uri" to a string.
///
/// Currently: "pattern": "^(?:http|https|gs|file):\/\/"
///
/// Note: If the contents are large or more than a single file is expected,
/// consider using fetch_from_url to write to a file instead.
pub async fn string_from_url<F, I>(
    product_url: &url::Url,
    auth_flow: &AuthFlowChoice,
    progress: &F,
    ui: &I,
    client: &Client,
) -> Result<String, PbmsError>
where
    F: Fn(FileProgress<'_>) -> ProgressResult,
    I: structured_ui::Interface,
{
    log::debug!("string_from_url {}", product_url);
    Ok(match product_url.scheme() {
        "http" | "https" => {
            let https_client = fuchsia_hyper::new_https_client();
            let req = Request::builder()
                .method(Method::GET)
                .uri(product_url.as_str())
                .body(http_body_util::Full::<hyper::body::Bytes>::new("".into()))?;
            let res = https_client.request(req).await?;
            if !res.status().is_success() {
                return Err(PbmsError::HttpFailed(res.status(), product_url.to_string()));
            }
            let bytes = res.into_body().collect().await?.to_bytes();
            String::from_utf8_lossy(&bytes).to_string()
        }
        GS_SCHEME => string_from_gcs(product_url.as_str(), auth_flow, progress, ui, client).await?,
        "file" => {
            if let Some(file_path) = &path_from_file_url(product_url) {
                std::fs::read_to_string(file_path).map_err(|e| PbmsError::Io(e))?
            } else {
                return Err(PbmsError::InvalidFileUrl(product_url.to_string()));
            }
        }
        _ => return Err(PbmsError::UnexpectedScheme(product_url.scheme().to_string())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_config::environment::test_env;
    use std::fs::File;
    use tempfile::TempDir;

    #[fuchsia::test]
    async fn test_load_product_bundle_intree_errors() {
        let test_dir = TempDir::new().expect("output directory");
        let build_dir =
            Utf8Path::from_path(test_dir.path()).expect("cannot convert builddir to Utf8Path");
        let env = test_env().in_tree(&test_dir.path()).build().unwrap();

        // If pb provided but invalid path return None
        let pb_path = build_dir.join("__invalid__").to_string();
        let pb = load_product_bundle(&env.context, &pb_path.clone()).await;
        assert!(matches!(pb.err().unwrap(), PbmsError::NotFound(p) if p == pb_path));

        // If pb provided and absolute and valid return Some(abspath)
        let pb = load_product_bundle(&env.context, build_dir).await;
        assert!(matches!(
            pb.err().unwrap(),
            PbmsError::Load(product_bundle::ProductBundleLoadError::OpenFile(_, _))
        ));

        // If pb provided, relative and but to a file not a directory.
        let relpath = "foo".to_string();
        std::fs::File::create(build_dir.join(relpath.clone())).expect("create relative dir");
        let pb = load_product_bundle(&env.context, &relpath.clone()).await;
        assert!(matches!(
            pb.err().unwrap(),
            PbmsError::Load(product_bundle::ProductBundleLoadError::NotADirectory(_))
        ));

        // If pb provided and relative and invalid return None
        let pb = load_product_bundle(&env.context, &"invalid".to_string()).await;
        assert!(matches!(pb.err().unwrap(), PbmsError::NotFoundAtPaths(_, _)));
    }

    #[fuchsia::test]
    async fn test_load_product_bundle_no_build_dir() {
        let env = ffx_config::test_init().unwrap();

        // Can handle an empty build path
        let pb = load_product_bundle(&env.context, "some_place".to_string()).await;
        assert!(matches!(pb.err().unwrap(), PbmsError::NotFound(p) if p == "some_place"));
    }

    #[test]
    fn test_is_local_product_bundle() {
        let temp_dir = TempDir::new().expect("temp dir");
        let temp_path = temp_dir.path();

        assert!(is_local_product_bundle(temp_path.as_os_str().to_str().unwrap()));
        assert!(!is_local_product_bundle("gs://fuchsia/test_fake.tgz"));
    }

    #[fuchsia::test]
    async fn test_make_way_for_output() {
        let test_dir = tempfile::TempDir::new().expect("temp dir");

        make_way_for_output(&test_dir.path(), /*force=*/ false).await.expect("empty dir is okay");

        std::fs::create_dir(&test_dir.path().join("foo")).expect("make_dir foo");
        std::fs::File::create(test_dir.path().join("info")).expect("create info");
        std::fs::File::create(test_dir.path().join("product_bundle.json"))
            .expect("create product_bundle.json");
        make_way_for_output(&test_dir.path(), /*force=*/ true).await.expect("rm dir is okay");

        let test_dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::create_dir(&test_dir.path().join("foo")).expect("make_dir foo");
        assert!(make_way_for_output(&test_dir.path(), /*force=*/ false).await.is_err());
    }

    macro_rules! make_pb_v2_in {
        ($dir:expr,$name:expr)=>{
            {
                let pb_dir = Utf8Path::from_path($dir.path()).unwrap();
                let pb_file = File::create(pb_dir.join("product_bundle.json")).unwrap();
                serde_json::to_writer(
                    &pb_file,
                    &serde_json::json!({
                        "version": "2",
                        "product_name": $name,
                        "product_version": "version",
                        "sdk_version": "sdk-version",
                        "partitions": {
                            "hardware_revision": "board",
                            "bootstrap_partitions": [],
                            "bootloader_partitions": [],
                            "partitions": [],
                            "unlock_credentials": [],
                        },
                    }),
                )
                .unwrap();
                pb_dir
            }
        }
    }

    #[fuchsia::test]
    async fn test_load_product_bundle_v2_valid() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = make_pb_v2_in!(tmp, "fake.x64");
        let env = ffx_config::test_init().expect("create test config");

        // Load with passing a path directly
        let pb =
            load_product_bundle(&env.context, pb_dir).await.expect("could not load product bundle");
        assert_eq!(pb.loaded_from_path(), pb_dir);
    }

    #[fuchsia::test]
    async fn test_load_product_bundle_v2_invalid() {
        let tmp = TempDir::new().unwrap();
        let pb_dir = Utf8Path::from_path(tmp.path()).unwrap();
        let env = ffx_config::test_init().expect("create test config");

        // Load with passing a path directly
        let pb = load_product_bundle(&env.context, pb_dir).await;
        assert!(pb.is_err());
    }
}
