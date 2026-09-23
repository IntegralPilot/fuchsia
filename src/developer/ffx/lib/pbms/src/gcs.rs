// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Access utilities for gcs metadata.

use crate::AuthFlowChoice;

use gcs::auth;
use gcs::client::{Client, DirectoryProgress, FileProgress, ProgressResponse, ProgressResult};
use gcs::error::GcsError;
use gcs::gs_url::split_gs_url;
use std::path::Path;

/// Download from a given `gcs_url`.
///
/// `gcs_url` is the full GCS url, e.g. "gs://bucket/path/to/file".
/// The resulting data will be written to a directory at `local_dir`.
pub(crate) async fn fetch_from_gcs<F, I>(
    gcs_url: &str,
    local_dir: &Path,
    auth_flow: &AuthFlowChoice,
    progress: &F,
    ui: &I,
    client: &Client,
) -> Result<(), crate::PbmsError>
where
    F: Fn(DirectoryProgress<'_>, FileProgress<'_>) -> ProgressResult,
    I: structured_ui::Interface,
{
    log::debug!("fetch_from_gcs {:?}", gcs_url);
    let (gcs_bucket, gcs_path) =
        split_gs_url(gcs_url).map_err(|_| crate::PbmsError::InvalidGsUrl(gcs_url.to_string()))?;
    loop {
        log::debug!("gcs_bucket {:?}, gcs_path {:?}", gcs_bucket, gcs_path);
        match client.fetch_all(gcs_bucket, gcs_path, &local_dir, progress).await {
            Ok(()) => break,
            Err(e) => match e.downcast_ref::<GcsError>() {
                Some(GcsError::NeedNewAccessToken) => {
                    log::debug!("fetch_from_gcs got NeedNewAccessToken");
                    let access_token = handle_new_access_token(auth_flow, ui).await?;
                    client.set_access_token(access_token).await;
                }
                Some(GcsError::NotFound(b, p)) => {
                    log::warn!("[gs://{}/{} not found]", b, p);
                    break;
                }
                Some(_) | None => return Err(crate::PbmsError::GcsOperation(e)),
            },
        }
    }
    Ok(())
}

