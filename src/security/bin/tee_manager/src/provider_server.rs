// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_fuchsia_tee_manager::{ProviderRequest, ProviderRequestStream};
use futures::stream::TryStreamExt as _;

/// `ProviderServer` implements the fuchsia.tee.manager.Provider FIDL protocol.
pub struct ProviderServer {
    storage_dir: &'static str,
}

impl ProviderServer {
    pub fn try_new(storage_dir: &'static str) -> Result<Self, Error> {
        std::fs::create_dir_all(&storage_dir)?;
        Ok(Self { storage_dir })
    }

    pub async fn serve(&self, stream: ProviderRequestStream) -> Result<(), Error> {
        let Self { storage_dir } = self;
        stream
            .err_into()
            .try_for_each(|ProviderRequest::RequestPersistentStorage { dir, control_handle: _ }| {
                futures::future::ready(
                    fuchsia_fs::directory::open_channel_in_namespace(
                        storage_dir,
                        fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
                        dir,
                    )
                    .with_context(|| {
                        format!("Failed to connect to storage directory ({}) service", storage_dir,)
                    }),
                )
            })
            .await
    }
}
