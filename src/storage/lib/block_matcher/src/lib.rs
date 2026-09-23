// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_storage_block::BlockProxy;
use fidl_fuchsia_storage_partitions as fpartitions;
use fs_management::filesystem::BlockConnector;
use fs_management::format::{DiskFormat, detect_disk_format};
use fuchsia_component::client::ServiceInstanceStream;
use futures::TryStreamExt;

pub type Guid = [u8; 16];

pub fn into_guid(guid: Guid) -> fidl_fuchsia_storage_block::Guid {
    fidl_fuchsia_storage_block::Guid { value: guid }
}

pub fn create_random_guid() -> Guid {
    *uuid::Uuid::new_v4().as_bytes()
}

async fn partition_type_guid_matches(guid: &Guid, partition: &BlockProxy) -> Result<bool> {
    let (status, type_guid) =
        partition.get_type_guid().await.context("Failed to get type guid (fidl error")?;
    zx::ok(status).context("Failed to get type guid")?;
    let type_guid = if let Some(guid) = type_guid { guid } else { return Ok(false) };
    let matched = type_guid.value == *guid;
    log::info!(matched, type_guid:?, target_guid:?=guid; "matching type guid");
    Ok(matched)
}

async fn partition_instance_guid_matches(guid: &Guid, partition: &BlockProxy) -> Result<bool> {
    let (status, instance_guid) =
        partition.get_instance_guid().await.context("Failed to get instance guid (fidl error")?;
    zx::ok(status).context("Failed to get instance guid")?;
    let instance_guid = if let Some(guid) = instance_guid { guid } else { return Ok(false) };
    let matched = instance_guid.value == *guid;
    log::info!(matched, instance_guid:?, target_guid:?=guid; "matching instance guid");
    Ok(matched)
}

async fn partition_name_matches(name: &str, partition: &BlockProxy) -> Result<bool> {
    let (status, partition_name) =
        partition.get_name().await.context("Failed to get partition name (fidl error")?;
    zx::ok(status).context("Failed to get partition name")?;
    let partition_name = if let Some(name) = partition_name { name } else { return Ok(false) };
    let matched = partition_name == name;
    log::info!(matched, partition_name = partition_name.as_str(), target_name = name; "matching name");
    Ok(matched)
}

async fn block_contents_match(format: DiskFormat, block: &BlockProxy) -> Result<bool> {
    let content_format = detect_disk_format(block).await;
    Ok(format == content_format)
}

