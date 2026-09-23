// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod config;
mod validators;

use anyhow::{Error, format_err};
use config::{Config, ConfigContext, FactoryConfig};
use fidl::endpoints::{ProtocolMarker, Request, RequestStream, create_proxy};
use fidl_fuchsia_boot::FactoryItemsMarker;
use fidl_fuchsia_factory::{
    AlphaFactoryStoreProviderMarker, AlphaFactoryStoreProviderRequest,
    AlphaFactoryStoreProviderRequestStream, CastCredentialsFactoryStoreProviderMarker,
    CastCredentialsFactoryStoreProviderRequest, CastCredentialsFactoryStoreProviderRequestStream,
    MiscFactoryStoreProviderMarker, MiscFactoryStoreProviderRequest,
    MiscFactoryStoreProviderRequestStream, PlayReadyFactoryStoreProviderMarker,
    PlayReadyFactoryStoreProviderRequest, PlayReadyFactoryStoreProviderRequestStream,
    WeaveFactoryStoreProviderMarker, WeaveFactoryStoreProviderRequest,
    WeaveFactoryStoreProviderRequestStream, WidevineFactoryStoreProviderMarker,
    WidevineFactoryStoreProviderRequest, WidevineFactoryStoreProviderRequestStream,
};
use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_io as fio;
use fuchsia_bootfs::BootfsParser;
use fuchsia_component::server::ServiceFs;
use futures::lock::Mutex;
use futures::{StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use std::io;
use std::sync::Arc;
use vfs::directory;
use vfs::file::vmo::read_only;
use vfs::tree_builder::TreeBuilder;

const CONCURRENT_LIMIT: usize = 10_000;
const DEFAULT_BOOTFS_FACTORY_ITEM_EXTRA: u32 = 0;
const FACTORY_DEVICE_CONFIG: &'static str = "/config/data/factory.config";

enum IncomingServices {
    AlphaFactoryStoreProvider(AlphaFactoryStoreProviderRequestStream),
    CastCredentialsFactoryStoreProvider(CastCredentialsFactoryStoreProviderRequestStream),
    MiscFactoryStoreProvider(MiscFactoryStoreProviderRequestStream),
    PlayReadyFactoryStoreProvider(PlayReadyFactoryStoreProviderRequestStream),
    WeaveFactoryStoreProvider(WeaveFactoryStoreProviderRequestStream),
    WidevineFactoryStoreProvider(WidevineFactoryStoreProviderRequestStream),
}

async fn parse_bootfs<'a>(vmo: zx::Vmo) -> Arc<directory::immutable::Simple> {
    let mut tree_builder = TreeBuilder::empty_dir();
    let mut has_invalid_factory_data = false;

    match BootfsParser::create_from_vmo(vmo) {
        Ok(parser) => parser.iter().for_each(|result| match result {
            Ok(entry) => {
                log::info!("Found {} in factory bootfs", entry.name);

                let name = entry.name;
                let path_parts: Vec<&str> = name.split("/").collect();
                let payload = entry.payload;
                tree_builder
                    .add_entry(
                        &path_parts,
                        read_only(payload.unwrap_or_else(|| {
                            log::error!("Failed to buffer bootfs entry {}", name);
                            has_invalid_factory_data = true;
                            Vec::new()
                        })),
                    )
                    .unwrap_or_else(|err| {
                        log::error!("Failed to add bootfs entry {} to directory: {}", name, err);
                        has_invalid_factory_data = true;
                    });
            }
            Err(err) => {
                log::error!("BootfsParser: {}", err);
                has_invalid_factory_data = true;
            }
        }),
        Err(err) => {
            log::error!("BootfsParser: {}", err);
            has_invalid_factory_data = true;
        }
    };

    if has_invalid_factory_data {
        file_crash_report(CORRUPT_FACTORY_DATA_CRASH_SIGNATURE).await;
    }

    tree_builder.build()
}

