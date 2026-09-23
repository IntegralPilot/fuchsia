// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::enum_variant_names)]

use anyhow::{Context as _, Error, anyhow};
use async_lock::RwLock as AsyncRwLock;
use cobalt_sw_delivery_registry as metrics;
use delivery_blob::DeliveryBlobType;
use fdio::Namespace;
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_contrib::ProtocolConnector;
use fidl_contrib::protocol_connector::ProtocolSender;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics as fmetrics;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_http as fpkg_http;
use fuchsia_async as fasync;
use fuchsia_cobalt_builders::MetricEventExt as _;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect as inspect;
use fuchsia_trace as ftrace;
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use log::{error, info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod authority_service;
mod cache;
mod cache_package_index;
mod clock;
mod component_resolver;
mod config;
mod eager_package_manager;
mod error;
mod inspect_util;
mod metrics_util;
mod ota_downloader;
mod repository;
mod repository_manager;
mod repository_service;
mod resolver_service;
mod rewrite_manager;
mod rewrite_service;
mod util;

#[cfg(test)]
mod test_util;

use crate::cache::BasePackageIndex;
use crate::config::Config;
use crate::repository_manager::{RepositoryManager, RepositoryManagerBuilder};
use crate::repository_service::RepositoryService;
use crate::resolver_service::ResolverServiceInspectState;
use crate::rewrite_manager::{LoadRulesError, RewriteManager, RewriteManagerBuilder};
use crate::rewrite_service::RewriteService;

// FIXME: allow for multiple threads and sendable futures once repo updates support it.
// FIXME(43342): trace durations assume they start and end on the same thread, but since the
// package resolver's executor is multi-threaded, a trace duration that includes an 'await' may not
// end on the same thread it starts on, resulting in invalid trace events.
// const SERVER_THREADS: usize = 2;

const MAX_CONCURRENT_PACKAGE_FETCHES: usize = 5;

// Each fetch_blob call emits an event, and a system update fetches about 1,000 blobs in about a
// minute.
const COBALT_CONNECTOR_BUFFER_SIZE: usize = 1000;

const STATIC_REPO_DIR: &str = "/config/data/repositories";
// Relative to /data.
const DYNAMIC_REPO_PATH: &str = "repositories.json";

// Relative to /config/data.
const STATIC_RULES_PATH: &str = "rewrites.json";
// Relative to /data.
const DYNAMIC_RULES_PATH: &str = "rewrites.json";

#[fuchsia::main(logging_tags = ["pkg-resolver"])]
pub fn main() -> Result<(), Error> {
    let startup_time = Instant::now();
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    info!("starting package resolver");

    let mut executor = fasync::LocalExecutorBuilder::new().build();
    executor.run_singlethreaded(main_inner_async(startup_time)).map_err(|err| {
        // Use anyhow to print the error chain.
        let err = anyhow!(err);
        error!("error running pkg-resolver: {:#}", err);
        err
    })
}

async fn main_inner_async(startup_time: Instant) -> Result<(), Error> {
    let config = Config::load_from_config_data_or_default();
    let structured_config = pkg_resolver_config::Config::take_from_startup_handle();

    let pkg_cache_proxy =
        fuchsia_component::client::connect_to_protocol::<fpkg::PackageCacheMarker>()
            .context("error connecting to package cache")?;
    let pkg_cache = fidl_fuchsia_pkg_ext::cache::Client::from_proxy(pkg_cache_proxy);

    let base_package_index = Arc::new(
        BasePackageIndex::from_proxy(pkg_cache.proxy())
            .await
            .context("failed to load base package index")?,
    );

    // The list of cache packages from the system image, not to be confused with the PackageCache.
    let system_cache_list = Arc::new(cache_package_index::from_proxy(pkg_cache.proxy()).await);

    let inspector = fuchsia_inspect::Inspector::default();
    inspector
        .root()
        .record_child("structured_config", |node| structured_config.record_inspect(node));

    let futures = FuturesUnordered::new();

    let (mut cobalt_sender, cobalt_fut) = ProtocolConnector::new_with_buffer_size(
        metrics_util::CobaltConnectedService,
        COBALT_CONNECTOR_BUFFER_SIZE,
    )
    .serve_and_log_errors();
    futures.push(cobalt_fut.boxed_local());

    let data_proxy = match fuchsia_fs::directory::open_in_namespace(
        "/data",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    ) {
        Ok(proxy) => Some(proxy),
        Err(e) => {
            warn!("failed to open /data: {:#}", anyhow!(e));
            None
        }
    };

    if data_proxy.is_some() {
        let namespace = Namespace::installed().context("failed to get installed namespace")?;
        namespace.unbind("/data").context("failed to unbind /data from default namespace")?;
    }

    let config_proxy =
        match fuchsia_fs::directory::open_in_namespace("/config/data", fio::PERM_READABLE) {
            Ok(proxy) => Some(proxy),
            Err(e) => {
                warn!("failed to open /config/data: {:#}", anyhow!(e));
                None
            }
        };

    let delivery_blob_type: DeliveryBlobType =
        structured_config.delivery_blob_type.try_into().with_context(|| {
            format!("invalid delivery blob type {}", structured_config.delivery_blob_type)
        })?;

    let repo_manager = Arc::new(AsyncRwLock::new(
        load_repo_manager(
            inspector.root().create_child("repository_manager"),
            &config,
            cobalt_sender.clone(),
            TufTimeouts {
                metadata: std::time::Duration::from_secs(
                    structured_config.tuf_metadata_timeout_seconds.into(),
                ),
                network_header: zx::BootDuration::from_seconds(
                    structured_config.tuf_network_header_timeout_seconds.into(),
                ),
            },
            data_proxy.clone(),
            delivery_blob_type,
        )
        .await,
    ));
    let rewrite_manager = Arc::new(AsyncRwLock::new(
        load_rewrite_manager(
            inspector.root().create_child("rewrite_manager"),
            &config,
            data_proxy.clone(),
            config_proxy,
        )
        .await,
    ));

    let (blob_fetch_queue, blob_fetcher) = crate::cache::BlobFetcher::new(
        fuchsia_component::client::connect_to_protocol::<fpkg_http::ClientMarker>()
            .context("error connecting to fuchsia.pkg.http/Client")?,
        inspector.root().create_child("blob_fetcher"),
        structured_config.blob_download_concurrency_limit.into(),
        repo_manager.read().await.stats(),
        cache::BlobFetchParams::builder()
            .header_network_timeout(zx::BootDuration::from_seconds(
                structured_config.blob_network_header_timeout_seconds.into(),
            ))
            .body_network_timeout(zx::BootDuration::from_seconds(
                structured_config.blob_network_body_timeout_seconds.into(),
            ))
            .download_resumption_attempts_limit(
                structured_config.blob_download_resumption_attempts_limit,
            )
            .build(),
    );
    futures.push(blob_fetch_queue.boxed_local());

    let resolver_service_inspect_state = Arc::new(ResolverServiceInspectState::from_node(
        inspector.root().create_child("resolver_service"),
    ));
    let (package_fetch_queue, package_resolver) = resolver_service::QueuedResolver::new(
        pkg_cache.clone(),
        Arc::clone(&base_package_index),
        Arc::clone(&system_cache_list),
        Arc::clone(&repo_manager),
        Arc::clone(&rewrite_manager),
        blob_fetcher.clone(),
        MAX_CONCURRENT_PACKAGE_FETCHES,
        Arc::clone(&resolver_service_inspect_state),
    );
    futures.push(package_fetch_queue.boxed_local());

    // `pkg-resolver` is required for an OTA and EagerPackageManager isn't.
    // Also `EagerPackageManager` depends on /data, which may or may not be available, especially in
    // tests. Wrapping `EagerPackageManager` in Arc<Option<_>> allows it to be used if available
    // during package resolve process.
    let eager_package_manager = Arc::new(
        crate::eager_package_manager::EagerPackageManager::from_namespace(
            package_resolver.clone(),
            pkg_cache.clone(),
            data_proxy,
            &system_cache_list,
            cobalt_sender.clone(),
        )
        .await
        .map_err(|e| {
            error!("failed to create EagerPackageManager: {:#}", e);
        })
        .ok()
        .map(AsyncRwLock::new),
    );

    let make_resolver_cb = {
        let repo_manager = Arc::clone(&repo_manager);
        let rewrite_manager = Arc::clone(&rewrite_manager);
        let package_resolver = package_resolver.clone();
        let pkg_cache = pkg_cache.clone();
        let cobalt_sender = cobalt_sender.clone();
        let eager_package_manager = Arc::clone(&eager_package_manager);
        move |gc_protection| {
            let repo_manager = Arc::clone(&repo_manager);
            let rewrite_manager = Arc::clone(&rewrite_manager);
            let package_resolver = package_resolver.clone();
            let pkg_cache = pkg_cache.clone();
            let base_package_index = Arc::clone(&base_package_index);
            let system_cache_list = Arc::clone(&system_cache_list);
            let cobalt_sender = cobalt_sender.clone();
            let resolver_service_inspect_state = Arc::clone(&resolver_service_inspect_state);
            let eager_package_manager = Arc::clone(&eager_package_manager);
            move |stream| {
                fasync::Task::local(
                    resolver_service::run_resolver_service(
                        Arc::clone(&repo_manager),
                        Arc::clone(&rewrite_manager),
                        package_resolver.clone(),
                        pkg_cache.clone(),
                        Arc::clone(&base_package_index),
                        Arc::clone(&system_cache_list),
                        stream,
                        gc_protection,
                        cobalt_sender.clone(),
                        Arc::clone(&resolver_service_inspect_state),
                        Arc::clone(&eager_package_manager),
                    )
                    .unwrap_or_else(|e| error!("run_resolver_service failed: {:#}", anyhow!(e))),
                )
                .detach()
            }
        }
    };

    let resolver_toolbox_cb = {
        let package_resolver = package_resolver.clone();
        let cobalt_sender = cobalt_sender.clone();
        let eager_package_manager = Arc::clone(&eager_package_manager);
        move |stream| {
            fasync::Task::local(
                resolver_service::run_resolver_toolbox_service(
                    package_resolver.clone(),
                    stream,
                    fpkg::GcProtection::OpenPackageTracking,
                    cobalt_sender.clone(),
                    Arc::clone(&eager_package_manager),
                )
                .unwrap_or_else(|e| {
                    error!("run_resolver_toolbox_service failed: {:#}", anyhow!(e))
                }),
            )
            .detach()
        }
    };

    let authority_cb = {
        let rewrite_manager = Arc::clone(&rewrite_manager);
        let repo_manager = Arc::clone(&repo_manager);
        move |stream| {
            fasync::Task::local(
                authority_service::serve(
                    stream,
                    Arc::clone(&rewrite_manager),
                    Arc::clone(&repo_manager),
                )
                .unwrap_or_else(|e: anyhow::Error| error!("serving authority service: {e:#}")),
            )
            .detach()
        }
    };

    let repo_cb = move |stream| {
        let repo_manager = Arc::clone(&repo_manager);

        fasync::Task::local(
            async move {
                let mut repo_service = RepositoryService::new(repo_manager);
                repo_service.run(stream).await
            }
            .unwrap_or_else(|e| error!("error encountered: {:#}", anyhow!(e))),
        )
        .detach()
    };

    let rewrite_cb = move |stream| {
        let mut rewrite_service = RewriteService::new(Arc::clone(&rewrite_manager));

        fasync::Task::local(
            async move { rewrite_service.handle_client(stream).await }
                .unwrap_or_else(|e| error!("while handling rewrite client {:#}", anyhow!(e))),
        )
        .detach()
    };

    let cup_cb = {
        let cobalt_sender = cobalt_sender.clone();
        move |stream| {
            fasync::Task::local(
                eager_package_manager::run_cup_service(
                    Arc::clone(&eager_package_manager),
                    stream,
                    cobalt_sender.clone(),
                )
                .unwrap_or_else(|e| error!("run_cup_service failed: {:#}", anyhow!(e))),
            )
            .detach()
        }
    };

    let component_resolver_cb = move |stream| {
        fasync::Task::local(
            component_resolver::serve(stream)
                .unwrap_or_else(|e| error!("serve_component_resolver_failed: {:#}", e)),
        )
        .detach()
    };

    let ota_downloader_cb = move |stream| {
        fasync::Task::local(
            ota_downloader::serve(stream, blob_fetcher.clone(), pkg_cache.clone())
                .unwrap_or_else(|e| error!("run_cup_service failed: {:#}", anyhow!(e))),
        )
        .detach()
    };

    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service(make_resolver_cb(fpkg::GcProtection::OpenPackageTracking))
        .add_fidl_service_at(
            format!("{}-ota", fpkg::PackageResolverMarker::PROTOCOL_NAME),
            make_resolver_cb(fpkg::GcProtection::Retained),
        )
        .add_fidl_service(resolver_toolbox_cb)
        .add_fidl_service(authority_cb)
        .add_fidl_service(repo_cb)
        .add_fidl_service(rewrite_cb)
        .add_fidl_service(cup_cb)
        .add_fidl_service(component_resolver_cb)
        .add_fidl_service(ota_downloader_cb);

    fs.take_and_serve_directory_handle().context("while serving directory handle")?;

    futures.push(fs.collect().boxed_local());

    cobalt_sender.send(
        fmetrics::MetricEvent::builder(metrics::PKG_RESOLVER_STARTUP_DURATION_MIGRATED_METRIC_ID)
            .as_integer(Instant::now().duration_since(startup_time).as_micros() as i64),
    );

    ftrace::instant!("app", "startup", ftrace::Scope::Process);

    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    futures.collect::<()>().await;

    Ok(())
}

async fn load_repo_manager(
    node: inspect::Node,
    config: &Config,
    mut cobalt_sender: ProtocolSender<fmetrics::MetricEvent>,
    tuf_timeouts: TufTimeouts,
    data_proxy: Option<fio::DirectoryProxy>,
    delivery_blob_type: DeliveryBlobType,
) -> RepositoryManager {
    // report any errors we saw, but don't error out because otherwise we won't be able
    // to update the system.
    let dynamic_repo_path =
        if config.enable_dynamic_configuration() { Some(DYNAMIC_REPO_PATH) } else { None };
    let builder = match RepositoryManagerBuilder::new(data_proxy, dynamic_repo_path, tuf_timeouts)
        .await
        .unwrap_or_else(|(builder, err)| {
            error!("error loading dynamic repo config: {:#}", anyhow!(err));
            builder
        })
        .delivery_blob_type(delivery_blob_type)
        .inspect_node(node)
        .load_static_configs_dir(STATIC_REPO_DIR)
    {
        Ok(builder) => {
            cobalt_sender.send(
                fmetrics::MetricEvent::builder(
                    metrics::REPOSITORY_MANAGER_LOAD_STATIC_CONFIGS_MIGRATED_METRIC_ID,
                )
                .with_event_codes(
                    metrics::RepositoryManagerLoadStaticConfigsMigratedMetricDimensionResult::Success,
                )
                .as_occurrence(1),
            );
            builder
        }
        Err((builder, errs)) => {
            for err in errs {
                let dimension_result: metrics::RepositoryManagerLoadStaticConfigsMigratedMetricDimensionResult
                    = (&err).into();
                cobalt_sender.send(
                    fmetrics::MetricEvent::builder(
                        metrics::REPOSITORY_MANAGER_LOAD_STATIC_CONFIGS_MIGRATED_METRIC_ID,
                    )
                    .with_event_codes(dimension_result)
                    .as_occurrence(1),
                );
                match &err {
                    crate::repository_manager::LoadError::Io { path: _, error }
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        info!("no statically configured repositories present");
                    }
                    _ => error!("error loading static repo config: {:#}", anyhow!(err)),
                };
            }
            builder
        }
    };

    match config.persisted_repos_dir() {
        Some(repo) => builder.with_persisted_repos_dir(repo),
        None => builder,
    }
    .cobalt_sender(cobalt_sender)
    .build()
}

