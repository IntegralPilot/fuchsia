// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Logic for persisting netstack state across reboots.

use std::io::Write as _;

use fidl_fuchsia_netstack_persistence as fnetstack_persistence;
use log::{debug, info, warn};
use netstack3_core::ip::IidSecret;
use rand::Rng;
use thiserror::Error;

/// State that is persisted by the netstack across reboots.
pub(crate) struct State {
    /// The secret key used to generate opaque IIDs for use in stable SLAAC
    /// addresses, as defined in [RFC 7217 section 5].
    ///
    /// [RFC 7217 section 5]: https://tools.ietf.org/html/rfc7217/#section-5
    pub opaque_iid_secret_key: IidSecret,
}

const PATH: &str = "/data/state";
const TMP_PATH: &str = "/data/state.tmp";

#[derive(Error, Debug)]
enum LoadError {
    #[error("error reading file from persistent storage: {0}")]
    ReadFile(std::io::Error),
    #[error("error deserializing FIDL for persisted state: {0}")]
    DeserializeFidl(fidl::Error),
    #[error("{0} field not set in persistent FIDL")]
    MissingField(&'static str),
}

#[derive(Error, Debug)]
enum StoreError {
    #[error("error opening file to store new persisted state: {0}")]
    OpenFile(std::io::Error),
    #[error("error serializing persisted state as FIDL: {0}")]
    SerializeFidl(fidl::Error),
    #[error("error writing new persisted state to file: {0}")]
    WriteData(std::io::Error),
    #[error("error syncing write of new persisted state to file: {0}")]
    SyncFile(std::io::Error),
    #[error("error overwriting persisted state with temporary file: {0}")]
    RenameTempFile(std::io::Error),
}

impl State {
    /// Returns persisted [`State`].
    ///
    /// Attempts first to load state from persistent storage. If errors are
    /// encountered (e.g. because no state has yet been persisted), a new instance
    /// is generated. Then attempts to store this new state. If errors are
    /// encountered when attempting to *store*, the new state will be returned as
    /// temporary state.
    pub(crate) fn load_or_create<R: Rng>(rng: &mut R) -> Self {
        debug!("getting or creating state from persistent storage");

        match Self::load() {
            Ok(state) => {
                info!("using existing persisted state");
                state
            }
            Err(e) => {
                debug!("failed to load existing persisted state; generating new state: {e}");
                let state = Self::new(rng);
                state.store().unwrap_or_else(|e| warn!("failed to store new persisted state: {e}"));

                info!("generated and stored new persisted state");
                state
            }
        }
    }

    fn new<R: Rng>(rng: &mut R) -> Self {
        Self { opaque_iid_secret_key: IidSecret::new_random(rng) }
    }

    fn load() -> Result<Self, LoadError> {
        let data = std::fs::read(PATH).map_err(LoadError::ReadFile)?;
        let fnetstack_persistence::State { opaque_iid_secret_key, __source_breaking } =
            fidl::unpersist(&data).map_err(LoadError::DeserializeFidl)?;
        let secret_key =
            opaque_iid_secret_key.ok_or(LoadError::MissingField("opaque_iid_secret_key"))?;

        Ok(State { opaque_iid_secret_key: IidSecret::new(secret_key) })
    }

    fn store(&self) -> Result<(), StoreError> {
        {
            // Write to the file, using a temporary file to ensure the original data isn't
            // lost if there is an error.
            let mut file = std::fs::File::create(TMP_PATH).map_err(StoreError::OpenFile)?;
            let State { opaque_iid_secret_key } = self;
            let data = fidl::persist(&fnetstack_persistence::State {
                opaque_iid_secret_key: Some(opaque_iid_secret_key.into_inner()),
                __source_breaking: fidl::marker::SourceBreaking,
            })
            .map_err(StoreError::SerializeFidl)?;
            file.write_all(&data).map_err(StoreError::WriteData)?;
            // This fsync is required because the storage stack doesn't guarantee data is
            // flushed before the rename, even after we close the file.
            file.sync_all().map_err(StoreError::SyncFile)?;
        }
        std::fs::rename(TMP_PATH, PATH).map_err(StoreError::RenameTempFile)?;
        Ok(())
    }
}
