// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use block_client::BlockClient;

use hyper::body::Sender;
use std::cmp::min;
use std::collections::HashMap;
use std::fs;
use std::future::Future;

type BlockProxyProvider = fn(&str) -> Result<fidl_fuchsia_storage_block::BlockProxy, Error>;

#[derive(Default)]
pub struct PartitionReader {
    sizes: HashMap<String, (u64, u32)>,
    block_proxy_provider: Option<BlockProxyProvider>,
}

impl PartitionReader {
    /// Returns block_count (u64) and block_size (u32) from cache of
    /// partitions keyed by logical name (eg, "000", "001", "002"...)
    /// or error if not found.
    pub fn size(&self, name: &str) -> Result<(u64, u32), Error> {
        self.sizes.get(name).ok_or(anyhow!("Unknown name")).copied()
    }

    /// Retrieve sizes of all block devices.
    async fn get_sizes(
        block_proxy_provider: BlockProxyProvider,
    ) -> Result<HashMap<String, (u64, u32)>, Error> {
        let mut sizes = HashMap::new();
        for entry in fs::read_dir("/gumshoe-dev-class-block")? {
            let entry = entry?;
            let entry_path = entry.path();
            let name = entry_path.file_name().ok_or(anyhow!("Invalid name (No OsString)"))?;
            let name = name.to_str().ok_or(anyhow!("Invalid name (No String)"))?.to_string();
            let block_path = entry_path.to_str().ok_or(anyhow!("Invalid path"))?;

            let block_proxy = block_proxy_provider(block_path)?;
            let info = block_proxy.get_info().await?.map_err(zx::Status::err_from_raw)?;
            sizes.insert(name, (info.block_count, info.block_size));
        }
        Ok(sizes)
    }

    pub async fn new(block_proxy_provider: BlockProxyProvider) -> Result<Self, Error> {
        Ok(PartitionReader {
            sizes: PartitionReader::get_sizes(block_proxy_provider).await?,
            block_proxy_provider: Some(block_proxy_provider),
        })
    }

    /// Returns an un-executed Future (so not yet "awaited upon") that
    /// will write the bytes of the named partition to given Sender.
    ///
    /// Responder awaits on this Future from a detached thread
    /// responsible for streaming the data back to client.
    pub fn allocate_reader(
        &self,
        name: String,
        sender: Sender,
    ) -> Result<impl Future<Output = Result<(), Error>> + 'static, Error> {
        let (block_count, block_size) = self.size(&name)?;

        let Some(block_proxy_provider) = self.block_proxy_provider else {
            return Err(anyhow!("No block_proxy_provider"));
        };

        async fn reader(
            block_proxy_provider: BlockProxyProvider,
            name: String,
            block_count: u64,
            block_size: u32,
            mut sender: Sender,
        ) -> Result<(), Error> {
            let path = format!("/gumshoe-dev-class-block/{}", name);
            let proxy = block_proxy_provider(&path)?;
            let rbc = block_client::RemoteBlockClient::new(proxy).await?;

            let mut offset: u64 = 0;
            while offset < block_count * block_size as u64 {
                let remaining_read = block_count * block_size as u64 - offset;
                let read_size = min(remaining_read, block_size as u64 * 10000);
                let mut bytes: Vec<u8> = vec![0; read_size as usize];

                rbc.read_at(block_client::MutableBufferSlice::Memory(&mut bytes), offset).await?;
                offset += bytes.len() as u64;

                sender.send_data(bytes.into()).await?;
            }
            Ok(())
        }
        Ok(reader(block_proxy_provider, name, block_count, block_size, sender))
    }
}
