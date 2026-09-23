// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Information on client data.

/// For a web site, a client secret is kept locked away in a secure server. This
/// is not a web site and the value is needed, so a non-secret "secret" is used.
///
/// "Google OAuth2 clients always have a secret, even if the client is an
/// installed application/utility such as gsutil.  Of course, in such cases the
/// "secret" is actually publicly known; security depends entirely on the
/// secrecy of refresh tokens, which effectively become bearer tokens."
pub(crate) const CLIENT_ID: &str =
    "835841167576-3nuipp484o20to14dqt1rlm301udcf0a.apps.googleusercontent.com";
pub(crate) const CLIENT_SECRET: &str = "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";

pub(crate) const AUTH_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// URL used for gaining a new access token.
///
/// See RefreshTokenRequest, OauthTokenResponse.
pub(crate) const OAUTH_REFRESH_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