async fn fetch_new_factory_item() -> Result<zx::Vmo, Error> {
    let factory_items = fuchsia_component::client::connect_to_protocol::<FactoryItemsMarker>()?;
    let (vmo_opt, _) = factory_items.get(DEFAULT_BOOTFS_FACTORY_ITEM_EXTRA).await?;
    vmo_opt.ok_or_else(|| format_err!("Failed to get a valid VMO from service"))
}

async fn read_file_from_proxy<'a>(
    dir_proxy: &'a fio::DirectoryProxy,
    file_path: &'a str,
) -> Result<Vec<u8>, Error> {
    let file = fuchsia_fs::directory::open_file_async(&dir_proxy, file_path, fio::PERM_READABLE)?;
    fuchsia_fs::file::read(&file).await.map_err(Into::into)
}

fn load_config_file(path: &str) -> Result<FactoryConfig, Error> {
    FactoryConfig::load(io::BufReader::new(std::fs::File::open(path)?))
}

async fn create_dir_from_context<'a>(
    context: &'a ConfigContext,
    dir: &'a fio::DirectoryProxy,
) -> Arc<directory::immutable::Simple> {
    let mut tree_builder = TreeBuilder::empty_dir();

    for (path, dest) in &context.file_path_map {
        let contents = match read_file_from_proxy(dir, path).await {
            Ok(contents) => contents,
            Err(_) => {
                log::error!("Failed to find {}, skipping", path);
                continue;
            }
        };

        let mut failed_validation = false;
        let mut validated = false;

        for validator_context in &context.validator_contexts {
            if validator_context.paths_to_validate.contains(path) {
                log::info!("Validating {} with {} validator", path, validator_context.name);
                if let Err(err) = validator_context.validator.validate(&path, &contents[..]) {
                    log::error!("{}", err);
                    failed_validation = true;
                    break;
                }
                validated = true;
            }
        }

        // Do not allow files that failed validation or have not been validated at all.
        if !failed_validation && validated {
            let path_parts: Vec<&str> = dest.split("/").collect();
            let file = read_only(contents);
            tree_builder.add_entry(&path_parts, file).unwrap_or_else(|err| {
                log::error!("Failed to add file {} to directory: {}", dest, err);
            });
        } else if !validated {
            log::error!("{} was never validated, ignored", path);
        }
    }

    tree_builder.build()
}

async fn apply_config(
    config: Config,
    dir_mtx: Arc<Mutex<fio::DirectoryProxy>>,
) -> fio::DirectoryProxy {
    // We only want to hold this lock to create `dir` so limit the scope of `dir_ref`.
    let dir = {
        let dir_ref = dir_mtx.lock().await;
        let context = config.into_context().expect("Failed to convert config into context");
        create_dir_from_context(&context, &*dir_ref).await
    };
    vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new())
}

async fn handle_request_stream<RS, G>(
    mut stream: RS,
    directory_mutex: Arc<Mutex<fio::DirectoryProxy>>,
    mut get_directory_request_fn: G,
) -> Result<(), Error>
where
    RS: RequestStream,
    G: FnMut(Request<RS::Protocol>) -> Option<fidl::endpoints::ServerEnd<fio::DirectoryMarker>>,
{
    while let Some(request) = stream.try_next().await? {
        if let Some(server_end) = get_directory_request_fn(request) {
            if let Err(err) = directory_mutex.lock().await.clone(server_end.into_channel().into()) {
                log::error!(
                    "Failed to clone directory connection for {}: {:?}",
                    RS::Protocol::DEBUG_NAME,
                    err
                );
            }
        }
    }
    Ok(())
}