async fn load_rewrite_manager(
    node: inspect::Node,
    config: &Config,
    data_proxy: Option<fio::DirectoryProxy>,
    config_proxy: Option<fio::DirectoryProxy>,
) -> RewriteManager {
    let dynamic_rules_path =
        if config.enable_dynamic_configuration() { Some(DYNAMIC_RULES_PATH) } else { None };
    let builder = RewriteManagerBuilder::new(data_proxy, dynamic_rules_path)
        .await
        .unwrap_or_else(|(builder, err)| {
            match err {
                // Given a fresh /data, it's expected the file doesn't exist.
                LoadRulesError::FileOpen(fuchsia_fs::node::OpenError::OpenError(
                    zx::Status::NOT_FOUND,
                )) => {}
                // Unable to open /data dir proxy.
                LoadRulesError::DirOpen(_) => {}
                err => error!(
                    "unable to load dynamic rewrite rules from disk, using defaults: {:#}",
                    anyhow!(err)
                ),
            };
            builder
        })
        .inspect_node(node)
        .static_rules_path(config_proxy, STATIC_RULES_PATH)
        .await
        .unwrap_or_else(|(builder, err)| {
            match err {
                // No static rules are configured for this system version.
                LoadRulesError::FileOpen(fuchsia_fs::node::OpenError::OpenError(
                    zx::Status::NOT_FOUND,
                )) => {}
                // Unable to open /config/data dir proxy.
                LoadRulesError::DirOpen(_) => {}
                err => {
                    error!("unable to load static rewrite rules from disk: {:#}", anyhow!(err))
                }
            };
            builder
        });

    builder.build()
}

#[derive(Clone, Copy, Debug)]
struct TufTimeouts {
    metadata: Duration,
    network_header: zx::BootDuration,
}
