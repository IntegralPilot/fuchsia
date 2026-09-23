// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_io as fio;
use log::*;
use std::collections::HashMap;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::simple as simpledir;
use vfs::execution_scope::ExecutionScope;
use vfs::file::vmo::VmoFile;

#[cfg(fuchsia_api_level_at_least = "HEAD")]
use {
    fidl::endpoints::DiscoverableProtocolMarker, fidl_fuchsia_component as fcomponent,
    fidl_fuchsia_component_runner as fcrunner,
};

enum DirectoryOrFile {
    Directory(HashMap<String, DirectoryOrFile>),
    File(Arc<zx::Vmo>),
}

/// An `ExecutionScope` does not terminate tasks scheduled on it when the scope is dropped, as many
/// copies of the same `ExecutionScope` can be created. To ensure that cleanup happens at the right
/// time, this struct will call `self.execution_scope.shutdown()` when dropped, causing any tasks
/// running within this scope to stop executing.
struct ExecutionScopeDropper {
    execution_scope: ExecutionScope,
}

impl Drop for ExecutionScopeDropper {
    fn drop(&mut self) {
        self.execution_scope.shutdown();
    }
}

pub async fn read_only_directory(
    directory_name: String,
    directory_contents: Arc<ftest::DirectoryContents>,
    outgoing_dir: ServerEnd<fio::DirectoryMarker>,
) {
    if let Err(e) =
        read_only_directory_helper(directory_name, directory_contents, outgoing_dir).await
    {
        error!("unable to provide read only directory: {:?}", e);
    }
}

async fn read_only_directory_helper(
    directory_name: String,
    directory_contents: Arc<ftest::DirectoryContents>,
    outgoing_dir: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), anyhow::Error> {
    let directory_hashmap = directory_contents_to_hashmap(directory_contents)?;
    let directory = build_directory(directory_hashmap)?;
    let execution_scope_dropper = ExecutionScopeDropper { execution_scope: ExecutionScope::new() };
    let directory_name_clone = directory_name.clone();
    let top_directory = simpledir::Simple::new_with_not_found_handler(move |path| {
        warn!(
            "nonexistent path {:?} accessed in read only directory {:?}",
            path, directory_name_clone
        );
    });
    top_directory.clone().add_entry(&directory_name, directory)?;
    vfs::directory::serve_on(
        top_directory,
        fio::PERM_READABLE,
        execution_scope_dropper.execution_scope.clone(),
        outgoing_dir,
    );
    execution_scope_dropper.execution_scope.wait().await;
    Ok(())
}

fn directory_contents_to_hashmap(
    directory_contents: Arc<ftest::DirectoryContents>,
) -> Result<HashMap<String, DirectoryOrFile>, Error> {
    let mut directory_hashmap = HashMap::new();
    for entry in directory_contents.entries.iter() {
        let mut current_directory = &mut directory_hashmap;
        let mut path_parts_iter = entry.file_path.split('/').peekable();
        while let Some(path_part) = path_parts_iter.next() {
            if path_parts_iter.peek().is_some() {
                let dir_or_file = current_directory
                    .entry(path_part.to_string())
                    .or_insert(DirectoryOrFile::Directory(HashMap::new()));
                current_directory = match dir_or_file {
                    DirectoryOrFile::Directory(d) => d,
                    DirectoryOrFile::File(_) => {
                        return Err(format_err!(
                            "directory_contents invalid, {:?} is inside of a file",
                            entry.file_path
                        ));
                    }
                };
            } else {
                let vmo =
                    Arc::new(entry.file_contents.vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?);
                vmo.set_content_size(&entry.file_contents.size)?;
                current_directory.insert(path_part.to_string(), DirectoryOrFile::File(vmo));
            }
        }
    }
    Ok(directory_hashmap)
}

