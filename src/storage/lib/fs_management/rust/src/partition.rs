// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::format::{DiskFormat, detect_disk_format};
use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_device::{ControllerMarker, ControllerProxy};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::{BlockMarker, Guid, VolumeManagerProxy};
use fuchsia_async::TimeoutExt;
use fuchsia_component_client::connect_to_named_protocol_at_dir_root;
use fuchsia_fs::directory::{WatchEvent, Watcher};
use futures::StreamExt;
use zx::{self as zx, MonotonicDuration};

/// Set of parameters to use for identifying the correct partition to open via
/// [`open_partition`]
///
/// If multiple matchers are specified, the first partition that satisfies any set
/// of matchers will be used. At least one of [`type_guids`], [`instance_guids`], [`labels`],
/// [`detected_formats`], or [`parent_device`] must be specified.
#[derive(Default, Clone)]
pub struct PartitionMatcher {
    /// Set of type GUIDs the partition must match. Ignored if empty.
    pub type_guids: Option<Vec<[u8; 16]>>,
    /// Set of instance GUIDs the partition must match. Ignored if empty.
    pub instance_guids: Option<Vec<[u8; 16]>>,
    pub labels: Option<Vec<String>>,
    pub detected_disk_formats: Option<Vec<DiskFormat>>,
    /// partition must be a child of this device.
    pub parent_device: Option<String>,
    /// The topological path must not start with this prefix.
    pub ignore_prefix: Option<String>,
    /// The topological path must not contain this substring.
    pub ignore_if_path_contains: Option<String>,
}

const BLOCK_DEV_PATH: &str = "/dev/class/block/";

/// Waits for a partition to appear on BLOCK_DEV_PATH that matches the fields in the
/// PartitionMatcher. Returns the path of the partition if found. Errors after timeout duration.
// TODO(https://fxbug.dev/42072982): Most users end up wanting the things we open for checking the partition,
// like the partition proxy and the topological path. We should consider returning all those
// resources instead of forcing them to retrieve them again.
pub async fn find_partition(
    matcher: PartitionMatcher,
    timeout: MonotonicDuration,
) -> Result<ControllerProxy, Error> {
    let dir = fuchsia_fs::directory::open_in_namespace(BLOCK_DEV_PATH, fuchsia_fs::PERM_READABLE)?;
    find_partition_in(&dir, matcher, timeout).await
}

/// Waits for a partition to appear in [`dir`] that matches the fields in [`matcher`]. Returns the
/// topological path of the partition if found. Returns an error after the timeout duration
/// expires.
pub async fn find_partition_in(
    dir: &fio::DirectoryProxy,
    matcher: PartitionMatcher,
    timeout: MonotonicDuration,
) -> Result<ControllerProxy, Error> {
    let timeout_seconds = timeout.into_seconds();
    async {
        let mut watcher = Watcher::new(dir).await.context("making watcher")?;
        while let Some(message) = watcher.next().await {
            let message = message.context("watcher channel returned error")?;
            match message.event {
                WatchEvent::ADD_FILE | WatchEvent::EXISTING => {
                    let filename = message.filename.to_str().unwrap();
                    if filename == "." {
                        continue;
                    }
                    let proxy = connect_to_named_protocol_at_dir_root::<ControllerMarker>(
                        &dir,
                        &format!("{filename}/device_controller"),
                    )
                    .context("opening partition path")?;
                    match partition_matches_with_proxy(&proxy, &matcher).await {
                        Ok(true) => {
                            return Ok(proxy);
                        }
                        Ok(false) => {}
                        Err(error) => {
                            log::info!(error:?; "Failure in partition match. Transient device?");
                        }
                    }
                }
                _ => (),
            }
        }
        Err(anyhow!("Watch stream unexpectedly ended"))
    }
    .on_timeout(timeout, || {
        Err(anyhow!("Timed out after {}s without finding expected partition", timeout_seconds))
    })
    .await
}