async fn open_factory_source(factory_config: FactoryConfig) -> Result<fio::DirectoryProxy, Error> {
    match factory_config {
        FactoryConfig::FactoryItems => {
            log::info!("{}", "Reading from FactoryItems service");
            let factory_items_directory = match fetch_new_factory_item().await {
                Ok(item) => parse_bootfs(item).await,
                Err(err) => {
                    log::error!("Failed to get factory item, returning empty item list: {}", err);
                    file_crash_report(CORRUPT_FACTORY_DATA_CRASH_SIGNATURE).await;
                    directory::immutable::simple()
                }
            };
            Ok(vfs::directory::serve_read_only(
                factory_items_directory,
                vfs::execution_scope::ExecutionScope::new(),
            ))
        }
        FactoryConfig::FactoryVerity => {
            log::info!("reading from factory verity");
            let (directory_proxy, directory_server_end) = create_proxy::<fio::DirectoryMarker>();
            fdio::open("/factory", fio::PERM_READABLE, directory_server_end.into_channel())?;
            Ok(directory_proxy)
        }
    }
}

/// Default name to use for `program_name` in the crash report.
const DEFAULT_PROGRAM_NAME: &'static str = "factory_store_providers";

const CORRUPT_FACTORY_DATA_CRASH_SIGNATURE: &'static str = "fuchsia-factory-invalid-data";

/// Files a request to the CrashReporter service with the specified crash report signature.
async fn file_crash_report(signature: &str) {
    let report = ffeedback::CrashReport {
        program_name: Some(DEFAULT_PROGRAM_NAME.to_string()),
        crash_signature: Some(signature.to_string()),
        is_fatal: Some(false),
        ..Default::default()
    };

    let crash_reporter =
        match fuchsia_component::client::connect_to_protocol::<ffeedback::CrashReporterMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                log::warn!("Failed to connect to CrashReporter: {:?}", e);
                return;
            }
        };

    if let Err(e) = crash_reporter.file_report(report).await {
        log::warn!("Failed to file crash report: {:?}", e);
    }
}

