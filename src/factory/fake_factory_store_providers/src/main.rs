// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use clap::Parser;
use fidl_fuchsia_factory::{
    AlphaFactoryStoreProviderRequest, AlphaFactoryStoreProviderRequestStream,
    CastCredentialsFactoryStoreProviderRequest, CastCredentialsFactoryStoreProviderRequestStream,
    MiscFactoryStoreProviderRequest, MiscFactoryStoreProviderRequestStream,
    PlayReadyFactoryStoreProviderRequest, PlayReadyFactoryStoreProviderRequestStream,
    WeaveFactoryStoreProviderRequest, WeaveFactoryStoreProviderRequestStream,
    WidevineFactoryStoreProviderRequest, WidevineFactoryStoreProviderRequestStream,
};
use fidl_fuchsia_io as fio;
use fuchsia_component::server::ServiceFs;
use futures::lock::Mutex;
use futures::prelude::*;
use serde_json::from_reader;
use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;
use vfs::file::vmo::read_only;
use vfs::tree_builder::TreeBuilder;

type LockedDirectoryProxy = Arc<Mutex<fio::DirectoryProxy>>;

enum IncomingServices {
    AlphaFactoryStoreProvider(AlphaFactoryStoreProviderRequestStream),
    CastCredentialsFactoryStoreProvider(CastCredentialsFactoryStoreProviderRequestStream),
    MiscFactoryStoreProvider(MiscFactoryStoreProviderRequestStream),
    PlayReadyFactoryStoreProvider(PlayReadyFactoryStoreProviderRequestStream),
    WeaveFactoryStoreProvider(WeaveFactoryStoreProviderRequestStream),
    WidevineFactoryStoreProvider(WidevineFactoryStoreProviderRequestStream),
}

fn start_test_dir(config_path: &str) -> fio::DirectoryProxy {
    let files: HashMap<String, String> = match File::open(&config_path) {
        Ok(file) => from_reader(file).unwrap(),
        Err(err) => {
            log::warn!("publishing empty directory for {} due to error: {:?}", config_path, err);
            HashMap::new()
        }
    };

    log::info!("Files from {}: {:?}", config_path, files);

    let mut tree = TreeBuilder::empty_dir();

    for (name, contents) in files.into_iter() {
        tree.add_entry(&name.split("/").collect::<Vec<&str>>(), read_only(contents)).unwrap();
    }

    let test_dir = tree.build();
    vfs::directory::serve_read_only(test_dir, vfs::execution_scope::ExecutionScope::new())
}

async fn run_server(req: IncomingServices, dir_mtx: LockedDirectoryProxy) -> Result<(), Error> {
    match req {
        IncomingServices::AlphaFactoryStoreProvider(mut stream) => {
            while let Some(request) = stream.try_next().await? {
                let AlphaFactoryStoreProviderRequest::GetFactoryStore { dir, control_handle: _ } =
                    request;
                dir_mtx.lock().await.clone(dir.into_channel().into())?;
            }
        }
        IncomingServices::CastCredentialsFactoryStoreProvider(mut stream) => {
            while let Some(request) = stream.try_next().await? {
                let CastCredentialsFactoryStoreProviderRequest::GetFactoryStore {
                    dir,
                    control_handle: _,
                } = request;
                dir_mtx.lock().await.clone(dir.into_channel().into())?;
            }
        }
        IncomingServices::MiscFactoryStoreProvider(mut stream) => {
            while let Some(request) = stream.try_next().await? {
                let MiscFactoryStoreProviderRequest::GetFactoryStore { dir, control_handle: _ } =
                    request;
                dir_mtx.lock().await.clone(dir.into_channel().into())?;
            }
        }
        IncomingServices::PlayReadyFactoryStoreProvider(mut stream) => {
            while let Some(request) = stream.try_next().await? {
                let PlayReadyFactoryStoreProviderRequest::GetFactoryStore {
                    dir,
                    control_handle: _,
                } = request;
                dir_mtx.lock().await.clone(dir.into_channel().into())?;
            }
        }
        IncomingServices::WeaveFactoryStoreProvider(mut stream) => {
            while let Some(request) = stream.try_next().await? {
                let WeaveFactoryStoreProviderRequest::GetFactoryStore { dir, control_handle: _ } =
                    request;
                dir_mtx.lock().await.clone(dir.into_channel().into())?;
            }
        }
        IncomingServices::WidevineFactoryStoreProvider(mut stream) => {
            while let Some(request) = stream.try_next().await? {
                let WidevineFactoryStoreProviderRequest::GetFactoryStore { dir, control_handle: _ } =
                    request;
                dir_mtx.lock().await.clone(dir.into_channel().into())?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum Provider {
    Alpha,
    Cast,
    Misc,
    Playready,
    Weave,
    Widevine,
}

#[derive(Debug, Parser)]
struct Flags {
    /// The factory store provider to fake.
    #[arg(short, long, ignore_case = true)]
    provider: Provider,

    /// The path to the config file for the provider.
    #[arg(short, long)]
    config: String,
}

#[fuchsia::main(logging_tags = ["fake_factory_store_providers"])]
async fn main() -> Result<(), Error> {
    let flags = Flags::parse();
    let dir = Arc::new(Mutex::new(start_test_dir(&flags.config)));

    let mut fs = ServiceFs::new_local();
    let mut fs_dir = fs.dir("svc");

    match flags.provider {
        Provider::Alpha => fs_dir.add_fidl_service(IncomingServices::AlphaFactoryStoreProvider),
        Provider::Cast => {
            fs_dir.add_fidl_service(IncomingServices::CastCredentialsFactoryStoreProvider)
        }
        Provider::Misc => fs_dir.add_fidl_service(IncomingServices::MiscFactoryStoreProvider),
        Provider::Playready => {
            fs_dir.add_fidl_service(IncomingServices::PlayReadyFactoryStoreProvider)
        }
        Provider::Weave => fs_dir.add_fidl_service(IncomingServices::WeaveFactoryStoreProvider),
        Provider::Widevine => {
            fs_dir.add_fidl_service(IncomingServices::WidevineFactoryStoreProvider)
        }
    };

    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(10, |req| {
        run_server(req, dir.clone()).unwrap_or_else(|e| log::error!("{:?}", e))
    })
    .await;
    Ok(())
}
