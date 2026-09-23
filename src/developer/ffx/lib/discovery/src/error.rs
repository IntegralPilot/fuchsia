// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("could not create emulator watcher from path {path:?}: {err}")]
    EmulatorWatcher { path: PathBuf, err: String },

    #[error("could not create gce watcher from path {path:?}: {err}")]
    GceWatcher { path: PathBuf, err: String },

    #[error("could not create fastboot watcher from path {path:?}: {err}")]
    FastbootWatcher { path: PathBuf, err: String },

    #[error("could not parse fastboot devices file {path:?}: {err}")]
    FastbootDiscovery { path: PathBuf, err: fastboot_file_discovery::GetFastbootEntriesError },

    #[error("could not start mdns watcher: {0}")]
    MdnsWatcher(String),

    #[error("could not start fastboot watcher: {0}")]
    FastbootUsbWatcher(String),

    #[error("could not start manual target watcher: {0}")]
    ManualTargetWatcher(String),

    #[error("could not send event: {0}")]
    Send(#[from] futures::channel::mpsc::SendError),

    #[error("could not send event: {0}")]
    TrySend(#[from] futures::channel::mpsc::TrySendError<crate::TargetEvent>),

    #[error("SocketBound events are not supported")]
    SocketBoundUnsupported,

    #[error("target has no addresses")]
    TargetHasNoAddresses,

    #[error("cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("invalid scope ID: {0}")]
    InvalidScopeId(#[from] netext::NetExtError),

    #[error("invalid target specifier: {0}")]
    ParseError(String),
}

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("no cache file has been specified")]
    Unspecified,

    #[error("bad location of cache file at {path:?}")]
    BadLocation { path: PathBuf },

    #[error("opening cache file at {path:?}")]
    OpenFile {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("creating cache file at {path:?}")]
    CreateFile {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("deserializing cache from {path:?}")]
    Deserialize {
        path: PathBuf,
        #[source]
        err: serde_json::Error,
    },

    #[error("serializing cache to {path:?}")]
    Serialize {
        path: PathBuf,
        #[source]
        err: serde_json::Error,
    },

    #[error("bad cache version: {0}")]
    BadVersion(u32),

    #[error("cache expired at {0}")]
    Expired(chrono::DateTime<chrono::Utc>),

    #[error("could not rename cache file from {from:?} to {to:?}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        err: std::io::Error,
    },

    #[error("could not remove cache file at {file:?}")]
    Remove {
        file: PathBuf,
        #[source]
        err: std::io::Error,
    },
}
