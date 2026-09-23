// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchUrlError {
    #[error("connecting to http service")]
    FidlHttpServiceConnectionError(#[source] anyhow::Error),

    #[error("calling Loader::Fetch")]
    LoaderFIDLError(#[from] fidl::Error),

    #[error("reading URL body")]
    UrlReadBodyError,

    #[error("reading data from socket")]
    ReadFromSocketError(#[source] std::io::Error),

    #[error("LoaderProxy error: '{0:?}'")]
    LoaderFetchError(fidl_fuchsia_net_http::Error),

    #[error("no HTTP status response")]
    NoStatusResponse,

    #[error("unexpected http status code {0}")]
    UnexpectedHttpStatusCode(u32),

    #[error("read size mismatch, expected {0},  got {1}")]
    SizeReadMismatch(u64, u64),

    #[error("range header is not allowed when the range argument is also provided")]
    DuplicateRangeHeader,
}
