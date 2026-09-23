// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use super::listener::ListenerError;
use crate::events::error::EventError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LogsError {
    #[error("couldn't connect to {protocol}: {source}")]
    ConnectingToService { protocol: &'static str, source: anyhow::Error },

    #[error("couldn't retrieve the ReadOnlyLog debuglog handle: {source}")]
    RetrievingDebugLog { source: fidl::Error },

    #[error("malformed event: `{source}`")]
    MalformedEvent {
        #[from]
        source: EventError,
    },

    #[error("FIDL internal error: `{source}`")]
    FidlError {
        #[from]
        source: fidl::Error,
    },

    #[error("Error duplicating handle: `{source}`")]
    DuplicateHandleError {
        #[from]
        source: fidl::Status,
    },

    #[error("error while handling {protocol} requests: {source}")]
    HandlingRequests { protocol: &'static str, source: fidl::Error },

    #[error("error from a listener: {source}")]
    Listener {
        #[from]
        source: ListenerError,
    },
}
