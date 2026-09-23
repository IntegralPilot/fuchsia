// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides helper functions for reading, writing, and clearing the Android bootloader message
//! stored in the /misc partition.

use anyhow::{Context as _, Error, anyhow};
use block_client::{BlockClient as _, RemoteBlockClient};
use bootloader_message::{BootloaderMessage, BootloaderMessageRaw};
use fidl_fuchsia_storage_block::BlockMarker;
use zerocopy::{FromBytes, IntoBytes};

/// High level interface to operate on the bootloader message.
pub struct BootloaderMessageStore {
    client: RemoteBlockClient,
}

impl BootloaderMessageStore {
    pub async fn new() -> Result<Self, Error> {
        let device =
            fuchsia_component::client::connect_to_protocol_at::<BlockMarker>("/block/misc")
                .context("unable to open /misc device")?;
        let client =
            RemoteBlockClient::new(device).await.context("unable to connect to /misc device")?;
        Ok(Self { client })
    }

    pub async fn read(&self) -> Result<BootloaderMessage, Error> {
        let mut buf = vec![0u8; std::mem::size_of::<BootloaderMessageRaw>()];
        self.client.read_at(buf.as_mut_slice().into(), 0).await?;
        let raw = BootloaderMessageRaw::read_from_bytes(&buf[..])
            .map_err(|_| anyhow!("failed to deserialize bootloader message"))?;
        let message = BootloaderMessage::try_from(raw)?;
        Ok(message)
    }

    pub async fn clear(&self) -> Result<(), Error> {
        let raw = BootloaderMessageRaw::default();
        self.client.write_at(raw.as_bytes().into(), 0).await?;
        Ok(())
    }

    // TODO(https://fxbug.dev/450627605): Remove when this is used.
    #[allow(dead_code)]
    pub async fn write(&self, message: BootloaderMessage) -> Result<(), Error> {
        let raw: BootloaderMessageRaw = message.try_into()?;
        self.client.write_at(raw.as_bytes().into(), 0).await?;
        Ok(())
    }
}
