// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::UtilsError;
use fidl::endpoints::{ClientEnd, create_endpoints, create_proxy};
use fidl_fuchsia_io as fio;

pub async fn open_pkg_file(
    pkg_dir: &fio::DirectoryProxy,
    relative_binary_path: &str,
) -> Result<zx::Vmo, UtilsError> {
    let (file, server) = create_proxy::<fio::FileMarker>();
    pkg_dir.open(
        relative_binary_path,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        &fio::Options::default(),
        server.into_channel(),
    )?;

    let vmo = file
        .get_backing_memory(
            fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | fio::VmoFlags::PRIVATE_CLONE,
        )
        .await??;
    Ok(vmo)
}

pub fn open_lib_dir(
    pkg_dir: &fio::DirectoryProxy,
) -> Result<ClientEnd<fio::DirectoryMarker>, UtilsError> {
    let (lib_dir, server) = create_endpoints::<fio::DirectoryMarker>();
    pkg_dir.open(
        "lib",
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        &fio::Options::default(),
        server.into_channel(),
    )?;
    Ok(lib_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{ServerEnd, create_proxy_and_stream};
    use futures::StreamExt;
    use vfs::directory::entry_container::Directory;
    use vfs::execution_scope::ExecutionScope;
    use vfs::object_request::ToObjectRequest;
    use vfs::pseudo_directory;

    #[fuchsia::test]
    async fn test_open_pkg_file() {
        let (pkg_client, mut pkg_stream) = create_proxy_and_stream::<fio::DirectoryMarker>();

        fuchsia_async::Task::local(async move {
            while let Some(Ok(request)) = pkg_stream.next().await {
                if let fio::DirectoryRequest::Open { flags, path, object, .. } = request {
                    assert_eq!(path, "bin/driver");
                    assert!(flags.contains(fio::Flags::PERM_READ_BYTES | fio::Flags::PERM_EXECUTE));

                    let server_end = ServerEnd::<fio::FileMarker>::new(object);
                    let mut stream = server_end.into_stream();

                    while let Some(Ok(request)) = stream.next().await {
                        if let fio::FileRequest::GetBackingMemory { flags: _, responder } = request
                        {
                            let vmo = zx::Vmo::create(12).unwrap();
                            vmo.write(b"test_content", 0).unwrap();
                            responder.send(Ok(vmo)).unwrap();
                        }
                    }
                }
            }
        })
        .detach();

        let vmo = open_pkg_file(&pkg_client, "bin/driver").await.expect("Failed to open pkg file");
        let mut buffer = vec![0; 12];
        vmo.read(&mut buffer, 0).expect("Failed to read vmo");
        assert_eq!(&buffer, b"test_content");
    }

    #[fuchsia::test]
    async fn test_open_lib_dir() {
        let pkg_dir = pseudo_directory! {
            "lib" => pseudo_directory! {},
        };

        let (pkg_client, pkg_server) = create_proxy::<fio::DirectoryMarker>();
        let scope = ExecutionScope::new();
        let flags = fio::PERM_READABLE | fio::PERM_EXECUTABLE;
        let mut object_request = flags.to_object_request(pkg_server.into_channel());
        pkg_dir
            .open(scope, vfs::path::Path::dot(), flags, &mut object_request)
            .expect("Failed to open");

        let lib_dir = open_lib_dir(&pkg_client).expect("Failed to open lib dir");
        let lib_proxy = lib_dir.into_proxy();

        // Verify we can interact with the lib directory
        let _attributes = lib_proxy
            .get_attributes(fio::NodeAttributesQuery::empty())
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("Failed to get attributes");
    }
}
