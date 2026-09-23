// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::filesystems::{BlobFilesystem, Blobfs, CacheClearableFilesystem, Fxblob};
use async_trait::async_trait;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_fxfs::{BlobCreatorProxy, BlobReaderProxy};
use fidl_fuchsia_io as fio;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::future::FutureExt;
use std::path::Path;
use storage_benchmarks::{BlockDeviceFactory, Filesystem, FilesystemConfig};
/// Config object for starting a `PkgDirInstance`. The `PkgDirInstance` allows blob benchmarks to
/// open and read a blob through its package directory as opposed to talking directly to the
/// filesystem.
#[derive(Clone)]
pub struct PkgDirTest {
    use_fxblob: bool,
}

impl PkgDirTest {
    pub fn new_fxblob() -> Self {
        PkgDirTest { use_fxblob: true }
    }

    pub fn new_blobfs() -> Self {
        PkgDirTest { use_fxblob: false }
    }
}

#[async_trait]
impl FilesystemConfig for PkgDirTest {
    type Filesystem = PkgDirInstance;

    async fn start_filesystem(
        &self,
        block_device_factory: &dyn BlockDeviceFactory,
    ) -> PkgDirInstance {
        let fs = if self.use_fxblob {
            Box::new(Fxblob.start_filesystem(block_device_factory).await) as Box<dyn BlobFilesystem>
        } else {
            Box::new(Blobfs.start_filesystem(block_device_factory).await) as Box<dyn BlobFilesystem>
        };

        let (clone, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        fs.exposed_dir()
            .clone(server_end.into_channel().into())
            .expect("connect to blob volume exposed dir");
        let realm = PkgDirRealm::new(self.use_fxblob, clone).await;
        PkgDirInstance { fs, realm, use_fxblob: self.use_fxblob }
    }

    fn name(&self) -> String {
        let fs = if self.use_fxblob { "fxblob" } else { "blobfs" };
        format!("{}-pkgdir", fs)
    }
}

pub struct PkgDirInstance {
    fs: Box<dyn BlobFilesystem>,
    realm: PkgDirRealm,
    use_fxblob: bool,
}

impl PkgDirInstance {
    pub fn pkgdir_proxy(&self) -> fidl_test_pkgdir::PkgDirProxy {
        self.realm.realm().root.connect_to_protocol_at_exposed_dir().unwrap()
    }
}

#[async_trait]
impl Filesystem for PkgDirInstance {
    async fn shutdown(self) {
        self.fs.shutdown_boxed().await
    }

    fn benchmark_dir(&self) -> &Path {
        self.fs.benchmark_dir()
    }
}

#[async_trait]
impl CacheClearableFilesystem for PkgDirInstance {
    async fn clear_cache(&mut self) {
        self.fs.clear_cache().await;
        let (clone, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        self.fs
            .exposed_dir()
            .clone(server_end.into_channel().into())
            .expect("connect to blob volume exposed dir");
        self.realm = PkgDirRealm::new(self.use_fxblob, clone).await
    }
}

#[async_trait]
impl BlobFilesystem for PkgDirInstance {
    fn blob_creator(&self) -> &BlobCreatorProxy {
        self.fs.blob_creator()
    }

    fn blob_reader(&self) -> &BlobReaderProxy {
        self.fs.blob_reader()
    }

    fn exposed_dir(&self) -> &fio::DirectoryProxy {
        self.fs.exposed_dir()
    }
}

pub struct PkgDirRealm {
    pub realm: RealmInstance,
}

impl PkgDirRealm {
    pub async fn new(fxblob: bool, exposed_dir: fio::DirectoryProxy) -> Self {
        let builder = RealmBuilder::new().await.unwrap();
        let pkgdir = builder
            .add_child("pkgdir-component", "#meta/pkgdir-component.cm", ChildOptions::new())
            .await
            .unwrap();
        builder.init_mutable_config_from_package(&pkgdir).await.unwrap();
        let exposed_dir = vfs::pseudo_directory! {
            "blob" => vfs::remote::remote_dir(exposed_dir),
        };
        let service_reflector = builder
            .add_local_child(
                "service_reflector",
                move |handles| {
                    let scope = vfs::execution_scope::ExecutionScope::new();
                    vfs::directory::serve_on(
                        exposed_dir.clone(),
                        fio::PERM_READABLE,
                        scope.clone(),
                        handles.outgoing_dir,
                    );
                    async move {
                        scope.wait().await;
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_test_pkgdir::PkgDirMarker>())
                    .from(&pkgdir)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol::<fidl_fuchsia_tracing_provider::RegistryMarker>(),
                    )
                    .from(Ref::parent())
                    .to(&pkgdir),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("blob-exec")
                            .path("/blob/root")
                            .rights(fio::R_STAR_DIR),
                    )
                    .from(&service_reflector)
                    .to(&pkgdir),
            )
            .await
            .unwrap();
        let svc_path = if fxblob {
            format!("/blob/svc/{}", fidl_fuchsia_fxfs::BlobReaderMarker::PROTOCOL_NAME)
        } else {
            format!("/blob/{}", fidl_fuchsia_fxfs::BlobReaderMarker::PROTOCOL_NAME)
        };
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol::<fidl_fuchsia_fxfs::BlobReaderMarker>()
                            .path(svc_path),
                    )
                    .from(&service_reflector)
                    .to(&pkgdir),
            )
            .await
            .unwrap();
        let realm = builder.build().await.expect("realm build failed");
        Self { realm }
    }

    fn realm(&self) -> &RealmInstance {
        &self.realm
    }
}