/// A constraint for the block device being waited for in `wait_for_block_device`.
#[derive(Debug)]
pub enum BlockDeviceMatcher<'a> {
    /// Only matches block devices that have this type Guid.
    TypeGuid(&'a Guid),

    /// Only matches block devices that have this instance Guid.
    InstanceGuid(&'a Guid),

    /// Only matches block devices that have this name.
    Name(&'a str),

    /// Only matches block devices whose contents match the given format.
    ContentsMatch(DiskFormat),
}

impl BlockDeviceMatcher<'_> {
    async fn matches(&self, partition: &BlockProxy) -> Result<bool> {
        match self {
            Self::TypeGuid(guid) => partition_type_guid_matches(guid, partition).await,
            Self::InstanceGuid(guid) => partition_instance_guid_matches(guid, partition).await,
            Self::Name(name) => partition_name_matches(name, partition).await,
            Self::ContentsMatch(format) => block_contents_match(*format, partition).await,
        }
    }
}

async fn matches_all(partition: &BlockProxy, matchers: &[BlockDeviceMatcher<'_>]) -> bool {
    for matcher in matchers {
        if !matcher.matches(partition).await.unwrap_or(false) {
            return false;
        }
    }
    true
}

/// Waits for the first partition service instance that meets all of the requirements of `matchers`.
/// Returns the path to the matched block device.
pub async fn wait_for_block_device(
    matchers: &[BlockDeviceMatcher<'_>],
    mut stream: ServiceInstanceStream<fpartitions::PartitionServiceMarker>,
) -> Result<fpartitions::PartitionServiceProxy> {
    while let Some(proxy) = stream.try_next().await? {
        let partition = proxy.connect_block()?.into_proxy();
        if matches_all(&partition, matchers).await {
            return Ok(proxy);
        }
    }
    unreachable!()
}

/// Returns the first partition in `partitions` matching all of `matchers.`  Ok(None) indicates no
/// partitions matched.
pub async fn find_block_device<C, Iter>(
    matchers: &[BlockDeviceMatcher<'_>],
    partitions: Iter,
) -> Result<Option<C>>
where
    C: BlockConnector,
    Iter: Iterator<Item = C>,
{
    for connector in partitions {
        let partition = connector.connect_block()?.into_proxy();
        if matches_all(&partition, matchers).await {
            return Ok(Some(connector));
        }
    }
    Ok(None)
}

pub const DEFAULT_BENCHMARK_FVM_SIZE_BYTES: u64 = 160 * 1024 * 1024;

/// Finds a partition reserved for testing/benchmarks in GPT, or creates it if absent.
pub async fn find_or_create_test_partition(
    service: fuchsia_component::client::Service<fpartitions::PartitionServiceMarker>,
    manager: fpartitions::PartitionsManagerProxy,
    partition_size_bytes: u64,
) -> Result<fpartitions::PartitionServiceProxy> {
    if let Some(connector) = find_block_device(
        &[
            BlockDeviceMatcher::Name(fs_management::format::constants::BENCHMARK_FVM_VOLUME_NAME),
            BlockDeviceMatcher::TypeGuid(
                &fs_management::format::constants::BENCHMARK_FVM_TYPE_GUID,
            ),
        ],
        service.clone().enumerate().await.context("Failed to enumerate partitions")?.into_iter(),
    )
    .await
    .context("Error while searching for benchmark-fvm")?
    {
        return Ok(connector);
    }

    if let Some(connector) = find_block_device(
        &[BlockDeviceMatcher::Name(fs_management::format::constants::PAD_RW_PARTITION_LABEL)],
        service.clone().enumerate().await.context("Failed to enumerate partitions")?.into_iter(),
    )
    .await
    .context("Error while searching for pad_rw")?
    {
        return Ok(connector);
    }

    // Otherwise, create the test partition in the GPT.
    let info = manager
        .get_block_info()
        .await
        .context("FIDL error on get_block_info")?
        .map_err(zx::Status::err_from_raw)
        .context("get_block_info failed")?;
    let transaction = manager
        .create_transaction()
        .await
        .context("FIDL error on create_transaction")?
        .map_err(zx::Status::err_from_raw)
        .context("create_transaction failed")?;
    let request = fpartitions::PartitionsManagerAddPartitionRequest {
        transaction: Some(transaction.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
        name: Some(fs_management::format::constants::BENCHMARK_FVM_VOLUME_NAME.to_string()),
        type_guid: Some(fidl_fuchsia_storage_block::Guid {
            value: fs_management::format::constants::BENCHMARK_FVM_TYPE_GUID,
        }),
        num_blocks: Some(partition_size_bytes / info.1 as u64),
        ..Default::default()
    };
    manager
        .add_partition(request)
        .await
        .context("FIDL error on add_partition")?
        .map_err(zx::Status::err_from_raw)
        .context("add_partition failed")?;
    manager
        .commit_transaction(transaction)
        .await
        .context("FIDL error on commit_transaction")?
        .map_err(zx::Status::err_from_raw)
        .context("commit_transaction failed")?;

    let service_instances = service.enumerate().await.context("Failed to enumerate partitions")?;
    find_block_device(
        &[
            BlockDeviceMatcher::Name(fs_management::format::constants::BENCHMARK_FVM_VOLUME_NAME),
            BlockDeviceMatcher::TypeGuid(
                &fs_management::format::constants::BENCHMARK_FVM_TYPE_GUID,
            ),
        ],
        service_instances.into_iter(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("Failed to find newly created test partition"))
}
