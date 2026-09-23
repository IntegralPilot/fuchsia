// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result, bail};
use blobfs_ramdisk::BlobfsRamdisk;
use fidl::endpoints::{self, ServerEnd};
use fidl_fuchsia_pkg_test::*;
use fidl_fuchsia_testing_harness::{OperationError, RealmProxy_RequestStream};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::runtime::Dictionary;
use fuchsia_component::server::ServiceFs;
use fuchsia_pkg_testing::{Package, PackageBuilder, SystemImageBuilder};
use futures::{FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use log::{error, info};
use realm_proxy::service::serve_with_proxy;
use {fidl_fuchsia_component_sandbox as fsandbox, fidl_fuchsia_io as fio};

// When this feature is enabled, the pkgdir tests will start Fxblob.
#[cfg(feature = "use_fxblob")]
static BLOB_IMPLEMENTATION: blobfs_ramdisk::Implementation = blobfs_ramdisk::Implementation::Fxblob;

// When this feature is not enabled, the pkgdir tests will start cpp Blobfs.
#[cfg(not(feature = "use_fxblob"))]
static BLOB_IMPLEMENTATION: blobfs_ramdisk::Implementation =
    blobfs_ramdisk::Implementation::CppBlobfs;

enum IncomingService {
    RealmProxy(RealmProxy_RequestStream),
    RealmFactory(RealmFactoryRequestStream),
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), Error> {
    info!("starting");

    // Spin up a blobfs and install the test package.
    let test_package = make_test_package().await;
    let system_image_package =
        SystemImageBuilder::new().static_packages(&[&test_package]).build().await;
    let blobfs = BlobfsRamdisk::builder()
        .implementation(BLOB_IMPLEMENTATION)
        .start()
        .await
        .expect("started blobfs");
    test_package.write_to_blobfs(&blobfs).await;
    system_image_package.write_to_blobfs(&blobfs).await;

    let blobfs_client = blobfs.client();

    serve_and_handle(blobfs_client, test_package.hash()).await?;

    Ok(())
}

async fn serve_and_handle(
    blobfs_client: blobfs::Client,
    hash: &fuchsia_hash::Hash,
) -> Result<(), crate::Error> {
    let (client, server) = endpoints::create_proxy();

    package_directory::serve(
        vfs::execution_scope::ExecutionScope::new(),
        blobfs_client,
        *hash,
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        server,
    )
    .await?;

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingService::RealmProxy);
    fs.dir("svc").add_fidl_service(IncomingService::RealmFactory);
    fs.take_and_serve_directory_handle()?;

    fs.for_each_concurrent(None, move |req| match req {
        IncomingService::RealmProxy(stream) => {
            let realm_proxy = PkgdirTestRealmProxy::new(Clone::clone(&client));
            serve_with_proxy(realm_proxy, stream)
                .unwrap_or_else(|e| error!("handling realm_proxy request stream{:?}", e))
                .boxed()
        }
        IncomingService::RealmFactory(stream) => serve_factory(Clone::clone(&client), stream)
            .unwrap_or_else(|e: crate::Error| error!("handling realm_proxy request stream{:?}", e))
            .boxed(),
    })
    .await;

    Ok(())
}

async fn serve_factory(
    directory: fio::DirectoryProxy,
    mut stream: RealmFactoryRequestStream,
) -> Result<(), crate::Error> {
    let store = connect_to_protocol::<fsandbox::CapabilityStoreMarker>().unwrap();
    let mut dict_id = 1;
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            RealmFactoryRequest::CreateRealm { options, responder } => {
                let pkg_directory_server_end = match options.pkg_directory_server {
                    Some(v) => ServerEnd::<fio::NodeMarker>::from(v.into_channel()),
                    None => {
                        responder.send(Err(OperationError::Failed)).ok();
                        bail!("pkg directory required");
                    }
                };
                if let Err(e) =
                    directory.clone(ServerEnd::new(pkg_directory_server_end.into_channel()))
                {
                    error!("failed to clone: {:?}", e);
                    responder.send(Err(OperationError::Failed)).ok();
                    continue;
                }

                store.dictionary_create(dict_id).await?.unwrap();
                let (my_dictionary, server) = endpoints::create_endpoints();
                store.dictionary_legacy_export(dict_id, server.into()).await.unwrap().unwrap();
                dict_id += 1;

                responder.send(Ok(my_dictionary))?;
            }
            RealmFactoryRequest::CreateRealm2 { options, responder } => {
                let pkg_directory_server_end = match options.pkg_directory_server {
                    Some(v) => ServerEnd::<fio::NodeMarker>::from(v.into_channel()),
                    None => {
                        responder.send(Err(OperationError::Failed)).ok();
                        bail!("pkg directory required");
                    }
                };
                if let Err(e) =
                    directory.clone(ServerEnd::new(pkg_directory_server_end.into_channel()))
                {
                    error!("failed to clone: {:?}", e);
                    responder.send(Err(OperationError::Failed)).ok();
                    continue;
                }

                responder.send(Ok(Dictionary::new().await.handle))?;
            }
            RealmFactoryRequest::_UnknownMethod { .. } => unreachable!(),
        }
    }
    Ok(())
}