/// Checks if the partition associated with proxy matches the matcher.
/// An error isn't necessarily an issue - we might be using a matcher that wants a type guid,
/// but the device we are currently checking doesn't implement get_type_guid. The error message may
/// help debugging why no partition was matched but should generally be considered recoverable.
pub async fn partition_matches_with_proxy(
    controller_proxy: &ControllerProxy,
    matcher: &PartitionMatcher,
) -> Result<bool, Error> {
    assert!(
        matcher.type_guids.is_some()
            || matcher.instance_guids.is_some()
            || matcher.detected_disk_formats.is_some()
            || matcher.parent_device.is_some()
            || matcher.labels.is_some()
    );

    let (partition_proxy, partition_server_end) = fidl::endpoints::create_proxy::<BlockMarker>();
    controller_proxy
        .connect_to_device_fidl(partition_server_end.into_channel())
        .context("connecting to partition protocol")?;

    if let Some(matcher_type_guids) = &matcher.type_guids {
        let (status, guid_option) =
            partition_proxy.get_type_guid().await.context("transport error on get_type_guid")?;
        zx::Status::ok(status).context("get_type_guid failed")?;
        let guid = guid_option.ok_or_else(|| anyhow!("Expected type guid"))?;
        if !matcher_type_guids.iter().any(|x| x == &guid.value) {
            return Ok(false);
        }
    }

    if let Some(matcher_instance_guids) = &matcher.instance_guids {
        let (status, guid_option) = partition_proxy
            .get_instance_guid()
            .await
            .context("transport error on get_instance_guid")?;
        zx::Status::ok(status).context("get_instance_guid failed")?;
        let guid = guid_option.ok_or_else(|| anyhow!("Expected instance guid"))?;
        if !matcher_instance_guids.iter().any(|x| x == &guid.value) {
            return Ok(false);
        }
    }

    if let Some(matcher_labels) = &matcher.labels {
        let (status, name) =
            partition_proxy.get_name().await.context("transport error on get_name")?;
        zx::Status::ok(status).context("get_name failed")?;
        let name = name.ok_or_else(|| anyhow!("Expected name"))?;
        if name.is_empty() {
            return Ok(false);
        }
        let mut matches_label = false;
        for label in matcher_labels {
            if name == *label {
                matches_label = true;
                break;
            }
        }
        if !matches_label {
            return Ok(false);
        }
    }

    let topological_path = controller_proxy
        .get_topological_path()
        .await
        .context("get_topological_path failed")?
        .map_err(zx::Status::err_from_raw)?;

    if let Some(matcher_parent_device) = &matcher.parent_device {
        if !topological_path.starts_with(matcher_parent_device) {
            return Ok(false);
        }
    }

    if let Some(matcher_ignore_prefix) = &matcher.ignore_prefix {
        if topological_path.starts_with(matcher_ignore_prefix) {
            return Ok(false);
        }
    }

    if let Some(matcher_ignore_if_path_contains) = &matcher.ignore_if_path_contains {
        if topological_path.find(matcher_ignore_if_path_contains) != None {
            return Ok(false);
        }
    }

    if let Some(matcher_detected_disk_formats) = &matcher.detected_disk_formats {
        let detected_format = detect_disk_format(&partition_proxy).await;
        if !matcher_detected_disk_formats.iter().any(|x| x == &detected_format) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub async fn fvm_allocate_partition(
    fvm_proxy: &VolumeManagerProxy,
    type_guid: [u8; 16],
    instance_guid: [u8; 16],
    name: &str,
    flags: u32,
    slice_count: u64,
) -> Result<ControllerProxy, Error> {
    let status = fvm_proxy
        .allocate_partition(
            slice_count,
            &Guid { value: type_guid },
            &Guid { value: instance_guid },
            name,
            flags,
        )
        .await?;
    zx::Status::ok(status)?;

    let matcher = PartitionMatcher {
        type_guids: Some(vec![type_guid]),
        instance_guids: Some(vec![instance_guid]),
        ..Default::default()
    };

    find_partition(matcher, MonotonicDuration::from_seconds(40)).await
}

#[cfg(test)]
mod tests {
    use super::{PartitionMatcher, partition_matches_with_proxy};
    use crate::format::{DiskFormat, constants};
    use block_server::{DeviceInfo, PartitionInfo};
    use fidl::endpoints::{RequestStream as _, create_proxy_and_stream};
    use fidl_fuchsia_device::{ControllerMarker, ControllerRequest};
    use fidl_fuchsia_storage_block::BlockRequestStream;
    use fuchsia_async as fasync;
    use futures::{FutureExt, StreamExt, pin_mut, select};
    use std::sync::Arc;
    use vmo_backed_block_server::{InitialContents, VmoBackedServerOptions};

    const VALID_TYPE_GUID: [u8; 16] = [1; 16];
    const VALID_INSTANCE_GUID: [u8; 16] = [2; 16];
    const VALID_LABEL: &str = "fake-server";

    const INVALID_GUID_1: [u8; 16] = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e,
        0x2f,
    ];

    const INVALID_GUID_2: [u8; 16] = [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e,
        0x3f,
    ];

    const INVALID_LABEL_1: &str = "TheWrongLabel";
    const INVALID_LABEL_2: &str = "StillTheWrongLabel";
    const PARENT_DEVICE_PATH: &str = "/fake/block/device/1";
    const NOT_PARENT_DEVICE_PATH: &str = "/fake/block/device/2";
    const DEFAULT_PATH: &str = "/fake/block/device/1/partition/001";

    async fn check_partition_matches(matcher: &PartitionMatcher) -> bool {
        let (proxy, mut stream) = create_proxy_and_stream::<ControllerMarker>();

        let fake_block_server = Arc::new(
            VmoBackedServerOptions {
                block_size: 512,
                info: DeviceInfo::Partition(PartitionInfo {
                    type_guid: VALID_TYPE_GUID,
                    instance_guid: VALID_INSTANCE_GUID,
                    name: VALID_LABEL.to_string(),
                    ..Default::default()
                }),
                initial_contents: InitialContents::FromCapacityAndBuffer(
                    1000,
                    &constants::FVM_MAGIC,
                ),
                ..Default::default()
            }
            .build()
            .unwrap(),
        );

        let mock_controller = async {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(ControllerRequest::GetTopologicalPath { responder }) => {
                        responder.send(Ok(DEFAULT_PATH)).unwrap();
                    }
                    Ok(ControllerRequest::ConnectToDeviceFidl { server, .. }) => {
                        let fake_block_server = fake_block_server.clone();
                        fasync::Task::spawn(async move {
                            if let Err(e) = fake_block_server
                                .serve(BlockRequestStream::from_channel(
                                    fasync::Channel::from_channel(server),
                                ))
                                .await
                            {
                                println!("VmoBackedServer::serve failed: {e:?}");
                            }
                        })
                        .detach();
                    }
                    _ => {
                        println!("Unexpected request: {:?}", request);
                        unreachable!()
                    }
                }
            }
        }
        .fuse();

        pin_mut!(mock_controller);

        select! {
            _ = mock_controller => unreachable!(),
            matches = partition_matches_with_proxy(&proxy, &matcher).fuse() => matches,
        }
        .unwrap_or(false)
    }

    #[fuchsia::test]
    async fn test_type_guid_match() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID, INVALID_GUID_1]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_instance_guid_match() {
        let matcher = PartitionMatcher {
            instance_guids: Some(vec![VALID_INSTANCE_GUID, INVALID_GUID_1]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_type_and_instance_guid_match() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID, INVALID_GUID_1]),
            instance_guids: Some(vec![VALID_INSTANCE_GUID, INVALID_GUID_2]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_parent_match() {
        let matcher = PartitionMatcher {
            parent_device: Some(PARENT_DEVICE_PATH.to_string()),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);

        let matcher2 = PartitionMatcher {
            parent_device: Some(NOT_PARENT_DEVICE_PATH.to_string()),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher2).await, false);
    }

    #[fuchsia::test]
    async fn test_single_label_match() {
        let the_labels = vec![VALID_LABEL.to_string()];
        let matcher = PartitionMatcher { labels: Some(the_labels), ..Default::default() };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_multi_label_match() {
        let mut the_labels = vec![VALID_LABEL.to_string()];
        the_labels.push(INVALID_LABEL_1.to_string());
        the_labels.push(INVALID_LABEL_2.to_string());
        let matcher = PartitionMatcher { labels: Some(the_labels), ..Default::default() };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_ignore_prefix_mismatch() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID]),
            ignore_prefix: Some("/fake/block/device".to_string()),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, false);
    }

    #[fuchsia::test]
    async fn test_ignore_prefix_match() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID]),
            ignore_prefix: Some("/real/block/device".to_string()),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_ignore_if_path_contains_mismatch() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID]),
            ignore_if_path_contains: Some("/device/1".to_string()),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, false);
    }

    #[fuchsia::test]
    async fn test_ignore_if_path_contains_match() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID]),
            ignore_if_path_contains: Some("/device/0".to_string()),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_type_and_label_match() {
        let the_labels = vec![VALID_LABEL.to_string()];
        let matcher = PartitionMatcher {
            type_guids: Some(vec![VALID_TYPE_GUID]),
            labels: Some(the_labels),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_type_guid_mismatch() {
        let matcher = PartitionMatcher {
            type_guids: Some(vec![INVALID_GUID_1, INVALID_GUID_2]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, false);
    }

    #[fuchsia::test]
    async fn test_instance_guid_mismatch() {
        let matcher = PartitionMatcher {
            instance_guids: Some(vec![INVALID_GUID_1, INVALID_GUID_2]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, false);
    }

    #[fuchsia::test]
    async fn test_label_mismatch() {
        let mut the_labels = vec![INVALID_LABEL_1.to_string()];
        the_labels.push(INVALID_LABEL_2.to_string());
        let matcher = PartitionMatcher { labels: Some(the_labels), ..Default::default() };
        assert_eq!(check_partition_matches(&matcher).await, false);
    }

    #[fuchsia::test]
    async fn test_detected_disk_format_match() {
        let matcher = PartitionMatcher {
            detected_disk_formats: Some(vec![DiskFormat::Fvm, DiskFormat::Minfs]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, true);
    }

    #[fuchsia::test]
    async fn test_detected_disk_format_mismatch() {
        let matcher = PartitionMatcher {
            detected_disk_formats: Some(vec![DiskFormat::Fxfs, DiskFormat::Minfs]),
            ..Default::default()
        };
        assert_eq!(check_partition_matches(&matcher).await, false);
    }
}
