// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_io::{self as fio, DirectoryProxy, PERM_READABLE};
use fuchsia_fs::directory::{DirentKind, open_directory, open_file, readdir};
use futures::future::BoxFuture;
use futures::{FutureExt, TryFutureExt};

pub fn register_stats(root: &fuchsia_inspect::Node, data_dir: DirectoryProxy, tree_stats: bool) {
    root.record_lazy_child("data_stats", move || {
        let data_dir = Clone::clone(&data_dir);
        async move {
            let inspector = fuchsia_inspect::Inspector::default();
            let root = inspector.root();

            // Filesystem info stats
            let stats_node = root.create_child("stats");
            record_filesystem_info(&stats_node, &data_dir).await?;
            root.record(stats_node);

            if tree_stats {
                // Tree size stats
                let tree_size_node = root.create_child("data");
                record_tree_size(tree_size_node.clone_weak(), data_dir).await?;
                root.record(tree_size_node);
            }

            Ok(inspector)
        }
        .or_else(|e: Error| async move {
            let inspector = fuchsia_inspect::Inspector::default();
            let root = inspector.root();
            root.record_string(
                "error",
                format!("Failed to collect fshost inspect metrics: {:?}", e),
            );
            Ok(inspector)
        })
        .boxed()
    });
}

/// Records filesystem information
async fn record_filesystem_info(
    stats_node: &fuchsia_inspect::Node,
    data_dir: &DirectoryProxy,
) -> Result<(), Error> {
    let (status, info_wrapped) =
        data_dir.query_filesystem().await.context("Transport error on query_filesystem")?;
    zx::Status::ok(status).context("query_filesystem failed")?;
    let info = info_wrapped.ok_or_else(|| anyhow!("failed to get filesystem info"))?;
    stats_node.record_uint("fvm_free_bytes", info.free_shared_pool_bytes);
    stats_node.record_uint("allocated_inodes", info.total_nodes);
    stats_node.record_uint("used_inodes", info.used_nodes);
    // Total bytes is the size of the partition plus the size it could conceivably grow into.
    // TODO(https://fxbug.dev/42165435): Remove this misleading metric.
    stats_node.record_uint("total_bytes", info.total_bytes + info.free_shared_pool_bytes);
    stats_node.record_uint("allocated_bytes", info.total_bytes);
    stats_node.record_uint("used_bytes", info.used_bytes);
    Ok(())
}

/// Records metrics for the size of each file/directory in the data filesystem's directory hierarchy
fn record_tree_size(
    node: fuchsia_inspect::Node,
    dir_proxy: DirectoryProxy,
) -> BoxFuture<'static, Result<TreeSize, Error>> {
    async move {
        let mut total: TreeSize = Default::default();
        for dir_entry in readdir(&dir_proxy).await?.iter() {
            match dir_entry.kind {
                DirentKind::File => {
                    let child = node.create_child(&dir_entry.name);
                    let file_proxy = open_file(&dir_proxy, &dir_entry.name, PERM_READABLE).await?;
                    let (_, immutable_attributes) = file_proxy
                        .get_attributes(
                            fio::NodeAttributesQuery::CONTENT_SIZE
                                | fio::NodeAttributesQuery::STORAGE_SIZE,
                        )
                        .await?
                        .map_err(zx::Status::err_from_raw)?;
                    let size: TreeSize = immutable_attributes.into();
                    size.record(&child);
                    node.record(child);
                    total += size;
                }
                DirentKind::Directory => {
                    let child = node.create_child(&dir_entry.name);
                    let directory_proxy =
                        open_directory(&dir_proxy, &dir_entry.name, PERM_READABLE)
                            .await
                            .context("open_directory failed")?;
                    let size = record_tree_size(child.clone_weak(), directory_proxy).await?;
                    total += size;
                    node.record(child);
                }
                _ => continue,
            }
        }
        total.record(&node);
        Ok(total)
    }
    .boxed()
}

#[derive(Default)]
struct TreeSize {
    content_size: u64,
    storage_size: u64,
}

impl TreeSize {
    // NOTE: Use caution when changing the names of these Inspect properties, there may be out of
    // tree users depending on these values.
    const CONTENT_SIZE_PROP_NAME: &'static str = "size";
    const STORAGE_SIZE_PROP_NAME: &'static str = "storage_size";

    fn record(&self, node: &fuchsia_inspect::Node) {
        node.record_uint(Self::CONTENT_SIZE_PROP_NAME, self.content_size);
        node.record_uint(Self::STORAGE_SIZE_PROP_NAME, self.storage_size);
    }
}

impl From<fio::ImmutableNodeAttributes> for TreeSize {
    fn from(value: fio::ImmutableNodeAttributes) -> Self {
        Self {
            content_size: value.content_size.unwrap_or_default(),
            storage_size: value.storage_size.unwrap_or_default(),
        }
    }
}

impl std::ops::AddAssign for TreeSize {
    fn add_assign(&mut self, rhs: Self) {
        self.content_size += rhs.content_size;
        self.storage_size += rhs.storage_size;
    }
}
