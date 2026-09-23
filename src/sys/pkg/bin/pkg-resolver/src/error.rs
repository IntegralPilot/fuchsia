// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg_ext::ResolveError;
use fidl_fuchsia_pkg_resolution::ResolveError as ResolveToolError;

/// Wraps a TUF error and provides an additional Timeout variant
#[derive(Debug, thiserror::Error)]
pub enum TufOrTimeout {
    // LINT.IfChange(tuf_error)
    #[error("rust tuf error")]
    Tuf(#[source] tuf::Error),
    // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go:tuf_error)
    #[error("tuf operation timed out")]
    Timeout,
}

/// Maps the internal ResolveError to the ResolveError type supported for tooling like ffx
pub(crate) fn to_resolve_tool_error(e: ResolveError) -> ResolveToolError {
    match e {
        ResolveError::AccessDenied => ResolveToolError::AccessDenied,
        ResolveError::BlobNotFound => ResolveToolError::BlobNotFound,
        ResolveError::Internal => ResolveToolError::Internal,
        ResolveError::InvalidContext => ResolveToolError::InvalidContext,
        ResolveError::InvalidUrl => ResolveToolError::InvalidUrl,
        ResolveError::Io => ResolveToolError::Io,
        ResolveError::NoSpace => ResolveToolError::NoSpace,
        ResolveError::PackageNotFound => ResolveToolError::PackageNotFound,
        ResolveError::RepoNotFound => ResolveToolError::RepoNotFound,
        ResolveError::UnavailableBlob => ResolveToolError::UnavailableBlob,
        ResolveError::UnavailableRepoMetadata => ResolveToolError::UnavailableRepoMetadata,
    }
}
