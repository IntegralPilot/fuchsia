// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::io::{Directory, RemoteDirectory};
use crate::path::RemoteComponentStoragePath;
use anyhow::{Result, anyhow, bail};

use flex_client::ProxyHasDomain;
use flex_fuchsia_io as fio;
use flex_fuchsia_sys2::StorageAdminProxy;

/// Create a new directory in a component's storage.
///
/// # Arguments
/// * `storage_admin`: The StorageAdminProxy
/// * `path`: The name of a new directory on the target component
pub async fn make_directory(storage_admin: StorageAdminProxy, path: String) -> Result<()> {
    let remote_path = RemoteComponentStoragePath::parse(&path)?;

    let (dir_proxy, server) = storage_admin.domain().create_proxy::<fio::DirectoryMarker>();
    let server = server.into_channel();
    let storage_dir = RemoteDirectory::from_proxy(dir_proxy);

    if remote_path.relative_path.as_os_str().is_empty() {
        bail!("Remote path cannot be the root");
    }

    // Open the storage
    storage_admin
        .open_component_storage_by_id(&remote_path.instance_id, server.into())
        .await?
        .map_err(|e| anyhow!("Could not open component storage: {:?}", e))?;

    // Send a request to create the directory
    let dir = storage_dir.create_dir(remote_path.relative_path, false)?;

    // Verify that we can actually read the contents of the directory created
    dir.entry_names().await?;

    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use crate::storage::test::{node_to_directory, setup_fake_storage_admin};
    use flex_fuchsia_io as fio;
    use futures::TryStreamExt;

    // TODO(xbhatnag): Replace this mock with something more robust like VFS.
    // Currently VFS is not cross-platform.
    fn setup_fake_directory(mut root_dir: fio::DirectoryRequestStream) {
        fuchsia_async::Task::local(async move {
            // Serve the root directory
            // Root directory should get Open call with CREATE flag
            let request = root_dir.try_next().await;
            let object = if let Ok(Some(fio::DirectoryRequest::Open {
                flags, path, object, ..
            })) = request
            {
                assert_eq!(path, "test");
                assert!(flags.intersects(fio::Flags::FLAG_MAYBE_CREATE));
                assert!(flags.intersects(fio::Flags::PROTOCOL_DIRECTORY));
                object
            } else {
                panic!("did not get open request: {:?}", request);
            };

            // Serve the new test directory
            let mut test_dir = node_to_directory(object.into());

            // Rewind on new directory should succeed
            let request = test_dir.try_next().await;
            if let Ok(Some(fio::DirectoryRequest::Rewind { responder, .. })) = request {
                responder.send(0).unwrap();
            } else {
                panic!("did not get rewind request: {:?}", request)
            }

            // ReadDirents should report no contents in the new directory
            let request = test_dir.try_next().await;
            if let Ok(Some(fio::DirectoryRequest::ReadDirents { responder, .. })) = request {
                responder.send(0, &[]).unwrap();
            } else {
                panic!("did not get readdirents request: {:?}", request)
            }
        })
        .detach();
    }

    #[fuchsia::test]
    async fn test_make_directory() -> Result<()> {
        let storage_admin = setup_fake_storage_admin("123456", setup_fake_directory);
        make_directory(storage_admin, "123456::test".to_string()).await
    }
}
