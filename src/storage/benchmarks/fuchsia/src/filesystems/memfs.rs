// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystems::MOUNT_PATH;
use async_trait::async_trait;
use fidl_fuchsia_component::{CreateChildArgs, RealmMarker};
use fs_management::FS_COLLECTION_NAME;
use fuchsia_component::client::{connect_to_protocol, open_childs_exposed_directory};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use storage_benchmarks::{BlockDeviceFactory, Filesystem, FilesystemConfig};
use {fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_io as fio};

/// Config object for starting Memfs instances.
#[derive(Clone)]
pub struct Memfs;

#[async_trait]
impl FilesystemConfig for Memfs {
    type Filesystem = MemfsInstance;

    async fn start_filesystem(
        &self,
        _block_device_factory: &dyn BlockDeviceFactory,
    ) -> MemfsInstance {
        MemfsInstance::new().await
    }

    fn name(&self) -> String {
        "memfs".to_owned()
    }
}

pub struct MemfsInstance {
    instance_name: String,
}

impl MemfsInstance {
    async fn new() -> Self {
        static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let instance_name = format!("memfs-{}", INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed));
        let collection_ref = fdecl::CollectionRef { name: FS_COLLECTION_NAME.to_string() };
        let child_decl = fdecl::Child {
            name: Some(instance_name.to_string()),
            url: Some("#meta/memfs.cm".to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };

        let realm_proxy = connect_to_protocol::<RealmMarker>().unwrap();
        realm_proxy
            .create_child(&collection_ref, &child_decl, CreateChildArgs::default())
            .await
            .expect("Failed to send FIDL request")
            .expect("Failed to create memfs instance");

        let exposed_dir =
            open_childs_exposed_directory(&instance_name, Some(FS_COLLECTION_NAME.to_string()))
                .await
                .expect("Failed to connect to memfs's exposed directory");

        let (root_dir, server_end) = fidl::endpoints::create_endpoints();
        exposed_dir
            .open(
                "memfs",
                fio::PERM_READABLE
                    | fio::Flags::PERM_INHERIT_WRITE
                    | fio::Flags::PERM_INHERIT_EXECUTE,
                &Default::default(),
                server_end.into_channel(),
            )
            .expect("Failed to open memfs's root");
        let namespace = fdio::Namespace::installed().expect("Failed to get local namespace");
        namespace.bind(MOUNT_PATH, root_dir).expect("Failed to bind memfs");

        Self { instance_name }
    }
}

#[async_trait]
impl Filesystem for MemfsInstance {
    async fn shutdown(self) {
        let realm_proxy = connect_to_protocol::<RealmMarker>().unwrap();
        realm_proxy
            .destroy_child(&fdecl::ChildRef {
                name: self.instance_name.clone(),
                collection: Some(FS_COLLECTION_NAME.to_string()),
            })
            .await
            .expect("Failed to send FIDL request")
            .expect("Failed to destroy memfs instance");

        let namespace = fdio::Namespace::installed().expect("Failed to get local namespace");
        namespace.unbind(MOUNT_PATH).expect("Failed to unbind memfs");
    }

    fn benchmark_dir(&self) -> &Path {
        Path::new(MOUNT_PATH)
    }
}

#[cfg(test)]
mod tests {
    use super::Memfs;
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use storage_benchmarks::block_device::PanickingBlockDeviceFactory;
    use storage_benchmarks::{Filesystem, FilesystemConfig};

    #[fuchsia::test]
    async fn start_memfs() {
        const FILE_CONTENTS: &str = "file-contents";
        let block_device_factory = PanickingBlockDeviceFactory::new();
        let fs = Memfs.start_filesystem(&block_device_factory).await;

        let file_path = fs.benchmark_dir().join("filename");
        {
            let mut file =
                OpenOptions::new().create_new(true).write(true).open(&file_path).unwrap();
            file.write_all(FILE_CONTENTS.as_bytes()).unwrap();
        }
        {
            let mut file = OpenOptions::new().read(true).open(&file_path).unwrap();
            let mut buf = [0u8; FILE_CONTENTS.len()];
            file.read_exact(&mut buf).unwrap();
            assert_eq!(std::str::from_utf8(&buf).unwrap(), FILE_CONTENTS);
        }
        fs.shutdown().await;
    }
}
