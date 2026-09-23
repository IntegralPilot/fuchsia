// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fuchsia io conformance testing harness for EROFS

use anyhow::{Context as _, Error};
use erofs_component::{ErofsPager, ErofsVolume};
use erofs_serializer::{SerializerNode, serialize};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_io_test::{
    self as io_test, HarnessConfig, TestHarnessRequest, TestHarnessRequestStream,
};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use std::sync::Arc;

struct Harness(TestHarnessRequestStream);

fn convert_entry(entry: io_test::DirectoryEntry) -> Result<SerializerNode, Error> {
    match entry {
        io_test::DirectoryEntry::Directory(io_test::Directory { name, entries, .. }) => {
            let mut converted_entries = Vec::new();
            for child in entries {
                let child = *child.expect("Directory entries must not be null");
                converted_entries.push(convert_entry(child)?);
            }
            Ok(SerializerNode::Directory { name, entries: converted_entries })
        }
        io_test::DirectoryEntry::File(io_test::File { name, contents, .. }) => {
            Ok(SerializerNode::File { name, data: contents })
        }
        io_test::DirectoryEntry::Symlink(io_test::Symlink { name, target, .. }) => {
            Ok(SerializerNode::Symlink { name, target })
        }
        other => {
            anyhow::bail!("Unsupported entry type: {:?}", other);
        }
    }
}

async fn run(mut stream: TestHarnessRequestStream, pager: Arc<ErofsPager>) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await.context("error running harness server")? {
        match request {
            TestHarnessRequest::GetConfig { responder } => {
                let config = HarnessConfig {
                    // Supported options:
                    supports_get_backing_memory: true,
                    supported_attributes: fio::NodeAttributesQuery::PROTOCOLS
                        | fio::NodeAttributesQuery::ABILITIES
                        | fio::NodeAttributesQuery::CONTENT_SIZE
                        | fio::NodeAttributesQuery::STORAGE_SIZE
                        | fio::NodeAttributesQuery::ID
                        | fio::NodeAttributesQuery::MODE
                        | fio::NodeAttributesQuery::UID
                        | fio::NodeAttributesQuery::GID
                        | fio::NodeAttributesQuery::LINK_COUNT
                        | fio::NodeAttributesQuery::CREATION_TIME
                        | fio::NodeAttributesQuery::MODIFICATION_TIME
                        | fio::NodeAttributesQuery::ACCESS_TIME
                        | fio::NodeAttributesQuery::CHANGE_TIME,
                    supports_symlinks: true,
                    // Unsupported options:
                    supports_executable_file: false,
                    supports_remote_dir: false,
                    supports_services: false,
                    supports_link_into: false,
                    supports_get_token: false,
                    supports_append: false,
                    supports_truncate: false,
                    supports_modify_directory: false,
                    supports_mutable_file: false,
                    supports_unnamed_temporary_file: false,
                    // TODO(https://fxbug.dev/479841115): support xattrs in erofs
                    supports_xattrs: false,
                };
                responder.send(&config)?;
            }
            TestHarnessRequest::CreateDirectory {
                contents,
                flags,
                object_request,
                control_handle: _,
            } => {
                // 1. Convert fidl entries to serializer nodes
                let mut serializer_nodes = Vec::new();
                for entry in contents {
                    if let Some(entry) = entry {
                        serializer_nodes.push(convert_entry(*entry)?);
                    }
                }

                // 2. Serialize tree to raw bytes
                let image = serialize(&serializer_nodes);

                // 3. Set up EROFS volume
                let vmo = zx::Vmo::create(image.len() as u64).context("Failed to create VMO")?;
                vmo.write(&image, 0).context("Failed to write VMO")?;
                ErofsVolume::serve(vmo, pager.clone(), flags, object_request)
                    .context("failed to serve volume")?;
            }
            TestHarnessRequest::OpenServiceDirectory { responder: _ } => {
                panic!("EROFS does not support service directories")
            }
        }
    }

    Ok(())
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(Harness);
    fs.take_and_serve_directory_handle()?;

    let pager = Arc::new(ErofsPager::new().context("Failed to create ErofsPager")?);

    let fut = fs.for_each_concurrent(10_000, |Harness(stream)| {
        let pager = pager.clone();
        run(stream, pager).unwrap_or_else(|e| log::error!("Error processing request: {:?}", e))
    });

    fut.await;
    Ok(())
}