fn build_directory(
    input: HashMap<String, DirectoryOrFile>,
) -> Result<Arc<simpledir::Simple>, Error> {
    let directory = simpledir::simple();
    for (path, directory_or_file) in input.into_iter() {
        match directory_or_file {
            DirectoryOrFile::Directory(sub_directory) => directory
                .clone()
                .add_entry(&path, build_directory(sub_directory)?)
                .context("could not add directory to directory")?,
            DirectoryOrFile::File(vmo) => {
                let vmo_handle = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
                let file = VmoFile::new(vmo_handle);
                directory
                    .clone()
                    .add_entry(&path, file)
                    .context("could not add file to directory")?;
            }
        }
    }
    Ok(directory)
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
pub async fn storage(
    storage_name: String,
    namespace: Vec<fcrunner::ComponentNamespaceEntry>,
    outgoing_dir: ServerEnd<fio::DirectoryMarker>,
    storage_admin: Option<ServerEnd<fcomponent::StorageAdminMarker>>,
) {
    if let Err(e) = storage_helper(storage_name, namespace, outgoing_dir, storage_admin).await {
        error!("unable to provide storage: {:?}", e);
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
async fn storage_helper(
    storage_name: String,
    namespace: Vec<fcrunner::ComponentNamespaceEntry>,
    outgoing_dir: ServerEnd<fio::DirectoryMarker>,
    storage_admin: Option<ServerEnd<fcomponent::StorageAdminMarker>>,
) -> Result<(), anyhow::Error> {
    let dir_id: u64 = rand::random();
    let dir_path = format!("/data/storage-dir-{:x}", dir_id);

    struct DeleteOnDrop {
        path: String,
    }
    impl Drop for DeleteOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
    let _delete_on_drop = DeleteOnDrop { path: dir_path.clone() };

    let storage_dir_proxy = fuchsia_fs::directory::open_in_namespace(
        &dir_path,
        fio::PERM_READABLE
            | fio::PERM_WRITABLE
            | fio::Flags::FLAG_MUST_CREATE
            | fio::Flags::PROTOCOL_DIRECTORY,
    )
    .context("failed to open temporary storage")?;
    let directory = simpledir::simple();
    directory
        .clone()
        .add_entry(storage_name, vfs::remote::remote_dir(storage_dir_proxy))
        .context("failed to add storage dir proxy to outgoing directory VFS")?;

    let execution_scope_dropper = ExecutionScopeDropper { execution_scope: ExecutionScope::new() };
    vfs::directory::serve_on(
        directory,
        fio::PERM_READABLE | fio::PERM_WRITABLE,
        execution_scope_dropper.execution_scope.clone(),
        outgoing_dir,
    );

    if let Some(storage_admin) = storage_admin {
        let svc_dir_entry = namespace
            .into_iter()
            .find(|entry| entry.path == Some("/svc".to_string()))
            .context("missing svc dir")?;
        let svc_dir_client_end = svc_dir_entry.directory.context("malformed namespace entry")?;
        let svc_dir_proxy = svc_dir_client_end.into_proxy();
        let flags = fio::Flags::PROTOCOL_SERVICE;
        #[cfg(fuchsia_api_level_at_least = "27")]
        let () = svc_dir_proxy
            .open(
                fcomponent::StorageAdminMarker::PROTOCOL_NAME,
                flags,
                &fio::Options::default(),
                storage_admin.into_channel(),
            )
            .map_err(fuchsia_fs::node::OpenError::SendOpenRequest)?;
        #[cfg(not(fuchsia_api_level_at_least = "27"))]
        let () = svc_dir_proxy
            .open3(
                fcomponent::StorageAdminMarker::PROTOCOL_NAME,
                flags,
                &fio::Options::default(),
                storage_admin.into_channel(),
            )
            .map_err(fuchsia_fs::node::OpenError::SendOpenReques)?;
    }

    execution_scope_dropper.execution_scope.wait().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_mem as fmem;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use maplit::hashset;
    use std::collections::HashSet;

    #[fuchsia::test]
    async fn read_only_directory_contains_expected_files() {
        let directory_name = "directory-capability-name".to_string();
        fn create_file_contents(contents: &str) -> fmem::Buffer {
            let vmo = zx::Vmo::create(contents.len() as u64).expect("failed to create vmo");
            vmo.write(contents.as_bytes(), 0).expect("failed to write to vmo");
            fmem::Buffer { vmo, size: contents.len() as u64 }
        }
        let directory_contents = ftest::DirectoryContents {
            entries: vec![
                ftest::DirectoryEntry {
                    file_path: "config/example.json".to_string(),
                    file_contents: create_file_contents("example file contents"),
                },
                ftest::DirectoryEntry {
                    file_path: "a/b/c/d/e/f".to_string(),
                    file_contents: create_file_contents("g"),
                },
                ftest::DirectoryEntry {
                    file_path: "hippos".to_string(),
                    file_contents: create_file_contents("rule!"),
                },
            ],
        };

        let (outgoing_dir_proxy, outgoing_dir_server_end) = create_proxy();

        let _directory_task = fasync::Task::local(read_only_directory(
            directory_name.clone(),
            Arc::new(directory_contents),
            outgoing_dir_server_end,
        ));

        let directory_filenames: HashSet<_> =
            fuchsia_fs::directory::readdir_recursive(&outgoing_dir_proxy, None)
                .map_ok(|dir_entry| dir_entry.name)
                .try_collect()
                .await
                .expect("failed to read directory");
        assert_eq!(
            directory_filenames,
            hashset! {
                "directory-capability-name/config/example.json".to_string(),
                "directory-capability-name/a/b/c/d/e/f".to_string(),
                "directory-capability-name/hippos".to_string(),
            },
        );

        let open_and_read_file = move |file_path: &'static str| {
            let outgoing_dir_proxy = Clone::clone(&outgoing_dir_proxy);
            async move {
                let file_proxy = fuchsia_fs::directory::open_file(
                    &outgoing_dir_proxy,
                    file_path,
                    fuchsia_fs::PERM_READABLE,
                )
                .await
                .unwrap_or_else(|e| panic!("failed to open file {:?}: {:?}", file_path, e));
                fuchsia_fs::file::read_to_string(&file_proxy)
                    .await
                    .unwrap_or_else(|e| panic!("failed to read file {:?}: {:?}", file_path, e))
            }
        };

        assert_eq!(
            open_and_read_file("directory-capability-name/config/example.json").await,
            "example file contents".to_string(),
        );
        assert_eq!(
            open_and_read_file("directory-capability-name/a/b/c/d/e/f").await,
            "g".to_string(),
        );
        assert_eq!(
            open_and_read_file("directory-capability-name/hippos").await,
            "rule!".to_string(),
        );
    }
}
