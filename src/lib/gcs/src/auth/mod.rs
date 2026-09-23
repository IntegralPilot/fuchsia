// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provide OAuth2 support for Google Cloud Storage (GCS) access.
//!
//! There are two main APIs here:
//! - new_refresh_token() gets a long-lived, storable (to disk) token than can
//!                       be used to create new access tokens.
//! - new_access_token() accepts a refresh token and returns a reusable though
//!                      short-lived token that can be used to access data.
//!
//! Caution: Some data handled here are security access keys (tokens) and must
//!          not be logged (or traced) or otherwise put someplace where the
//!          secrecy could be compromised. I.e. watch out when adding/reviewing
//!          log::*, log::*, or `impl` of Display or Debug.

pub mod device;
pub mod info;
pub mod pkce;

use crate::error::GcsError;
use anyhow::Result;
use http_body_util::BodyExt;
use hyper::{Method, Request};
use info::{CLIENT_ID, CLIENT_SECRET, OAUTH_REFRESH_TOKEN_ENDPOINT};
use serde::{Deserialize, Serialize};
use std::fmt;

type Body = http_body_util::Full<hyper::body::Bytes>;

#[derive(Clone, PartialEq)]
pub struct GcsCredentials {
    /// The OAuth2 client which requested data access on behalf of the user.
    pub client_id: String,

    /// The client "secret" for desktop applications are not actually secret.
    /// In this case it acts like additional information for the client_id.
    pub client_secret: String,

    /// A long-lived token which is used to mint access tokens. This is private
    /// to the user and must not be printed to a log or otherwise leaked.
    pub refresh_token: String,
}

/// Custom debug to avoid printing the refresh_token.
impl fmt::Debug for GcsCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcsCredentials client_id: {:?}, client_secret: {:?}, refresh_token: <hidden>",
            self.client_id, self.client_secret
        )
    }
}

impl GcsCredentials {
    pub fn new(refresh_token: &str) -> Self {
        Self {
            client_id: CLIENT_ID.to_string(),
            client_secret: CLIENT_SECRET.to_string(),
            refresh_token: refresh_token.to_string(),
        }
    }
}

/// Response body from [`OAUTH_REFRESH_TOKEN_ENDPOINT`].
/// 'expires_in' is intentionally omitted.
#[derive(Deserialize)]
struct OauthTokenResponse {
    /// A limited time (see `expires_in`) token used in an Authorization header.
    access_token: String,
}

/// POST body to [`OAUTH_REFRESH_TOKEN_ENDPOINT`].
#[derive(Serialize)]
struct RefreshTokenRequest<'a> {
    /// A value provided by GCS for fetching tokens.
    client_id: &'a str,

    /// A (normally secret) value provided by GCS for fetching tokens.
    client_secret: &'a str,

    /// A long lasting secret token used to attain a new `access_token`.
    refresh_token: &'a str,

    /// Will be "refresh_token" for a refresh token.
    grant_type: &'a str,
}

/// Use the 'credentials' to request an access token.
///
/// Access tokens are short-lived. Unlike a refresh token, there's little value
/// in storing an access token to disk for later use, though it may be used many
/// times before needing to get a new access_token.
pub async fn new_access_token(credentials: &GcsCredentials) -> Result<String, GcsError> {
    log::debug!("new_access_token");
    let req_body = RefreshTokenRequest {
        client_id: &credentials.client_id,
        client_secret: &credentials.client_secret,
        refresh_token: &credentials.refresh_token,
        grant_type: "refresh_token",
    };
    let body = serde_json::to_vec(&req_body)?;
    let req = Request::builder()
        .method(Method::POST)
        .uri(OAUTH_REFRESH_TOKEN_ENDPOINT)
        .body(Body::from(body))?;

    let https_client = fuchsia_hyper::new_https_client();
    let res = https_client.request(req).await?;

    if res.status().is_success() {
        let bytes = res.into_body().collect().await?.to_bytes();
        let info: OauthTokenResponse = serde_json::from_slice(&bytes)?;
        Ok(info.access_token)
    } else {
        match res.status() {
            http::StatusCode::BAD_REQUEST => return Err(GcsError::NeedNewRefreshToken),
            _ => return Err(GcsError::HttpResponseError(res.status())),
        }
    }
}
