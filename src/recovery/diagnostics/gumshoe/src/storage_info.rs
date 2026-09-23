// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};

use serde::{Deserialize, Serialize};
use std::fs;

type BlockProxyProvider = fn(&str) -> Result<fidl_fuchsia_storage_block::BlockProxy, Error>;

#[derive(Debug, Serialize, Deserialize)]
pub struct Device {
    logical_path: String,
    download_path: Option<String>,
    block_size: Option<u32>,
    block_count: Option<u64>,
    capacity: Option<u64>,
    name: Option<String>,
}

impl Device {
    pub fn new(
        logical_path: &str,
        block_size: Option<u32>,
        block_count: Option<u64>,
        name: Option<&str>,
    ) -> Self {
        // When zip_with() becomes available, use it:
        //   let capacity = block_size.zip_with(block_count, |s, c| s as u64 * c );
        // in the meantime:
        let capacity = match (block_size, block_count) {
            (Some(block_size), Some(block_count)) => Some(block_size as u64 * block_count),
            (_, _) => None,
        };

        let download_path = match logical_path.rsplit_once('/') {
            Some((_, suffix)) => Some(format!("/download/{suffix}")),
            None => None,
        };

        Self {
            logical_path: logical_path.to_string(),
            download_path: download_path,
            block_size: block_size,
            block_count: block_count,
            capacity: capacity,
            name: name.map(|n| n.to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageInfo {
    devices: Option<Vec<Device>>,
}

impl StorageInfo {
    /// Returns partition's name or Error.
    pub async fn get_name(
        partition_proxy: &fidl_fuchsia_storage_block::BlockProxy,
    ) -> Result<String, Error> {
        match partition_proxy.get_name().await? {
            (status, name) => {
                zx::ok(status)?;
                name.ok_or(anyhow!("name unavailable"))
            }
        }
    }

    /// Returns partition's block_size and block_count, or Error.
    pub async fn get_capacity(
        partition_proxy: &fidl_fuchsia_storage_block::BlockProxy,
    ) -> Result<(u32, u64), Error> {
        let info = partition_proxy.get_info().await?.map_err(zx::Status::err_from_raw)?;
        Ok((info.block_size, info.block_count))
    }

    pub async fn discover_devices(
        partition_provider: BlockProxyProvider,
    ) -> Result<Vec<Device>, Error> {
        let mut devices = Vec::new();
        for entry in fs::read_dir("/gumshoe-dev-class-block")? {
            let entry = entry?;
            let entry_path = entry.path();
            let block_path = entry_path.to_str().ok_or(anyhow::anyhow!("Invalid path"))?;

            let partition_proxy = partition_provider(block_path)?;
            let name = Self::get_name(&partition_proxy).await.ok();

            let (block_size, block_count) = Self::get_capacity(&partition_proxy).await.ok().unzip();

            devices.push(Device::new(block_path, block_size, block_count, name.as_deref()));
        }
        Ok(devices)
    }

    pub async fn new(partition_provider: BlockProxyProvider) -> Result<Self, Error> {
        Ok(StorageInfo { devices: Some(Self::discover_devices(partition_provider).await?) })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn ramdisk_capacity_read() {
        // TODO(b/237561394) Create a RAM Disk with a few testable partitions.
    }
}
