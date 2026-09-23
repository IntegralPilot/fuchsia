// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_component as fcomponent;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "namespace {prefix} should have exactly one entry but it has {count}. This suggests a \
        bug in the namespace protocol."
    )]
    InvalidNamespaceEntryCount { prefix: String, count: usize },

    #[error("namespace creation failed: {0:?}")]
    NamespaceCreation(fcomponent::NamespaceError),

    #[error("the namespace is not installed: {0:?}")]
    NamespaceNotInstalled(#[source] zx::Status),

    #[error("binding the namespace failed: {0:?}")]
    NamespaceBind(#[source] zx::Status),

    #[error(
        "namespace {prefix} contains incomplete entry. This suggests a bug in the namespace \
            protocol {message}"
    )]
    EntryIncomplete { prefix: String, message: String },

    #[error(
        "namespace {prefix} does not match path. This suggests a bug in the namespace protocol. \
            Path was {path}"
    )]
    PrefixDoesNotMatchPath { prefix: String, path: String },

    #[error("failed to connect to protocol: {0}")]
    ConnectionFailed(String),

    #[error("fidl error")]
    Fidl(#[from] fidl::Error),

    #[error("operation failed: {0:?}")]
    OperationError(fidl_fuchsia_testing_harness::OperationError),
}