/// Get a new access token based on the AuthFlowChoice.
///
/// Intended to simplify handling of a GcsError::NeedNewAccessToken error.
pub async fn handle_new_access_token<I>(
    auth_flow: &AuthFlowChoice,
    ui: &I,
) -> std::result::Result<String, crate::PbmsError>
where
    I: structured_ui::Interface,
{
    log::debug!("handle_new_access_token");
    let access_token = match auth_flow {
        AuthFlowChoice::Gcloud => {
            let output = std::process::Command::new("gcloud")
                .args(["auth", "print-access-token"])
                .output()
                .map_err(|e| crate::PbmsError::Io(e))?;
            if !output.status.success() {
                log::error!(
                    "The gcloud process to get an access token returned {} with stderr:\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
                return Err(crate::PbmsError::Gcs(GcsError::ExecForAccessFailed(
                    "gcloud".into(),
                    output.status,
                    format!(
                        "{}\nHint: You may need to run `gcloud auth login` to authenticate.",
                        String::from_utf8_lossy(&output.stderr)
                    ),
                )));
            }
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        AuthFlowChoice::Default | AuthFlowChoice::Pkce | AuthFlowChoice::Device => {
            let credentials = credentials::Credentials::load_or_new().await;
            let access_token = match auth::new_access_token(&credentials.gcs_credentials()).await {
                Ok(a) => a,
                Err(GcsError::NeedNewRefreshToken) => {
                    update_refresh_token(auth_flow, ui).await?;
                    // Make one additional attempt now that the refresh token
                    // is updated.
                    let credentials = credentials::Credentials::load_or_new().await;
                    auth::new_access_token(&credentials.gcs_credentials())
                        .await
                        .map_err(|e| crate::PbmsError::Gcs(e))?
                }
                Err(_) => return Err(crate::PbmsError::GetAccessTokenFailed),
            };
            access_token
        }
        AuthFlowChoice::Exec(exec) => {
            let output =
                std::process::Command::new(&exec).output().map_err(|e| crate::PbmsError::Io(e))?;
            if !output.status.success() {
                log::error!(
                    "The {:?} process to get an access token returned {} with stderr:\n{}",
                    exec,
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
                return Err(crate::PbmsError::Gcs(GcsError::ExecForAccessFailed(
                    exec.into(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).to_string(),
                )));
            }
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        AuthFlowChoice::NoAuth => return Err(crate::PbmsError::Gcs(GcsError::AuthRequired)),
    };
    Ok(access_token)
}

/// Download a single file from `gcs_url` to an in-ram string.
///
/// `gcs_url` is the full GCS url, e.g. "gs://bucket/path/to/file".

pub(crate) async fn string_from_gcs<F, I>(
    gcs_url: &str,
    auth_flow: &AuthFlowChoice,
    progress: &F,
    ui: &I,
    client: &Client,
) -> Result<String, crate::PbmsError>
where
    F: Fn(FileProgress<'_>) -> ProgressResult,
    I: structured_ui::Interface,
{
    log::debug!("string_from_gcs {:?}", gcs_url);
    let (gcs_bucket, gcs_path) =
        split_gs_url(gcs_url).map_err(|_| crate::PbmsError::InvalidGsUrl(gcs_url.to_string()))?;
    let mut result = Vec::new();
    loop {
        log::debug!("gcs_bucket {:?}, gcs_path {:?}", gcs_bucket, gcs_path);
        match client.write(gcs_bucket, gcs_path, &mut result, progress).await {
            Ok(ProgressResponse::Continue) => break,
            Ok(ProgressResponse::Cancel) => {
                log::info!("ProgressResponse requesting cancel, exiting");
                return Err(crate::PbmsError::ProgressCancelled);
            }
            Err(e) => match e.downcast_ref::<GcsError>() {
                Some(GcsError::NeedNewAccessToken) => {
                    log::debug!("string_from_gcs got NeedNewAccessToken");
                    let access_token = handle_new_access_token(auth_flow, ui).await?;
                    client.set_access_token(access_token).await;
                }
                Some(GcsError::NotFound(b, p)) => {
                    log::warn!("[gs://{}/{} not found]", b, p);
                    break;
                }
                Some(_) | None => return Err(crate::PbmsError::GcsOperation(e)),
            },
        }
    }
    Ok(String::from_utf8_lossy(&result).to_string())
}

/// List objects from GCS.
pub async fn list_from_gcs<I>(
    bucket: &str,
    prefix: &str,
    auth_flow: &AuthFlowChoice,
    ui: &I,
    client: &Client,
) -> Result<Vec<String>, crate::PbmsError>
where
    I: structured_ui::Interface,
{
    loop {
        match client.list(bucket, prefix).await {
            Ok(result) => return Ok(result),
            Err(e) => match e.downcast_ref::<GcsError>() {
                Some(GcsError::NeedNewAccessToken) => {
                    log::debug!("list_from_gcs got NeedNewAccessToken");
                    let access_token = handle_new_access_token(auth_flow, ui).await?;
                    client.set_access_token(access_token).await;
                }
                Some(_) | None => return Err(crate::PbmsError::GcsOperation(e)),
            },
        }
    }
}

/// Prompt the user to visit the OAUTH2 permissions web page and enter a new
/// authorization code, then convert that to a refresh token and write that
/// refresh token to the ~/.boto file.
async fn update_refresh_token<I>(
    auth_flow: &AuthFlowChoice,
    ui: &I,
) -> std::result::Result<(), crate::PbmsError>
where
    I: structured_ui::Interface,
{
    log::debug!("update_refresh_token");
    let refresh_token = match auth_flow {
        AuthFlowChoice::Default | AuthFlowChoice::Pkce => {
            auth::pkce::new_refresh_token(ui).await.map_err(crate::PbmsError::GetRefreshToken)?
        }
        AuthFlowChoice::Device => {
            auth::device::new_refresh_token(ui).await.map_err(crate::PbmsError::GetRefreshToken)?
        }
        AuthFlowChoice::Exec(_) | AuthFlowChoice::Gcloud => {
            // gcloud and Exec flows manage their own credentials.
            // They do not write or use the ~/.boto file managed by this library.
            return Err(crate::PbmsError::RefreshTokenNotSupported);
        }
        AuthFlowChoice::NoAuth => {
            return Err(crate::PbmsError::RefreshTokenNotSupported);
        }
    };
    log::debug!("Writing credentials");
    let mut credentials = credentials::Credentials::load_or_new().await;
    credentials.oauth2.refresh_token = refresh_token.to_string();
    credentials.save().await.map_err(crate::PbmsError::SaveCredentials)?;
    Ok(())
}