#[fuchsia::main(logging_tags = ["factory_store_providers"])]
async fn main() -> Result<(), Error> {
    log::info!("{}", "Starting factory_store_providers");

    let factory_config = load_config_file(FACTORY_DEVICE_CONFIG).unwrap_or_default();
    let directory_proxy = open_factory_source(factory_config)
        .await
        .map_err(|e| {
            log::error!("{:?}", e);
            e
        })
        .unwrap();

    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service(IncomingServices::AlphaFactoryStoreProvider)
        .add_fidl_service(IncomingServices::CastCredentialsFactoryStoreProvider)
        .add_fidl_service(IncomingServices::MiscFactoryStoreProvider)
        .add_fidl_service(IncomingServices::PlayReadyFactoryStoreProvider)
        .add_fidl_service(IncomingServices::WeaveFactoryStoreProvider)
        .add_fidl_service(IncomingServices::WidevineFactoryStoreProvider);
    fs.take_and_serve_directory_handle().expect("Failed to serve factory providers");

    log::info!("{}", "Setting up factory directories");
    let dir_mtx = Arc::new(Mutex::new(directory_proxy));
    let alpha_config = Config::load::<AlphaFactoryStoreProviderMarker>().unwrap_or_default();
    let alpha_directory = Arc::new(Mutex::new(apply_config(alpha_config, dir_mtx.clone()).await));

    let cast_credentials_config = Config::load::<CastCredentialsFactoryStoreProviderMarker>()?;
    let cast_directory =
        Arc::new(Mutex::new(apply_config(cast_credentials_config, dir_mtx.clone()).await));

    let misc_config = Config::load::<MiscFactoryStoreProviderMarker>()?;
    let misc_directory = Arc::new(Mutex::new(apply_config(misc_config, dir_mtx.clone()).await));

    let playready_config = Config::load::<PlayReadyFactoryStoreProviderMarker>()?;
    let playready_directory =
        Arc::new(Mutex::new(apply_config(playready_config, dir_mtx.clone()).await));

    let widevine_config = Config::load::<WidevineFactoryStoreProviderMarker>()?;
    let widevine_directory =
        Arc::new(Mutex::new(apply_config(widevine_config, dir_mtx.clone()).await));

    // The weave config may or may not be present.
    let weave_config = Config::load::<WeaveFactoryStoreProviderMarker>().unwrap_or_default();
    let weave_directory = Arc::new(Mutex::new(apply_config(weave_config, dir_mtx.clone()).await));

    fs.for_each_concurrent(CONCURRENT_LIMIT, move |incoming_service| {
        let alpha_directory_clone = alpha_directory.clone();
        let cast_directory_clone = cast_directory.clone();
        let misc_directory_clone = misc_directory.clone();
        let playready_directory_clone = playready_directory.clone();
        let weave_directory_clone = weave_directory.clone();
        let widevine_directory_clone = widevine_directory.clone();

        async move {
            match incoming_service {
                IncomingServices::AlphaFactoryStoreProvider(stream) => {
                    let alpha_directory_clone = alpha_directory_clone.clone();
                    handle_request_stream(
                        stream,
                        alpha_directory_clone,
                        |req: AlphaFactoryStoreProviderRequest| {
                            req.into_get_factory_store().map(|item| item.0)
                        },
                    )
                    .await
                }
                IncomingServices::CastCredentialsFactoryStoreProvider(stream) => {
                    let cast_directory_clone = cast_directory_clone.clone();
                    handle_request_stream(
                        stream,
                        cast_directory_clone,
                        |req: CastCredentialsFactoryStoreProviderRequest| {
                            req.into_get_factory_store().map(|item| item.0)
                        },
                    )
                    .await
                }
                IncomingServices::MiscFactoryStoreProvider(stream) => {
                    let misc_directory_clone = misc_directory_clone.clone();
                    handle_request_stream(
                        stream,
                        misc_directory_clone,
                        |req: MiscFactoryStoreProviderRequest| {
                            req.into_get_factory_store().map(|item| item.0)
                        },
                    )
                    .await
                }
                IncomingServices::PlayReadyFactoryStoreProvider(stream) => {
                    let playready_directory_clone = playready_directory_clone.clone();
                    handle_request_stream(
                        stream,
                        playready_directory_clone,
                        |req: PlayReadyFactoryStoreProviderRequest| {
                            req.into_get_factory_store().map(|item| item.0)
                        },
                    )
                    .await
                }
                IncomingServices::WeaveFactoryStoreProvider(stream) => {
                    let weave_directory_clone = weave_directory_clone.clone();
                    handle_request_stream(
                        stream,
                        weave_directory_clone,
                        |req: WeaveFactoryStoreProviderRequest| {
                            req.into_get_factory_store().map(|item| item.0)
                        },
                    )
                    .await
                }
                IncomingServices::WidevineFactoryStoreProvider(stream) => {
                    let widevine_directory_clone = widevine_directory_clone.clone();
                    handle_request_stream(
                        stream,
                        widevine_directory_clone,
                        |req: WidevineFactoryStoreProviderRequest| {
                            req.into_get_factory_store().map(|item| item.0)
                        },
                    )
                    .await
                }
            }
        }
        .unwrap_or_else(|err| log::error!("Failed to handle incoming service: {}", err))
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::Proxy as _;
    use vfs::pseudo_directory;

    #[fuchsia::test]
    async fn test_open_factory_verity() {
        // Bind a vfs to /factory.
        let dir = pseudo_directory! {
            "a" => read_only("a content"),
            "b" => pseudo_directory! {
                "c" => read_only("c content"),
            },
        };
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());
        let ns = fdio::Namespace::installed().unwrap();
        ns.bind("/factory", dir_proxy.into_client_end().unwrap()).unwrap();

        let factory_proxy = open_factory_source(FactoryConfig::FactoryVerity).await.unwrap();

        assert_eq!(
            read_file_from_proxy(&factory_proxy, "a").await.unwrap(),
            "a content".as_bytes()
        );
        assert_eq!(
            read_file_from_proxy(&factory_proxy, "b/c").await.unwrap(),
            "c content".as_bytes()
        );
    }
}