/// Constructs a test package to be used in the integration tests.
async fn make_test_package() -> Package {
    let exceeds_max_buf_contents = repeat_by_n('a', (fio::MAX_BUF + 1).try_into().unwrap());

    // Each file's contents is the file's path as bytes for testing simplicity.
    let mut builder = PackageBuilder::new("test-package")
        .add_resource_at("file", "file".as_bytes())
        .add_resource_at("meta/file", "meta/file".as_bytes())
        // For use in testing Directory.Open calls with segmented paths.
        .add_resource_at("dir/file", "dir/file".as_bytes())
        .add_resource_at("dir/dir/file", "dir/dir/file".as_bytes())
        .add_resource_at("dir/dir/dir/file", "dir/dir/dir/file".as_bytes())
        .add_resource_at("meta/dir/file", "meta/dir/file".as_bytes())
        .add_resource_at("meta/dir/dir/file", "meta/dir/dir/file".as_bytes())
        .add_resource_at("meta/dir/dir/dir/file", "meta/dir/dir/dir/file".as_bytes())
        // For use in testing File.Read calls where the file contents exceeds MAX_BUF.
        .add_resource_at("exceeds_max_buf", exceeds_max_buf_contents.as_bytes())
        .add_resource_at("meta/exceeds_max_buf", exceeds_max_buf_contents.as_bytes());

    // For use in testing File.GetBackingMemory on different file sizes.
    let file_sizes = [0, 1, 4095, 4096, 4097];
    for size in &file_sizes {
        builder = builder
            .add_resource_at(
                format!("file_{}", size),
                make_file_contents(*size).collect::<Vec<u8>>().as_slice(),
            )
            .add_resource_at(
                format!("meta/file_{}", size),
                make_file_contents(*size).collect::<Vec<u8>>().as_slice(),
            );
    }

    // Make directory nodes of each kind (root dir, non-meta subdir, meta dir, meta subdir)
    // that overflow the fuchsia.io/Directory.ReadDirents buffer.
    for base in ["", "dir_overflow_readdirents/", "meta/", "meta/dir_overflow_readdirents/"] {
        // In the integration tests, we'll want to be able to test calling ReadDirents on a
        // directory. Since ReadDirents returns `MAX_BUF` bytes worth of directory entries, we need
        // to have test coverage for the "overflow" case where the directory has more than
        // `MAX_BUF` bytes worth of directory entries.
        //
        // Through math, we determine that we can achieve this overflow with 31 files whose names
        // are length `MAX_NAME_LENGTH`. Here is this math:
        /*
           ReadDirents -> vector<uint8>:MAX_BUF

           MAX_BUF = 8192

           struct dirent {
            // Describes the inode of the entry.
            uint64 ino;
            // Describes the length of the dirent name in bytes.
            uint8 size;
            // Describes the type of the entry. Aligned with the
            // POSIX d_type values. Use `DIRENT_TYPE_*` constants.
            uint8 type;
            // Unterminated name of entry.
            char name[0];
           }

           sizeof(dirent) if name is MAX_NAME_LENGTH = 255 bytes long = 8 + 1 + 1 + 255 = 265 bytes

           8192 / 265 ~= 30.9

           => 31 directory entries of maximum size will trigger overflow
        */
        for seed in ('a'..='z').chain('A'..='E') {
            builder = builder.add_resource_at(
                format!("{}{}", base, repeat_by_n(seed, fio::MAX_NAME_LENGTH.try_into().unwrap())),
                &b""[..],
            )
        }
    }
    builder.build().await.expect("build package")
}

fn repeat_by_n(seed: char, n: usize) -> String {
    std::iter::repeat(seed).take(n).collect()
}

fn make_file_contents(size: usize) -> impl Iterator<Item = u8> {
    b"ABCD".iter().copied().cycle().take(size)
}

// A [RealmProxy] implementation that returns clones of the package directory
// for testing. It only responds to requests to connect to fuchsia.io.Directory.
// Any other protocol connection is rejected with [OperationError::Unsupported].
pub struct PkgdirTestRealmProxy {
    directory: fio::DirectoryProxy,
}

impl PkgdirTestRealmProxy {
    pub fn new(directory: fio::DirectoryProxy) -> Self {
        Self { directory }
    }
}

impl realm_proxy::service::RealmProxy for PkgdirTestRealmProxy {
    fn connect_to_named_protocol(
        &mut self,
        protocol: &str,
        server_end: zx::Channel,
    ) -> Result<(), OperationError> {
        if protocol != "fuchsia.io.Directory" {
            error!("this test realm proxy only serves the fuchsia.io.Directory protocol");
            return Err(OperationError::Unsupported);
        }
        self.directory.clone(server_end.into()).map_err(|e| {
            error!("{:?}", e);
            OperationError::Failed
        })
    }

    fn open_service(&self, _service: &str, _server_end: zx::Channel) -> Result<(), OperationError> {
        Err(OperationError::Unsupported)
    }

    fn connect_to_service_instance(
        &self,
        _service: &str,
        _instance: &str,
        _server_end: zx::Channel,
    ) -> Result<(), OperationError> {
        Err(OperationError::Unsupported)
    }
}
