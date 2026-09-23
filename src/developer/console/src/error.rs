// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::io::IoHandlesError;
use crate::namespace::NamespaceError;
use crate::process::ProcessError;
use crate::program::ProgramError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("encountered unknown FIDL interaction: {name} {unknown_ordinal}")]
    UnknownInteraction { name: &'static str, unknown_ordinal: u64 },
    #[error(transparent)]
    MissingFidlField(#[from] MissingFidlFieldError),
    #[error("unexpected FIDL error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("can't build namespace: {0}")]
    Namespace(#[from] NamespaceError),
    #[error("failed to load program: {0}")]
    Program(#[from] ProgramError),
    #[error("failed to build process: {0}")]
    Process(#[from] ProcessError),
    #[error("failed to build io handles: {0}")]
    IoHandles(#[from] IoHandlesError),
}

#[derive(Error, Debug)]
#[error("missing expected FIDL field in API: {0}")]
pub struct MissingFidlFieldError(pub &'static str);
