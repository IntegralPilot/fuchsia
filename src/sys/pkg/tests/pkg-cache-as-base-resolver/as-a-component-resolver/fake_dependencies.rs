// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServerEnd};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics as fmetrics;
use fuchsia_component::client;
use futures::stream::TryStreamExt as _;
use log::{info, warn};
use mock_metrics::MockMetricEventLoggerFactory;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable as _;

// When this feature is enabled, the base-resolver integration tests will start Fxblob.
#[cfg(feature = "use_fxblob")]
static BLOB_IMPLEMENTATION: blobfs_ramdisk::Implementation = blobfs_ramdisk::Implementation::Fxblob;

// When this feature is not enabled, the base-resolver integration tests will start cpp Blobfs.
#[cfg(not(feature = "use_fxblob"))]
static BLOB_IMPLEMENTATION: blobfs_ramdisk::Implementation =
    blobfs_ramdisk::Implementation::CppBlobfs;

const OUT_DIR_FLAGS: fio::Flags =
    fio::PERM_READABLE.union(fio::PERM_WRITABLE).union(fio::PERM_EXECUTABLE);

async fn initialize_dictionary(
    store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    value: &str,
) -> Result<u64> {
    let dict_id = id_gen.next();
    store
        .dictionary_create(dict_id)
        .await?
        .map_err(|e| anyhow!("failed dictionary_create: {e:?}"))?;

    let key = "fuchsia.zircon.system.pkgfs.cmd";

    let config_id = id_gen.next();
    let config = fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(value.to_string()));
    let config = fsandbox::Capability::Data(fsandbox::Data::Bytes(fidl::persist(&config)?));
    store.import(config_id, config).await?.map_err(|e| anyhow!("failed import: {e:?}"))?;
    store
        .dictionary_insert(
            dict_id,
            &fsandbox::DictionaryItem { key: key.to_string(), value: config_id },
        )
        .await?
        .map_err(|e| anyhow!("failed dictionary_insert: {e:?}"))?;

    Ok(dict_id)
}

async fn run_dictionary_router(
    dict_id: u64,
    store: fsandbox::CapabilityStoreProxy,
    id_gen: sandbox::CapabilityIdGenerator,
    mut stream: fsandbox::DictionaryRouterRequestStream,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fsandbox::DictionaryRouterRequest::Route { payload: _, responder } => {
                let dup_dict_id = id_gen.next();
                store.duplicate(dict_id, dup_dict_id).await.unwrap().unwrap();
                let capability = store.export(dup_dict_id).await.unwrap().unwrap();
                let fsandbox::Capability::Dictionary(dict) = capability else {
                    panic!("capability was not a dictionary? {capability:?}");
                };
                let _ =
                    responder.send(Ok(fsandbox::DictionaryRouterRouteResponse::Dictionary(dict)));
            }
            fsandbox::DictionaryRouterRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown DictionaryRouter request");
            }
        }
    }
}

#[fuchsia::main]
async fn main() {
    info!("started");
    let this_pkg = fuchsia_pkg_testing::Package::identity().await.unwrap();
    let static_packages =
        system_image::StaticPackages::from([("mock-package/0".parse().unwrap(), *this_pkg.hash())]);
    let mut static_packages_bytes = vec![];
    static_packages.serialize(&mut static_packages_bytes).expect("write static_packages");
    let system_image = fuchsia_pkg_testing::PackageBuilder::new("system_image")
        .add_resource_at("data/static_packages", static_packages_bytes.as_slice())
        .build()
        .await
        .expect("build system_image package");
    let blobfs = blobfs_ramdisk::BlobfsRamdisk::builder()
        .implementation(BLOB_IMPLEMENTATION)
        .start()
        .await
        .unwrap();
    let () = system_image.write_to_blobfs(&blobfs).await;
    let () = this_pkg.write_to_blobfs_ignore_subpackages(&blobfs).await;
    let the_subpackage = fuchsia_pkg_testing::Package::from_dir("/the-subpackage").await.unwrap();
    let () = the_subpackage.write_to_blobfs(&blobfs).await;

    let pkgfs_boot_arg_value = format!("bin/pkgsvr+{}", *system_image.hash());
    let store = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>().unwrap();
    let id_gen = sandbox::CapabilityIdGenerator::new();

    let dict_id = initialize_dictionary(&store, &id_gen, &pkgfs_boot_arg_value).await.unwrap();

    // Use VFS because ServiceFs does not support PERM_EXECUTABLE, but /blob needs it.
    let out_dir = vfs::pseudo_directory! {
        "svc" => vfs::pseudo_directory! {
            fmetrics::MetricEventLoggerFactoryMarker::PROTOCOL_NAME =>
                vfs::service::host(move |stream| {
                    Arc::new(MockMetricEventLoggerFactory::new()).run_logger_factory(stream)
                }),
            fsandbox::DictionaryRouterMarker::PROTOCOL_NAME => {
                vfs::service::host(move |stream| {
                    run_dictionary_router(dict_id, store.clone(), id_gen.clone(), stream) })
            }
        },
        "blob" =>
            vfs::remote::remote_dir(blobfs.root_dir_proxy().expect("get blobfs root dir")),
    };

    out_dir
        .add_entry(
            "fxfs-svc",
            vfs::remote::remote_dir(blobfs.svc_dir().expect("get blobfs svc dir")),
        )
        .unwrap();

    let scope = vfs::execution_scope::ExecutionScope::new();
    let dir_server: ServerEnd<fio::DirectoryMarker> =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::DirectoryRequest.into())
            .unwrap()
            .into();
    vfs::directory::serve_on(out_dir, OUT_DIR_FLAGS, scope.clone(), dir_server);
    let () = scope.wait().await;
}
