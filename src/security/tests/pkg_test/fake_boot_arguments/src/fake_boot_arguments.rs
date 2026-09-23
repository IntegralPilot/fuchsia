// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runtime as fruntime;
use fuchsia_component::runtime::{Data, DataValue, Dictionary, DictionaryRouterReceiver};
use futures::{FutureExt as _, StreamExt as _};
use log::info;
use std::sync::Arc;

static PKGFS_BOOT_ARG_VALUE_PREFIX: &'static str = "bin/pkgsvr+";

/// Flags for fake_boot_arguments.
#[derive(argh::FromArgs, Debug, PartialEq)]
pub struct Args {
    /// absolute path to system_image package file.
    #[argh(option)]
    system_image_path: String,
}

async fn initialize_dictionary(value: &str) -> Result<Dictionary> {
    let dictionary = Dictionary::new().await;
    let config = fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(value.to_string()));
    let data = Data::new(DataValue::Bytes(fidl::persist(&config)?)).await;

    let key = "fuchsia.zircon.system.pkgfs.cmd";
    dictionary.insert(key, data).await;

    Ok(dictionary)
}

enum BootServices {
    Items(fidl_fuchsia_boot::ItemsRequestStream),
    Router(fruntime::DictionaryRouterRequestStream),
}

#[fuchsia::main]
async fn main() {
    info!("Starting fake_boot_arguments...");
    let args @ Args { system_image_path } = &argh::from_env();
    info!(args:?; "Initalizing fake_boot_arguments");

    let system_image = fuchsia_fs::file::read(
        &fuchsia_fs::file::open_in_namespace(system_image_path.as_str(), fuchsia_fs::PERM_READABLE)
            .unwrap(),
    )
    .await
    .unwrap();

    let system_image_merkle = fuchsia_merkle::root_from_slice(&system_image);
    let pkgfs_boot_arg_value = format!("{}{}", PKGFS_BOOT_ARG_VALUE_PREFIX, system_image_merkle);

    let dictionary = initialize_dictionary(&pkgfs_boot_arg_value).await.unwrap();

    let mut fs = fuchsia_component::server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(BootServices::Items);
    fs.dir("svc").add_fidl_service(BootServices::Router);
    fs.take_and_serve_directory_handle().unwrap();

    fs.for_each_concurrent(None, move |stream| {
        let dictionary = dictionary.clone();
        async move {
            match stream {
                BootServices::Items(stream) => {
                    // The VMO provided here would be for the recovery case only, which isn't of interest for pkg_test.
                    run_boot_items(stream, None).await
                }
                BootServices::Router(stream) => {
                    let dictionary = dictionary.clone();
                    DictionaryRouterReceiver::from(stream)
                        .handle_with(move |_request, _instance_token| {
                            futures::future::ready(Ok(Some(dictionary.clone()))).boxed()
                        })
                        .await;
                }
            }
        }
    })
    .await;
}

// Mocks for fshost, from https://cs.opensource.google/fuchsia/fuchsia/+/main:src/storage/fshost/integration/src/mocks.rs
// fshost uses exactly one boot item - it checks to see if there is an item of type
// ZBI_TYPE_STORAGE_RAMDISK. If it's there, it's a vmo that represents a ramdisk version of the
// fvm, and fshost creates a ramdisk from the vmo so it can go through the normal device matching.
async fn run_boot_items(
    mut stream: fidl_fuchsia_boot::ItemsRequestStream,
    vmo: Option<Arc<zx::Vmo>>,
) {
    while let Some(request) = stream.next().await {
        match request.unwrap() {
            fidl_fuchsia_boot::ItemsRequest::Get { type_, extra, responder } => {
                assert_eq!(type_, zbi::Type::StorageRamdisk as u32);
                assert_eq!(extra, 0);
                let response_vmo = vmo.as_ref().map(|vmo| {
                    vmo.create_child(zx::VmoChildOptions::SLICE, 0, vmo.get_size().unwrap())
                        .unwrap()
                });
                responder.send(response_vmo, 0).unwrap();
            }
            fidl_fuchsia_boot::ItemsRequest::Get2 { type_, extra, responder } => {
                assert_eq!(type_, zbi::Type::StorageRamdisk as u32);
                assert_eq!((*extra.unwrap()).n, 0);
                responder.send(Ok(Vec::new())).unwrap();
            }
            fidl_fuchsia_boot::ItemsRequest::GetBootloaderFile { .. } => {
                panic!(
                    "unexpectedly called GetBootloaderFile on {}",
                    fidl_fuchsia_boot::ItemsMarker::PROTOCOL_NAME
                );
            }
        }
    }
}
