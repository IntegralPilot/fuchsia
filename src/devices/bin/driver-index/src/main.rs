// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::driver_loading_fuzzer::Session;
use crate::escrow_support::{apply_state, handle_stall, resume_state};
use crate::indexer::*;
use crate::load_driver::*;
use crate::resolved_driver::ResolvedDriver;
use anyhow::{Context, Result, anyhow};
use driver_index_config::Config;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_index::{
    DevelopmentManagerRequest, DevelopmentManagerRequestStream, DriverIndexRequest,
    DriverIndexRequestStream,
};
use fidl_fuchsia_driver_registrar as fdr;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component::escrow::EscrowOperation;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use zx::Status;

mod composite_helper;
mod composite_node_spec_manager;
mod driver_loading_fuzzer;
mod escrow_support;
mod indexer;
mod load_driver;
mod match_common;
mod resolved_driver;
mod rkyv_ext;

#[cfg(test)]
mod test_common;

/// Wraps all hosted protocols into a single type that can be matched against
/// and dispatched.
enum IncomingRequest {
    DriverIndexProtocol(DriverIndexRequestStream),
    DriverDevelopmentProtocol(DevelopmentManagerRequestStream),
    DriverRegistrarProtocol(fdr::DriverRegistrarRequestStream),
}

fn ignore_peer_closed(err: fidl::Error) -> Result<(), fidl::Error> {
    if err.is_closed() { Ok(()) } else { Err(err) }
}

fn log_error(err: anyhow::Error) -> anyhow::Error {
    log::error!("{:#?}", err);
    err
}

fn create_and_setup_index(
    boot_drivers: Vec<ResolvedDriver>,
    enable_driver_load_fuzzer: bool,
    delay_fallback_until_base_drivers_indexed: bool,
    driver_load_fuzzer_max_delay_ms: i64,
) -> Rc<Indexer> {
    if !enable_driver_load_fuzzer {
        return Rc::new(Indexer::new(
            boot_drivers,
            BaseRepo::NotResolved,
            delay_fallback_until_base_drivers_indexed,
        ));
    }

    let indexer = Rc::new(Indexer::new(
        vec![],
        BaseRepo::NotResolved,
        delay_fallback_until_base_drivers_indexed,
    ));

    // TODO(https://fxbug.dev/42076984): Pass in a seed from the input, if available.
    let (sender, receiver) = futures::channel::mpsc::unbounded::<Vec<ResolvedDriver>>();
    indexer.clone().start_driver_load(
        receiver,
        Session::new(
            sender,
            boot_drivers,
            zx::MonotonicDuration::from_millis(driver_load_fuzzer_max_delay_ms),
            None,
        ),
    );
    indexer
}

async fn run_driver_info_iterator_server(
    driver_info: Arc<Mutex<Vec<fdf::DriverInfo>>>,
    stream: fdd::DriverInfoIteratorRequestStream,
) -> Result<()> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async {
            let driver_info_clone = driver_info.clone();
            match request {
                fdd::DriverInfoIteratorRequest::GetNext { responder } => {
                    let result = {
                        let mut driver_info = driver_info_clone.lock();
                        let len = driver_info.len();
                        driver_info.split_off(len - std::cmp::min(100, len))
                    };

                    responder
                        .send(&result)
                        .or_else(ignore_peer_closed)
                        .context("error responding to GetDriverInfo")?;
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

async fn run_composite_node_specs_iterator_server(
    specs: Arc<Mutex<Vec<fdf::CompositeInfo>>>,
    stream: fdd::CompositeNodeSpecIteratorRequestStream,
) -> Result<()> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async {
            let specs_clone = specs.clone();
            match request {
                fdd::CompositeNodeSpecIteratorRequest::GetNext { responder } => {
                    let result = {
                        let mut specs = specs_clone.lock();
                        let len = specs.len();
                        specs.split_off(len - std::cmp::min(5, len))
                    };

                    responder
                        .send(&result)
                        .or_else(ignore_peer_closed)
                        .context("error responding to GetNodeGroups")?;
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

async fn run_driver_development_server(
    indexer: Rc<Indexer>,
    stream: DevelopmentManagerRequestStream,
) -> Result<()> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async {
            let indexer = indexer.clone();
            match request {
                DevelopmentManagerRequest::GetDriverInfo { driver_filter, iterator, .. } => {
                    let driver_info = indexer.get_driver_info(driver_filter);
                    if driver_info.len() == 0 {
                        iterator.close_with_epitaph(Status::NOT_FOUND)?;
                        return Ok(());
                    }
                    let driver_info = Arc::new(Mutex::new(driver_info));
                    let iterator = iterator.into_stream();
                    fasync::Task::spawn(async move {
                        run_driver_info_iterator_server(driver_info, iterator)
                            .await
                            .expect("Failed to run driver info iterator");
                    })
                    .detach();
                }
                DevelopmentManagerRequest::GetCompositeNodeSpecs {
                    name_filter, iterator, ..
                } => {
                    let composite_node_spec_manager = indexer.composite_node_spec_manager.borrow();
                    let specs = composite_node_spec_manager.get_specs(name_filter);
                    if specs.is_empty() {
                        iterator.close_with_epitaph(Status::NOT_FOUND)?;
                        return Ok(());
                    }
                    let specs = Arc::new(Mutex::new(specs));
                    let iterator = iterator.into_stream();
                    fasync::Task::spawn(async move {
                        run_composite_node_specs_iterator_server(specs, iterator)
                            .await
                            .expect("Failed to run specs iterator");
                    })
                    .detach();
                }
                DevelopmentManagerRequest::DisableDriver {
                    driver_url,
                    package_hash,
                    responder,
                } => {
                    let disable_result = indexer.disable_driver(driver_url, package_hash);
                    responder
                        .send(disable_result)
                        .or_else(ignore_peer_closed)
                        .context("error responding to Disable")?;
                }
                DevelopmentManagerRequest::EnableDriver { driver_url, package_hash, responder } => {
                    let enable_result = indexer.enable_driver(driver_url, package_hash);
                    responder
                        .send(enable_result)
                        .or_else(ignore_peer_closed)
                        .context("error responding to Disable")?;
                }
                DevelopmentManagerRequest::RebindCompositesWithDriver { driver_url, responder } => {
                    let rebind_result = indexer.rebind_composites_with_driver(driver_url);
                    responder
                        .send(rebind_result)
                        .or_else(ignore_peer_closed)
                        .context("error responding to RebindCompositesWithDriver")?;
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

async fn run_driver_registrar_server(
    indexer: Rc<Indexer>,
    stream: fdr::DriverRegistrarRequestStream,
    full_resolver: &Option<fresolution::ResolverProxy>,
) -> Result<()> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async {
            let indexer = indexer.clone();
            match request {
                fdr::DriverRegistrarRequest::Register { driver_url, responder } => {
                    match full_resolver {
                        None => {
                            responder
                                .send(Err(Status::PROTOCOL_NOT_SUPPORTED.into_raw()))
                                .or_else(ignore_peer_closed)
                                .context("error responding to Register")?;
                        }
                        Some(resolver) => {
                            let register_result =
                                indexer.register_driver(driver_url, resolver).await;

                            responder
                                .send(register_result)
                                .or_else(ignore_peer_closed)
                                .context("error responding to Register")?;
                        }
                    }
                }
                fdr::DriverRegistrarRequest::_UnknownMethod { ordinal, method_type, .. } => {
                    log::warn!(
                        "DriverRegistrarRequest::UnknownMethod {:?} with ordinal {}",
                        method_type,
                        ordinal
                    );
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

async fn run_index_server_with_timeout(
    indexer: Rc<Indexer>,
    stream: DriverIndexRequestStream,
    idle_timeout: fasync::MonotonicDuration,
) -> Result<()> {
    let (stream, unbind_if_stalled) = detect_stall::until_stalled(stream, idle_timeout);
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async {
            let indexer = indexer.clone();
            match request {
                DriverIndexRequest::MatchDriver { args, responder } => {
                    let match_result = indexer.match_driver(args);
                    let send_result = responder
                        .send(match_result.as_ref().map_err(|e| *e))
                        .or_else(ignore_peer_closed);

                    if let Err(fidl::Error::ServerResponseWrite(fidl::TransportError::Status(
                        Status::OUT_OF_RANGE,
                    ))) = &send_result
                    {
                        send_result.context(format!(
                            "error responding to MatchDriver. Match result was too big: {:?}",
                            match_result
                        ))?;
                    } else {
                        send_result.context("error responding to MatchDriver.")?;
                    }
                }
                DriverIndexRequest::MatchPendingNode { dependencies, responder } => {
                    let match_result = indexer.match_pending_node(dependencies);
                    responder
                        .send(match_result.as_ref().map_err(|e| *e))
                        .or_else(ignore_peer_closed)
                        .context("error responding to MatchPendingNode")?;
                }
                DriverIndexRequest::AddCompositeNodeSpec { payload, responder } => {
                    responder
                        .send(indexer.add_composite_node_spec(payload))
                        .or_else(ignore_peer_closed)
                        .context("error responding to AddCompositeNodeSpec")?;
                }
                DriverIndexRequest::RebindCompositeNodeSpec {
                    spec,
                    driver_url_suffix,
                    responder,
                } => {
                    responder
                        .send(indexer.rebind_composite(spec, driver_url_suffix))
                        .or_else(ignore_peer_closed)
                        .context("error responding to RebindCompositeNodeSpec")?;
                }
                DriverIndexRequest::SetNotifier { notifier, control_handle: _ } => {
                    indexer.set_notifier(notifier);
                }
            }
            Ok(())
        })
        .await?;

    // The `unbind_if_stalled` future will resolve if the stream was idle
    // for `idle_timeout` or if the stream finished. If the stream was idle,
    // it will resolve with the unbound server endpoint.
    //
    // If the connection did not close or receive new messages within the
    // timeout, send it over to component manager to wait for it on our behalf.
    if let Ok(Some(server_end)) = unbind_if_stalled.await {
        // Escrow the `server_end`...
        // This will open `/escrow/fuchsia.driver.index.DriverIndex` and pass the server
        // endpoint obtained from the idle FIDL connection.
        client::connect_channel_to_protocol_at::<fidl_fuchsia_driver_index::DriverIndexMarker>(
            server_end.into(),
            "/escrow",
        )?;
    }

    Ok(())
}

async fn run_load_base_drivers(
    should_load_base_drivers: bool,
    index: Rc<Indexer>,
    base_drivers: Vec<String>,
    eager_drivers: HashSet<cm_types::Url>,
    disabled_drivers: HashSet<cm_types::Url>,
) -> Result<()> {
    if should_load_base_drivers {
        let base_resolver = client::connect_to_protocol_at_path::<fresolution::ResolverMarker>(
            "/svc/fuchsia.component.resolution.Resolver-base",
        )
        .context("Failed to connect to base component resolver")?;
        let res = load_base_drivers(
            index,
            &base_drivers,
            &base_resolver,
            &eager_drivers,
            &disabled_drivers,
        )
        .await
        .context("Error loading base packages")
        .map_err(log_error);
        log::info!("loaded base drivers.");
        res
    } else {
        Ok(())
    }
}

async fn run_driver_index(
    index: &Rc<Indexer>,
    idle_timeout: fasync::MonotonicDuration,
    full_resolver: Option<fresolution::ResolverProxy>,
    escrow_operation: EscrowOperation,
    capability_store: fsandbox::CapabilityStoreProxy,
    id_gen: sandbox::CapabilityIdGenerator,
    dict_id: u64,
) -> Result<()> {
    let mut service_fs = ServiceFs::new();

    service_fs.dir("svc").add_fidl_service(IncomingRequest::DriverIndexProtocol);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::DriverDevelopmentProtocol);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::DriverRegistrarProtocol);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    let staller_service_fs = service_fs.until_stalled(idle_timeout);
    staller_service_fs
                .for_each_concurrent(None, |item| async {
                    // match on `request` and handle each protocol.
                    match item {
                        fuchsia_component::server::Item::Request(request, _active_guard) => {
                            // Note on |_active_guard|:
                            // While we have an active server running for any of the below protocols
                            // (driver index, driver development, driver registrar) we hold the
                            // |_active_guard| alive. This prevents the stall logic in the
                            // service_fs from kicking in and sending us into the stall branch.
                            //
                            // The driver development and driver registrar protocols are generally
                            // very short lived as the clients are ffx tools, so they connect, call
                            // the method they want, and then disconnect.
                            //
                            // The driver index is used by the driver manager actively during
                            // the startup of a device, but then once in a stable state it becomes
                            // idle. When that happens and the idle timeout is reached, the
                            // index_server will exit and escrow its server end with the component
                            // framework and return back out to here.
                            //
                            // Both of these mechanisms will allow the |_active_guard| to release
                            // and allow the stall handler to be reached.
                            match request {
                                IncomingRequest::DriverIndexProtocol(stream) => {
                                    run_index_server_with_timeout(
                                        index.clone(),
                                        stream,
                                        idle_timeout,
                                    )
                                    .await
                                }
                                IncomingRequest::DriverDevelopmentProtocol(stream) => {
                                    run_driver_development_server(index.clone(), stream).await
                                }
                                IncomingRequest::DriverRegistrarProtocol(stream) => {
                                    run_driver_registrar_server(
                                        index.clone(),
                                        stream,
                                        &full_resolver,
                                    )
                                    .await
                                }
                            }
                        }
                        fuchsia_component::server::Item::Stalled(outgoing_directory) => {
                            let stall_result = handle_stall(
                                index.clone(),
                                &escrow_operation,
                                outgoing_directory,
                                &capability_store,
                                &id_gen,
                                dict_id,
                            )
                            .await;
                            if let Err(e) = stall_result {
                                panic!("Stall handler failed with '{:?}'. Next index run will have incomplete data.", e);
                            }
                            Ok(())
                        }
                    }
                    .unwrap_or_else(|e| log::error!("Error running index_server: {:?}", e))
                })
                .await;
    Ok(())
}

// NOTE: This tag is load-bearing to make sure that the output
// shows up in serial.
#[fuchsia::main(logging_tags = ["driver"])]
async fn main() -> Result<()> {
    let escrow_operation = EscrowOperation::new();

    let config = Config::take_from_startup_handle();
    let full_resolver = if config.enable_ephemeral_drivers {
        Some(
            client::connect_to_protocol_at_path::<fresolution::ResolverMarker>(
                "/svc/fuchsia.component.resolution.Resolver-full",
            )
            .context("Failed to connect to full package resolver")?,
        )
    } else {
        None
    };

    let capability_store = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>()
        .expect("Could not connect to CapabilityStore protocol.");
    let id_gen = sandbox::CapabilityIdGenerator::new();
    let dict_id = id_gen.next();
    let resumed_state = match fuchsia_runtime::take_startup_handle(
        fuchsia_runtime::HandleType::EscrowedDictionary.into(),
    ) {
        Some(dictionary) => {
            let dictionary = fsandbox::Capability::Dictionary(fsandbox::DictionaryRef {
                token: dictionary.into(),
            });
            capability_store
                .import(dict_id, dictionary)
                .await
                .map_err(|e| anyhow!("Failed to call import: {:?}", e))?
                .map_err(|e| anyhow!("Failed to import with store error: {:?}", e))?;

            Some(resume_state(&capability_store, &id_gen, dict_id).await?)
        }
        None => None,
    };

    let dict_id = id_gen.next();

    let eager_drivers: HashSet<cm_types::Url> = config
        .bind_eager
        .iter()
        .filter(|url| !url.is_empty())
        .filter_map(|url| cm_types::Url::new(url).ok())
        .collect();
    let disabled_drivers: HashSet<cm_types::Url> = config
        .disabled_drivers
        .iter()
        .filter(|url| !url.is_empty())
        .filter_map(|url| cm_types::Url::new(url).ok())
        .collect();
    for driver in disabled_drivers.iter() {
        log::info!("Disabling driver {}", driver);
    }
    for driver in eager_drivers.iter() {
        log::info!("Marking driver {} as eager", driver);
    }

    let boot_resolver = client::connect_to_protocol_at_path::<fresolution::ResolverMarker>(
        "/svc/fuchsia.component.resolution.Resolver-boot",
    )
    .context("Failed to connect to boot resolver")?;

    let (boot_driver_list, base_driver_list) = {
        // We only provide empty vectors for these in the driver test realm when the
        // fuchsia.driver.test.DriverLists protocol is available to provide these to us dynamically.
        if config.boot_drivers.is_empty() && config.base_drivers.is_empty() {
            let test_driver_lists =
                client::connect_to_protocol::<fidl_fuchsia_driver_test::DriverListsMarker>()
                    .context("Failed to connect to driver test DriverLists protocol.")?;
            test_driver_lists
                .get_driver_lists()
                .await?
                .map_err(|e| anyhow!("Failed to get_driver_lists {:?}", e))?
        } else {
            (config.boot_drivers, config.base_drivers)
        }
    };

    let boot_repo_resume = resumed_state.as_ref().and_then(|s| s.boot_repo.clone());
    let boot_drivers = match boot_repo_resume {
        Some(boot_repo) => {
            log::info!("loading boot drivers from escrow.");
            boot_repo
        }
        None => {
            load_boot_drivers(&boot_driver_list, &boot_resolver, &eager_drivers, &disabled_drivers)
                .await
                .context("Failed to load boot drivers")
                .map_err(log_error)?
        }
    };

    let mut should_load_base_drivers = true;
    for argument in std::env::args() {
        if argument == "--no-base-drivers" {
            should_load_base_drivers = false;
            log::info!("Not loading base drivers");
        }
    }

    let idle_timeout = if config.stop_on_idle_timeout_millis >= 0 {
        fasync::MonotonicDuration::from_millis(config.stop_on_idle_timeout_millis)
    } else {
        // Negative value means no timeout.
        fasync::MonotonicDuration::INFINITE
    };

    let index = create_and_setup_index(
        boot_drivers,
        config.enable_driver_load_fuzzer,
        config.delay_fallback_until_base_drivers_indexed,
        config.driver_load_fuzzer_max_delay_ms,
    );
    if let Some(resume_state) = resumed_state {
        let base_loaded = apply_state(resume_state, index.clone());
        if base_loaded {
            should_load_base_drivers = false;
        }
    };

    // We don't want to block driver manager from matching up with boot drivers while we resolve
    // base drivers, so we run them in parallel.
    // The base drivers task completes when the base drivers are loaded in.
    // The driver index task does not complete unless the idle timeout is reached.
    // Therefore the join of these two does not complete unless the index task completes when idle
    // timeout is reached.
    let index_clone = index.clone();
    let load_base_task = fasync::Task::local(async move {
        let _ = run_load_base_drivers(
            should_load_base_drivers,
            index_clone,
            base_driver_list,
            eager_drivers,
            disabled_drivers,
        )
        .await;
    });
    load_base_task.detach();

    // Start the task which watches for an instruction to stop from component manager, and kills
    // this process if such an instruction is received.
    escrow_operation.watch_for_stop().expect("failed to watch for stop on lifecycle channel");

    let main_task = run_driver_index(
        &index,
        idle_timeout,
        full_resolver,
        escrow_operation,
        capability_store,
        id_gen,
        dict_id,
    )
    .fuse();

    let index_result = main_task.await;
    log::info!("driver-index stopping because it is idle.");
    index_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite_node_spec_manager::strip_parents_from_spec;
    use crate::resolved_driver::{DeviceCategoryDef, DriverPackageType};
    use crate::test_common::*;
    use bind::compiler::test_lib::*;
    use bind::compiler::{CompiledBindRules, CompositeBindRules, CompositeParent, Symbol};
    use bind::interpreter::decode_bind_rules::DecodedRules;
    use bind::parser::bind_library::ValueType;
    use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, Proxy};
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_data as fdata;
    use fidl_fuchsia_driver_framework as fdf;
    use fidl_fuchsia_driver_index as fdi;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_mem as fmem;
    use std::collections::HashMap;

    fn create_driver_info(
        url: String,
        colocate: bool,
        device_categories: Vec<fdf::DeviceCategory>,
        package_type: DriverPackageType,
        fallback: bool,
    ) -> fdf::DriverInfo {
        fdf::DriverInfo {
            url: Some(url),
            colocate: Some(colocate),
            device_categories: Some(device_categories),
            package_type: fdf::DriverPackageType::from_primitive(package_type as u8),
            is_fallback: Some(fallback),
            bind_rules_bytecode: None,
            is_disabled: Some(false),
            ..Default::default()
        }
    }

    fn create_default_device_category() -> fdf::DeviceCategory {
        fdf::DeviceCategory {
            category: Some(resolved_driver::DEFAULT_DEVICE_CATEGORY.to_string()),
            subcategory: None,
            ..Default::default()
        }
    }

    async fn get_driver_info_proxy(
        development_proxy: &fdi::DevelopmentManagerProxy,
        driver_filter: &[String],
    ) -> Vec<fdf::DriverInfo> {
        let (info_iterator, info_iterator_server) =
            fidl::endpoints::create_proxy::<fdd::DriverInfoIteratorMarker>();
        development_proxy.get_driver_info(driver_filter, info_iterator_server).unwrap();

        let mut driver_infos = Vec::new();
        loop {
            let driver_info = info_iterator.get_next().await;
            if driver_info.is_err() {
                break;
            }
            let mut driver_info = driver_info.unwrap();
            if driver_info.len() == 0 {
                break;
            }
            driver_infos.append(&mut driver_info)
        }

        return driver_infos;
    }

    async fn resolve_component_from_namespace(
        component_url: &str,
    ) -> Result<fresolution::Component, anyhow::Error> {
        let (client_end, server_end) = fidl::endpoints::create_endpoints();
        fuchsia_fs::directory::open_channel_in_namespace(
            "/pkg",
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
            server_end,
        )?;
        let proxy = client_end.into_proxy();
        let component_url = url::Url::parse(component_url)?;
        let decl_file = fuchsia_fs::directory::open_file_async(
            &proxy,
            component_url.fragment().unwrap(),
            fio::PERM_READABLE,
        )?;
        let decl: fdecl::Component = fuchsia_fs::file::read_fidl(&decl_file).await?;
        Ok(fresolution::Component {
            decl: Some(fmem::Data::Bytes(fidl::persist(&decl).unwrap())),
            package: Some(fresolution::Package {
                directory: Some(ClientEnd::new(proxy.into_channel().unwrap().into())),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    async fn run_resolver_server(stream: fresolution::ResolverRequestStream) -> Result<()> {
        stream
            .map(|result| result.context("failed request"))
            .try_for_each(|request| async {
                match request {
                    fresolution::ResolverRequest::Resolve { component_url, responder } => {
                        let component = resolve_component_from_namespace(&component_url).await?;
                        responder.send(Ok(component)).context("error sending response")?;
                    }
                    fresolution::ResolverRequest::ResolveWithContext {
                        component_url: _,
                        context: _,
                        responder,
                    } => {
                        log::error!(
                            "ResolveWithContext is not currently implemented in driver-index"
                        );
                        responder
                            .send(Err(fresolution::ResolverError::Internal))
                            .context("error sending response")?;
                    }
                    fresolution::ResolverRequest::_UnknownMethod { ordinal, .. } => {
                        log::warn!(ordinal:%; "Unknown Resolver request");
                    }
                }
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn run_index_server(
        indexer: Rc<Indexer>,
        stream: DriverIndexRequestStream,
    ) -> Result<()> {
        return run_index_server_with_timeout(indexer, stream, zx::MonotonicDuration::INFINITE)
            .await;
    }

    async fn execute_driver_index_test(
        index: Indexer,
        stream: fdi::DriverIndexRequestStream,
        test: impl Future<Output = ()>,
    ) {
        let index = Rc::new(index);
        let index_task = run_index_server(index.clone(), stream).fuse();
        let test = test.fuse();

        futures::pin_mut!(index_task, test);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test => {},
        }
    }

    fn create_always_match_bind_rules() -> DecodedRules {
        let bind_rules = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(bind_rules).unwrap(),
        )
        .unwrap()
    }

    #[fuchsia::test]
    async fn run_with_timeout() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();
        let index = Rc::new(Indexer::new(vec![], BaseRepo::Resolved(std::vec![]), false));
        run_index_server_with_timeout(
            index.clone(),
            stream,
            zx::MonotonicDuration::from_millis(10),
        )
        .await
        .unwrap();

        let result =
            proxy.add_composite_node_spec(&fdf::CompositeNodeSpec { ..Default::default() }).await;
        assert_eq!(true, result.is_err());
        let fidl::Error::ClientChannelClosed { epitaph, protocol_name, .. } = result.err().unwrap()
        else {
            panic!("wrong error");
        };
        assert_eq!(fdi::DriverIndexMarker::PROTOCOL_NAME, protocol_name);
        assert_eq!(epitaph, Status::NOT_FOUND);
    }

    // This test depends on '/pkg/config/drivers_for_test.json' existing in the test package.
    // The test reads that json file to determine which bind rules to read and index.
    #[fuchsia::test]
    async fn read_from_json() {
        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], BaseRepo::NotResolved, false));

        let eager_drivers = HashSet::new();
        let disabled_drivers = HashSet::new();
        let base_drivers = vec![
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm".into(),
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind2-component.cm".into(),
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-fallback-component.cm"
                .into(),
        ];

        // Run two tasks: the fake resolver and the task that loads the base drivers.
        let load_base_drivers_task = load_base_drivers(
            index.clone(),
            &base_drivers,
            &resolver,
            &eager_drivers,
            &disabled_drivers,
        )
        .fuse();
        let resolver_task = run_resolver_server(resolver_stream).fuse();
        futures::pin_mut!(load_base_drivers_task, resolver_task);
        futures::select! {
            result = load_base_drivers_task => {
                result.unwrap();
            },
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
        };

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            // Check the value from the 'test-bind' binary. This should match my-driver.cm
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(1),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_url =
                "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm"
                    .to_string();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!(expected_url, d.url.unwrap());
                    assert_eq!(true, d.colocate.unwrap());
                    assert_eq!(false, d.is_fallback.unwrap());
                    assert_eq!(fdf::DriverPackageType::Base, d.package_type.unwrap());
                    assert_eq!(
                        vec![create_default_device_category()],
                        d.device_categories.unwrap()
                    );
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            // Check the value from the 'test-bind2' binary. This should match my-driver2.cm
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_url =
                "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind2-component.cm"
                    .to_string();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!(expected_url, d.url.unwrap());
                    assert_eq!(false, d.colocate.unwrap());
                    assert_eq!(false, d.is_fallback.unwrap());
                    assert_eq!(fdf::DriverPackageType::Base, d.package_type.unwrap());
                    assert_eq!(
                        vec![create_default_device_category()],
                        d.device_categories.unwrap()
                    );
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            // Check an unknown value. This should return the NOT_FOUND error.
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(3),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap();
            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_bind_string() {
        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![make_abort_ne_symbinst(
                Symbol::Key("my-key".to_string(), ValueType::Str),
                Symbol::StringValue("test-value".to_string()),
            )],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        // Make our driver.
        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: cm_types::Url::new(
                "fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm"
            )
            .unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = make_property(
                "my-key",
                fdf::NodePropertyValue::StringValue("test-value".to_string()),
            );
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_result = fdi::MatchDriverResult::Driver(fdf::DriverInfo {
                url: Some("fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm".to_string()),
                colocate: Some(false),
                device_categories: Some(vec![]),
                package_type: Some(fdf::DriverPackageType::Base),
                is_fallback: Some(false),
                bind_rules_bytecode: None,
                is_disabled: Some(false),
                ..Default::default()
            });

            assert_eq!(expected_result, result);
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_and_composite_spec_prioritization() {
        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        // Make our driver.
        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: cm_types::Url::new(
                "fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm"
            )
            .unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        // Add a composite node spec that always matches.
        let composite_spec = fdf::CompositeNodeSpec {
            name: Some("test_spec".to_string()),
            parents2: Some(vec![fdf::ParentSpec2 {
                bind_rules: vec![fdf::BindRule2 {
                    key: "my-key".to_string(),
                    condition: fdf::Condition::Accept,
                    values: vec![fdf::NodePropertyValue::StringValue("test-value".to_string())],
                }],
                properties: vec![],
            }]),
            ..Default::default()
        };

        index.add_composite_node_spec(composite_spec).unwrap();

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = make_property(
                "my-key",
                fdf::NodePropertyValue::StringValue("test-value".to_string()),
            );

            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_parent = fdf::CompositeParent {
                composite: Some(fdf::CompositeInfo {
                    spec: Some(fdf::CompositeNodeSpec {
                        name: Some("test_spec".to_string()),
                        parents2: Some(vec![fdf::ParentSpec2 {
                            bind_rules: vec![],
                            properties: vec![],
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                index: Some(0),
                ..Default::default()
            };

            assert_eq!(result, fdi::MatchDriverResult::CompositeParents(vec![expected_parent]));
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_bind_enum() {
        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![make_abort_ne_symbinst(
                Symbol::Key("my-key".to_string(), ValueType::Enum),
                Symbol::EnumValue("test-value".to_string()),
            )],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        // Make our driver.
        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: cm_types::Url::new(
                "fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm"
            )
            .unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = make_property(
                "my-key",
                fdf::NodePropertyValue::EnumValue("test-value".to_string()),
            );

            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_result = fdi::MatchDriverResult::Driver(fdf::DriverInfo {
                url: Some("fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm".to_string()),
                colocate: Some(false),
                package_type: Some(fdf::DriverPackageType::Base),
                is_fallback: Some(false),
                device_categories: Some(vec![]),
                bind_rules_bytecode: None,
                is_disabled: Some(false),
                ..Default::default()
            });

            assert_eq!(expected_result, result);
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_multiple_non_fallbacks() {
        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        let boot_repo = vec![
            ResolvedDriver {
                component_url: cm_types::Url::new("fuchsia-boot:///#meta/driver-1.cm").unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
            ResolvedDriver {
                component_url: cm_types::Url::new("fuchsia-boot:///#meta/driver-2.cm").unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
        ];

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(boot_repo, BaseRepo::Resolved(std::vec![]), false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            let result = proxy.match_driver(&args).await.unwrap();

            assert_eq!(result, Err(Status::NOT_SUPPORTED.into_raw()));
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_url_matching() {
        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        let boot_repo = vec![
            ResolvedDriver {
                component_url: cm_types::Url::new("fuchsia-boot:///#meta/driver-1.cm").unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
            ResolvedDriver {
                component_url: cm_types::Url::new("fuchsia-boot:///#meta/driver-2.cm").unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
        ];

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(boot_repo, BaseRepo::Resolved(std::vec![]), false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args = fdi::MatchDriverArgs {
                properties: Some(vec![property.clone()]),
                driver_url_suffix: Some("driver-1.cm".to_string()),
                ..Default::default()
            };

            let result = proxy.match_driver(&args).await.unwrap().unwrap();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!("fuchsia-boot:///#meta/driver-1.cm", d.url.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            let args = fdi::MatchDriverArgs {
                properties: Some(vec![property.clone()]),
                driver_url_suffix: Some("driver-2.cm".to_string()),
                ..Default::default()
            };
            let result = proxy.match_driver(&args).await.unwrap().unwrap();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!("fuchsia-boot:///#meta/driver-2.cm", d.url.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            let args = fdi::MatchDriverArgs {
                properties: Some(vec![property]),
                driver_url_suffix: Some("bad_driver.cm".to_string()),
                ..Default::default()
            };

            let result = proxy.match_driver(&args).await.unwrap();
            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_url_disabled() {
        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        let boot_repo = vec![
            ResolvedDriver {
                component_url: cm_types::Url::new("fuchsia-boot:///#meta/driver-1.cm").unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
            ResolvedDriver {
                component_url: cm_types::Url::new("fuchsia-boot:///#meta/driver-2.cm").unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
        ];

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let (development_proxy, development_stream) =
            fidl::endpoints::create_proxy_and_stream::<fdi::DevelopmentManagerMarker>();

        let index = Rc::new(Indexer::new(boot_repo, BaseRepo::Resolved(std::vec![]), false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let development_task =
            run_driver_development_server(index.clone(), development_stream).fuse();
        let test_task = async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args = fdi::MatchDriverArgs {
                properties: Some(vec![property.clone()]),
                driver_url_suffix: Some("driver-1.cm".to_string()),
                ..Default::default()
            };

            // Ask for driver-1 and it should give that to us.
            let result = proxy.match_driver(&args).await.unwrap().unwrap();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!("fuchsia-boot:///#meta/driver-1.cm", d.url.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            // Disable driver-1.
            development_proxy
                .disable_driver("fuchsia-boot:///#meta/driver-1.cm", None)
                .await
                .unwrap()
                .unwrap();

            // Ask for driver-1 again and it should return not found since its been disabled.
            let result = proxy.match_driver(&args).await.unwrap();
            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));

            let args = fdi::MatchDriverArgs {
                properties: Some(vec![property.clone()]),
                ..Default::default()
            };

            // Ask for any and we should get driver-2, since driver-1 is disabled.
            let result = proxy.match_driver(&args).await.unwrap().unwrap();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!("fuchsia-boot:///#meta/driver-2.cm", d.url.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            development_proxy
                .enable_driver("fuchsia-boot:///#meta/driver-1.cm", None)
                .await
                .unwrap()
                .unwrap();

            let args = fdi::MatchDriverArgs {
                properties: Some(vec![property.clone()]),
                driver_url_suffix: Some("driver-1.cm".to_string()),
                ..Default::default()
            };

            // Ask for driver-1 and it should give that to us since it's not disabled anymore.
            let result = proxy.match_driver(&args).await.unwrap().unwrap();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!("fuchsia-boot:///#meta/driver-1.cm", d.url.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }
        }
        .fuse();

        futures::pin_mut!(index_task, development_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            result = development_task => {
                panic!("Development task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_non_fallback_boot_priority() {
        const FALLBACK_BOOT_DRIVER_COMPONENT_URL: &str =
            "fuchsia-pkg://fuchsia.com/package#driver/fallback-boot.cm";
        const NON_FALLBACK_BOOT_DRIVER_COMPONENT_URL: &str =
            "fuchsia-pkg://fuchsia.com/package#driver/non-fallback-base.cm";

        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        let boot_repo = vec![
            ResolvedDriver {
                component_url: cm_types::Url::new(FALLBACK_BOOT_DRIVER_COMPONENT_URL).unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: true,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
            ResolvedDriver {
                component_url: cm_types::Url::new(NON_FALLBACK_BOOT_DRIVER_COMPONENT_URL).unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Boot,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
        ];

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(boot_repo, BaseRepo::Resolved(std::vec![]), false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_result = fdi::MatchDriverResult::Driver(create_driver_info(
                NON_FALLBACK_BOOT_DRIVER_COMPONENT_URL.to_string(),
                false,
                vec![],
                DriverPackageType::Boot,
                false,
            ));

            // The non-fallback boot driver should be returned and not the
            // fallback boot driver.
            assert_eq!(result, expected_result);
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_non_fallback_base_priority() {
        const FALLBACK_BOOT_DRIVER_COMPONENT_URL: &str =
            "fuchsia-pkg://fuchsia.com/package#driver/fallback-boot.cm";
        const NON_FALLBACK_BASE_DRIVER_COMPONENT_URL: &str =
            "fuchsia-pkg://fuchsia.com/package#driver/non-fallback-base.cm";
        const FALLBACK_BASE_DRIVER_COMPONENT_URL: &str =
            "fuchsia-pkg://fuchsia.com/package#driver/fallback-base.cm";

        // Make the bind instructions.
        let always_match = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match).unwrap(),
        )
        .unwrap();

        let boot_repo = vec![ResolvedDriver {
            component_url: cm_types::Url::new(FALLBACK_BOOT_DRIVER_COMPONENT_URL).unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: true,
            package_type: DriverPackageType::Boot,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }];

        let base_repo = BaseRepo::Resolved(std::vec![
            ResolvedDriver {
                component_url: cm_types::Url::new(FALLBACK_BASE_DRIVER_COMPONENT_URL).unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: true,
                package_type: DriverPackageType::Base,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
            ResolvedDriver {
                component_url: cm_types::Url::new(NON_FALLBACK_BASE_DRIVER_COMPONENT_URL).unwrap(),
                bind_rules: always_match.clone(),
                bind_bytecode: vec![],
                colocate: false,
                device_categories: vec![],
                fallback: false,
                package_type: DriverPackageType::Base,
                package_hash: None,
                is_dfv2: None,
                disabled: false,
            },
        ]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(boot_repo, base_repo, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_result = fdi::MatchDriverResult::Driver(create_driver_info(
                NON_FALLBACK_BASE_DRIVER_COMPONENT_URL.to_string(),
                false,
                vec![],
                DriverPackageType::Base,
                false,
            ));

            // The non-fallback base driver should be returned and not the
            // fallback boot driver even though boot drivers get priority
            // because non-fallback drivers get even higher priority.
            assert_eq!(result, expected_result);
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_load_packaged_boot_drivers() {
        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let eager_drivers = HashSet::new();
        let disabled_drivers = HashSet::new();
        let boot_drivers = vec![];

        let load_boot_drivers_task =
            load_boot_drivers(&boot_drivers, &resolver, &eager_drivers, &disabled_drivers).fuse();

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        futures::pin_mut!(load_boot_drivers_task, resolver_task);
        let drivers = futures::select! {
            result = load_boot_drivers_task => result.unwrap(),
            result = resolver_task => panic!("Resolver task finished: {:?}", result),
        };

        // Expect package qualifiers are set for all packaged drivers.
        assert!(drivers.iter().all(|driver| {
            driver.component_url.as_str().starts_with("fuchsia-boot:///driver-index-unittests")
        }));
        assert!(
            drivers
                .iter()
                .all(|driver| driver.package_type == resolved_driver::DriverPackageType::Boot)
        );
        assert!(drivers.iter().all(|driver| driver.package_hash.is_some()));
    }

    #[fuchsia::test]
    async fn test_load_eager_fallback_boot_driver() {
        let eager_driver_component_url = cm_types::Url::new(
            "fuchsia-boot:///driver-index-unittests#meta/test-fallback-component.cm",
        )
        .unwrap();
        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();
        let eager_drivers = HashSet::from([eager_driver_component_url.clone()]);
        let disabled_drivers = HashSet::new();
        let boot_drivers =
            vec!["fuchsia-boot:///driver-index-unittests#meta/test-fallback-component.cm".into()];

        let load_boot_drivers_task =
            load_boot_drivers(&boot_drivers, &resolver, &eager_drivers, &disabled_drivers).fuse();

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        futures::pin_mut!(load_boot_drivers_task, resolver_task);
        let drivers = futures::select! {
            result = load_boot_drivers_task => {
                result.unwrap()
            },
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
        };
        assert!(
            !drivers
                .iter()
                .find(|driver| driver.component_url == eager_driver_component_url)
                .expect("Fallback driver did not load")
                .fallback
        );
    }

    #[fuchsia::test]
    async fn test_load_eager_fallback_base_driver() {
        let eager_driver_component_url = cm_types::Url::new(
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-fallback-component.cm",
        )
        .unwrap();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let index = Rc::new(Indexer::new(std::vec![], BaseRepo::NotResolved, false));

        let eager_drivers = HashSet::from([eager_driver_component_url.clone()]);
        let disabled_drivers = HashSet::new();
        let base_drivers = vec![
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-fallback-component.cm"
                .into(),
        ];

        let load_base_drivers_task = load_base_drivers(
            Rc::clone(&index),
            &base_drivers,
            &resolver,
            &eager_drivers,
            &disabled_drivers,
        )
        .fuse();
        let resolver_task = run_resolver_server(resolver_stream).fuse();
        futures::pin_mut!(load_base_drivers_task, resolver_task);
        futures::select! {
            result = load_base_drivers_task => {
                result.unwrap();
            },
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
        };

        let base_repo = index.base_repo.borrow();
        match *base_repo {
            BaseRepo::Resolved(ref drivers) => {
                assert!(
                    !drivers
                        .iter()
                        .find(|driver| driver.component_url == eager_driver_component_url)
                        .expect("Fallback driver did not load")
                        .fallback
                );
            }
            _ => {
                panic!("Base repo was not resolved");
            }
        }
    }

    #[fuchsia::test]
    async fn test_dont_load_disabled_fallback_boot_driver() {
        let disabled_driver_component_url = cm_types::Url::new(
            "fuchsia-boot:///driver-index-unittests#meta/test-fallback-component.cm",
        )
        .unwrap();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();
        let eager_drivers = HashSet::new();
        let disabled_drivers = HashSet::from([disabled_driver_component_url.clone()]);
        let boot_drivers = vec![];

        let load_boot_drivers_task =
            load_boot_drivers(&boot_drivers, &resolver, &eager_drivers, &disabled_drivers).fuse();

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        futures::pin_mut!(load_boot_drivers_task, resolver_task);
        let drivers = futures::select! {
            result = load_boot_drivers_task => {
                result.unwrap()
            },
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
        };
        assert!(
            drivers
                .iter()
                .find(|driver| driver.component_url == disabled_driver_component_url)
                .is_none()
        );
    }

    #[fuchsia::test]
    async fn test_dont_load_disabled_fallback_base_driver() {
        let disabled_driver_component_url = cm_types::Url::new(
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-fallback-component.cm",
        )
        .unwrap();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let index = Rc::new(Indexer::new(std::vec![], BaseRepo::NotResolved, false));

        let eager_drivers = HashSet::new();
        let disabled_drivers = HashSet::from([disabled_driver_component_url.clone()]);
        let base_drivers = vec![];

        let load_base_drivers_task = load_base_drivers(
            Rc::clone(&index),
            &base_drivers,
            &resolver,
            &eager_drivers,
            &disabled_drivers,
        )
        .fuse();
        let resolver_task = run_resolver_server(resolver_stream).fuse();
        futures::pin_mut!(load_base_drivers_task, resolver_task);
        futures::select! {
            result = load_base_drivers_task => {
                result.unwrap();
            },
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
        };

        let base_repo = index.base_repo.borrow();
        match *base_repo {
            BaseRepo::Resolved(ref drivers) => {
                assert!(
                    drivers
                        .iter()
                        .find(|driver| driver.component_url == disabled_driver_component_url)
                        .is_none()
                );
            }
            _ => {
                panic!("Base repo was not resolved");
            }
        }
    }

    #[fuchsia::test]
    async fn test_match_driver_when_require_system_true_and_base_repo_not_resolved() {
        let always_match = create_always_match_bind_rules();
        let boot_repo = vec![ResolvedDriver {
            component_url: cm_types::Url::new("fuchsia-boot:///#driver/fallback-boot.cm").unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: true,
            package_type: DriverPackageType::Boot,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }];
        let index = Indexer::new(boot_repo, BaseRepo::NotResolved, true);
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        execute_driver_index_test(index, stream, async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap();

            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_match_driver_when_require_system_false_and_base_repo_not_resolved() {
        const FALLBACK_BOOT_DRIVER_COMPONENT_URL: &str = "fuchsia-boot:///#driver/fallback-boot.cm";

        let always_match = create_always_match_bind_rules();
        let boot_repo = vec![ResolvedDriver {
            component_url: cm_types::Url::new(FALLBACK_BOOT_DRIVER_COMPONENT_URL).unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: true,
            package_type: DriverPackageType::Boot,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }];
        let index = Indexer::new(boot_repo, BaseRepo::NotResolved, false);
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        execute_driver_index_test(index, stream, async move {
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_result = fdi::MatchDriverResult::Driver(create_driver_info(
                FALLBACK_BOOT_DRIVER_COMPONENT_URL.to_string(),
                false,
                vec![],
                DriverPackageType::Boot,
                true,
            ));
            assert_eq!(result, expected_result);
        })
        .await;
    }

    // This test relies on two drivers existing in the /pkg/ directory of the
    // test package.
    #[ignore = "Re-enable once we have a fake resolver"]
    #[fuchsia::test]
    async fn test_boot_drivers() {
        let boot_drivers = vec![];
        let boot_resolver = client::connect_to_protocol::<fresolution::ResolverMarker>().unwrap();
        let drivers =
            load_boot_drivers(&boot_drivers, &boot_resolver, &HashSet::new(), &HashSet::new())
                .await
                .unwrap();

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(drivers, BaseRepo::NotResolved, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            // Check the value from the 'test-bind' binary. This should match my-driver.cm
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(1),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_url = "fuchsia-boot:///#meta/test-bind-component.cm".to_string();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!(expected_url, d.url.unwrap());
                    assert_eq!(true, d.colocate.unwrap());
                    assert_eq!(false, d.is_fallback.unwrap());
                    assert_eq!(fdf::DriverPackageType::Boot, d.package_type.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            // Check the value from the 'test-bind2' binary. This should match my-driver2.cm
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap().unwrap();

            let expected_url = "fuchsia-boot:///#meta/test-bind2-component.cm".to_string();
            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!(expected_url, d.url.unwrap());
                    assert_eq!(false, d.colocate.unwrap());
                    assert_eq!(false, d.is_fallback.unwrap());
                    assert_eq!(fdf::DriverPackageType::Boot, d.package_type.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }

            // Check an unknown value. This should return the NOT_FOUND error.
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(3),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };
            let result = proxy.match_driver(&args).await.unwrap();
            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_parent_spec_match() {
        let base_repo = BaseRepo::Resolved(std::vec![]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();
        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));
        let index_task = run_index_server(index.clone(), stream).fuse();

        let test_task = async move {
            let bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let properties = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let composite_spec =
                make_composite_spec("test_spec", vec![make_parent_spec(bind_rules, properties)]);

            assert_eq!(Ok(()), proxy.add_composite_node_spec(&composite_spec).await.unwrap());

            let device_properties_match = vec![
                make_property("testkey_1", fdf::NodePropertyValue::IntValue(200)),
                make_property("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];
            let match_args = fdi::MatchDriverArgs {
                properties: Some(device_properties_match),
                ..Default::default()
            };

            let result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            assert_eq!(
                fdi::MatchDriverResult::CompositeParents(vec![fdf::CompositeParent {
                    composite: Some(fdf::CompositeInfo {
                        spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                        ..Default::default()
                    }),
                    index: Some(0),
                    ..Default::default()
                }]),
                result
            );

            let device_properties_mismatch = vec![
                make_property("testkey_1", fdf::NodePropertyValue::IntValue(200)),
                make_property(
                    "lapwing",
                    fdf::NodePropertyValue::StringValue("dotterel".to_string()),
                ),
            ];
            let mismatch_args = fdi::MatchDriverArgs {
                properties: Some(device_properties_mismatch),
                ..Default::default()
            };

            let result = proxy.match_driver(&mismatch_args).await.unwrap();
            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_parent_spec_match_too_large() {
        let base_repo = BaseRepo::Resolved(std::vec![]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();
        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));
        let index_task = run_index_server(index.clone(), stream).fuse();

        let test_task = async move {
            let bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let properties = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            // Load in a bunch of composite node specs that will match later.
            for i in 1..400 {
                let composite_spec = make_composite_spec(
                    format!("test_group_{}", i).as_str(),
                    vec![make_parent_spec(bind_rules.clone(), properties.clone())],
                );

                assert_eq!(Ok(()), proxy.add_composite_node_spec(&composite_spec).await.unwrap());
            }

            let device_properties_match = vec![
                make_property("testkey_1", fdf::NodePropertyValue::IntValue(200)),
                make_property("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];
            let match_args = fdi::MatchDriverArgs {
                properties: Some(device_properties_match),
                ..Default::default()
            };

            // Since there is a bunch of matching parent specs this will hit the too big case.
            let result = proxy.match_driver(&match_args).await;
            assert!(result.is_err());
        }
        .fuse();

        let index_expected_error = "error responding to MatchDriver. Match result was too big: ";

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            index_result = index_task => {
                assert!(index_result.is_err());
                assert!(index_result.unwrap_err().to_string().starts_with(index_expected_error));
            },
            () = test_task => {},
        }
        futures::select! {
            index_result = index_task => {
                assert!(index_result.is_err());
                assert!(index_result.unwrap_err().to_string().starts_with(index_expected_error));
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_matched_composite() {
        // Create the Composite Bind rules.
        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("trembler".to_string(), ValueType::Str),
            Symbol::StringValue("thrasher".to_string()),
        )];

        let additional_node_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key("thrasher".to_string(), ValueType::Str),
                Symbol::StringValue("catbird".to_string()),
            ),
            make_abort_ne_symbinst(
                Symbol::Key("catbird".to_string(), ValueType::Number),
                Symbol::NumberValue(1),
            ),
        ];

        let bind_rules = CompositeBindRules {
            device_name: "mimid".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "catbird".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "mockingbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![],
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        // Make the composite driver.
        let url =
            cm_types::Url::new("fuchsia-pkg://fuchsia.com/package#driver/dg_matched_composite.cm")
                .unwrap();
        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: url.clone(),
            bind_rules: rules,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let node_1_bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let node_1_props_match = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let node_2_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let node_2_props_match = vec![
                make_property("catbird", fdf::NodePropertyValue::IntValue(1)),
                make_property(
                    "thrasher",
                    fdf::NodePropertyValue::StringValue("catbird".to_string()),
                ),
            ];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec (
                        "spec_match",
                        vec![
                            make_parent_spec(
                                node_1_bind_rules.clone(),
                                node_1_props_match.clone(),
                            ),
                            make_parent_spec(
                                node_2_bind_rules.clone(),
                                node_2_props_match.clone(),
                            ),
                        ]))
                    .await
                    .unwrap()
            );

            let node_1_props_nonmatch = vec![fdf::NodeProperty2 {
                key: "trembler".to_string(),
                value: fdf::NodePropertyValue::StringValue("catbird".to_string()),
            }];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec(
                        "spec_non_match_1",
                        vec![
                            make_parent_spec(node_1_bind_rules.clone(), node_1_props_nonmatch),
                            make_parent_spec(node_2_bind_rules.clone(), node_2_props_match)
                        ]
                    ))
                    .await
                    .unwrap()
            );

            let node_2_props_nonmatch = vec![fdf::NodeProperty2 {
                key: "testkey_1".to_string(),
                value: fdf::NodePropertyValue::IntValue(10),
            }];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec(
                        "spec_non_match_2",
                        vec![
                            make_parent_spec(node_1_bind_rules.clone(), node_1_props_match),
                            make_parent_spec(node_2_bind_rules.clone(), node_2_props_nonmatch)
                        ]
                    ))
                    .await
                    .unwrap()
            );
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_no_optional_matched_composite_with_optional() {
        // Create the Composite Bind rules.
        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("trembler".to_string(), ValueType::Str),
            Symbol::StringValue("thrasher".to_string()),
        )];

        let additional_node_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key("thrasher".to_string(), ValueType::Str),
                Symbol::StringValue("catbird".to_string()),
            ),
            make_abort_ne_symbinst(
                Symbol::Key("catbird".to_string(), ValueType::Number),
                Symbol::NumberValue(1),
            ),
        ];

        let optional_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("thrasher".to_string(), ValueType::Str),
            Symbol::StringValue("trembler".to_string()),
        )];

        let bind_rules = CompositeBindRules {
            device_name: "mimid".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "catbird".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "mockingbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![CompositeParent {
                name: "lapwing".to_string(),
                instructions: optional_node_inst,
            }],
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        // Make the composite driver.
        let url =
            cm_types::Url::new("fuchsia-pkg://fuchsia.com/package#driver/dg_matched_composite.cm")
                .unwrap();
        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: url.clone(),
            bind_rules: rules,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let node_1_bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let node_1_props_match = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let node_2_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let node_2_props_match = vec![
                make_property("catbird", fdf::NodePropertyValue::IntValue(1)),
                make_property(
                    "thrasher",
                    fdf::NodePropertyValue::StringValue("catbird".to_string()),
                ),
            ];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec(
                        "spec_match",
                        vec![
                            make_parent_spec(node_1_bind_rules.clone(), node_1_props_match.clone()),
                            make_parent_spec(
                                node_2_bind_rules.clone(),
                                node_2_props_match.clone(),
                            ),

                        ]
                    ))
                    .await
                    .unwrap()
            );

            let node_1_props_nonmatch = vec![fdf::NodeProperty2 {
                key: "trembler".to_string(),
                value: fdf::NodePropertyValue::StringValue("catbird".to_string()),
            }];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec(
                        "spec_non_match_1",
                        vec![
                            make_parent_spec(node_1_bind_rules.clone(), node_1_props_nonmatch),
                            make_parent_spec(node_2_bind_rules.clone(), node_2_props_match)
                        ]
                    ))
                    .await
                    .unwrap()
            );

            let node_2_props_nonmatch = vec![fdf::NodeProperty2 {
                key: "testkey_1".to_string(),
                value: fdf::NodePropertyValue::IntValue(10),
            }];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec(
                        "spec_non_match_2",
                        vec![
                            make_parent_spec(node_1_bind_rules.clone(), node_1_props_match),
                            make_parent_spec(node_2_bind_rules.clone(), node_2_props_nonmatch)
                        ]
                    ))
                    .await
                    .unwrap()
            );
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_with_optional_matched_composite_with_optional() {
        // Create the Composite Bind rules.
        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("trembler".to_string(), ValueType::Str),
            Symbol::StringValue("thrasher".to_string()),
        )];

        let additional_node_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key("thrasher".to_string(), ValueType::Str),
                Symbol::StringValue("catbird".to_string()),
            ),
            make_abort_ne_symbinst(
                Symbol::Key("catbird".to_string(), ValueType::Number),
                Symbol::NumberValue(1),
            ),
        ];

        let optional_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("thrasher".to_string(), ValueType::Str),
            Symbol::StringValue("trembler".to_string()),
        )];

        let bind_rules = CompositeBindRules {
            device_name: "mimid".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "catbird".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "mockingbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![CompositeParent {
                name: "lapwing".to_string(),
                instructions: optional_node_inst,
            }],
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        // Make the composite driver.
        let url =
            cm_types::Url::new("fuchsia-pkg://fuchsia.com/package#driver/dg_matched_composite.cm")
                .unwrap();
        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: url.clone(),
            bind_rules: rules,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let node_1_bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let node_1_props_match = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let optional_1_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let optional_1_props_match = vec![fdf::NodeProperty2 {
                key: "thrasher".to_string(),
                value: fdf::NodePropertyValue::StringValue("trembler".to_string()),
            }];

            let node_2_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let node_2_props_match = vec![
                make_property("catbird", fdf::NodePropertyValue::IntValue(1)),
                make_property(
                    "thrasher",
                    fdf::NodePropertyValue::StringValue("catbird".to_string()),
                ),
            ];

            assert_eq!(
                    Ok(()),
                    proxy
                        .add_composite_node_spec(&make_composite_spec(
                            "spec_match",
                            vec![
                                make_parent_spec(
                                    node_1_bind_rules.clone(),
                                    node_1_props_match.clone(),
                                ),
                                make_parent_spec(
                                    optional_1_bind_rules.clone(),
                                    optional_1_props_match.clone(),
                                ),
                                make_parent_spec(
                                    node_2_bind_rules.clone(),
                                    node_2_props_match.clone(),
                                )
                            ]
                        ))
                        .await
                        .unwrap()
                );
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_then_driver() {
        // Create the Composite Bind rules.
        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("trembler".to_string(), ValueType::Str),
            Symbol::StringValue("thrasher".to_string()),
        )];

        let additional_node_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key("thrasher".to_string(), ValueType::Str),
                Symbol::StringValue("catbird".to_string()),
            ),
            make_abort_ne_symbinst(
                Symbol::Key("catbird".to_string(), ValueType::Number),
                Symbol::NumberValue(1),
            ),
        ];

        let bind_rules = CompositeBindRules {
            device_name: "mimid".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "catbird".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "mockingbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![],
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        // Make the composite driver.
        let url =
            cm_types::Url::new("fuchsia-pkg://fuchsia.com/package#driver/dg_matched_composite.cm")
                .unwrap();

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        // Start our index out without any drivers.
        let index = Rc::new(Indexer::new(std::vec![], BaseRepo::NotResolved, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let node_1_bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let node_1_props_match = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let node_2_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let node_2_props_match = vec![
                make_property("catbird", fdf::NodePropertyValue::IntValue(1)),
                make_property(
                    "thrasher",
                    fdf::NodePropertyValue::StringValue("catbird".to_string()),
                ),
            ];

            // When we add the spec it should get not found since there's no drivers.
            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(
                        &make_composite_spec("test_spec",
                        vec![
                            make_parent_spec(
                                 node_1_bind_rules.clone(),
                                 node_1_props_match.clone(),
                            ),
                            make_parent_spec(
                                node_2_bind_rules.clone(),
                                node_2_props_match,
                            ),
                        ]
                    ))
                    .await
                    .unwrap()
            );

            let device_properties_match = vec![
                make_property("testkey_1", fdf::NodePropertyValue::IntValue(200)),
                make_property("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];
            let match_args = fdi::MatchDriverArgs {
                properties: Some(device_properties_match),
                ..Default::default()
            };

            // We can see the spec comes back without a matched driver.
            let match_result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            if let fdi::MatchDriverResult::CompositeParents(info) = match_result {
                assert_eq!(None, info[0].composite.as_ref().unwrap().matched_driver);
            } else {
                assert!(false, "Did not get back a spec.");
            }

            // Notify the spec manager of a new composite driver.
            {
                let mut composite_node_spec_manager =
                    index.composite_node_spec_manager.borrow_mut();
                composite_node_spec_manager.new_driver_available(&ResolvedDriver {
                    component_url: url.clone(),
                    bind_rules: rules,
                    bind_bytecode: vec![],
                    colocate: false,
                    device_categories: vec![],
                    fallback: false,
                    package_type: DriverPackageType::Base,
                    package_hash: None,
                    is_dfv2: None,
                    disabled: false,
                });
            }

            // Now when we get it back, it has the matching composite driver on it.
            let match_result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            if let fdi::MatchDriverResult::CompositeParents(info) = match_result {
                assert_eq!(
                    &"mimid".to_string(),
                    info[0]
                        .composite
                        .as_ref()
                        .unwrap()
                        .matched_driver
                        .as_ref()
                        .unwrap()
                        .composite_driver
                        .as_ref()
                        .unwrap()
                        .composite_name
                        .as_ref()
                        .unwrap()
                );
            } else {
                assert!(false, "Did not get back a spec.");
            }
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_no_optional_then_driver_with_optional() {
        // Create the Composite Bind rules.
        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("trembler".to_string(), ValueType::Str),
            Symbol::StringValue("thrasher".to_string()),
        )];

        let additional_node_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key("thrasher".to_string(), ValueType::Str),
                Symbol::StringValue("catbird".to_string()),
            ),
            make_abort_ne_symbinst(
                Symbol::Key("catbird".to_string(), ValueType::Number),
                Symbol::NumberValue(1),
            ),
        ];

        let optional_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("thrasher".to_string(), ValueType::Str),
            Symbol::StringValue("trembler".to_string()),
        )];

        let bind_rules = CompositeBindRules {
            device_name: "mimid".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "catbird".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "mockingbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![CompositeParent {
                name: "lapwing".to_string(),
                instructions: optional_node_inst,
            }],
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        // Make the composite driver.
        let url =
            cm_types::Url::new("fuchsia-pkg://fuchsia.com/package#driver/dg_matched_composite.cm")
                .unwrap();

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        // Start our index out without any drivers.
        let index = Rc::new(Indexer::new(std::vec![], BaseRepo::NotResolved, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let node_1_bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let node_1_props_match = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let node_2_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let node_2_props_match = vec![
                make_property("catbird", fdf::NodePropertyValue::IntValue(1)),
                make_property(
                    "thrasher",
                    fdf::NodePropertyValue::StringValue("catbird".to_string()),
                ),
            ];

            // When we add the spec it should get not found since there's no drivers.
            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(
                        &make_composite_spec("test_spec",
                            vec![make_parent_spec(
                                 node_1_bind_rules.clone(),
                                 node_1_props_match.clone(),
                            ),
                            make_parent_spec(
                                node_2_bind_rules.clone(),
                                node_2_props_match,
                            )]))
                    .await
                    .unwrap()
            );

            let device_properties_match = vec![
                make_property("testkey_1", fdf::NodePropertyValue::IntValue(200)),
                make_property("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];
            let match_args = fdi::MatchDriverArgs {
                properties: Some(device_properties_match),
                ..Default::default()
            };

            // We can see the spec comes back without a matched drive.
            let match_result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            if let fdi::MatchDriverResult::CompositeParents(info) = match_result {
                assert_eq!(None, info[0].composite.as_ref().unwrap().matched_driver);
            } else {
                assert!(false, "Did not get back a spec.");
            }

            // Notify the spec manager of a new composite driver.
            {
                let mut composite_node_spec_manager =
                    index.composite_node_spec_manager.borrow_mut();
                composite_node_spec_manager.new_driver_available(&ResolvedDriver {
                    component_url: url.clone(),
                    bind_rules: rules,
                    bind_bytecode: vec![],
                    colocate: false,
                    device_categories: vec![],
                    fallback: false,
                    package_type: DriverPackageType::Base,
                    package_hash: None,
                    is_dfv2: None,
                    disabled: false,
                });
            }

            // Now when we get it back, it has the matching composite driver on it.
            let match_result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            if let fdi::MatchDriverResult::CompositeParents(info) = match_result {
                assert_eq!(
                    &"mimid".to_string(),
                    info[0]
                        .composite
                        .as_ref()
                        .unwrap()
                        .matched_driver
                        .as_ref()
                        .unwrap()
                        .composite_driver
                        .as_ref()
                        .unwrap()
                        .composite_name
                        .as_ref()
                        .unwrap()
                );
            } else {
                assert!(false, "Did not get back a spec.");
            }
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_with_optional_then_driver_with_optional() {
        // Create the Composite Bind rules.
        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("trembler".to_string(), ValueType::Str),
            Symbol::StringValue("thrasher".to_string()),
        )];

        let additional_node_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key("thrasher".to_string(), ValueType::Str),
                Symbol::StringValue("catbird".to_string()),
            ),
            make_abort_ne_symbinst(
                Symbol::Key("catbird".to_string(), ValueType::Number),
                Symbol::NumberValue(1),
            ),
        ];

        let optional_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("thrasher".to_string(), ValueType::Str),
            Symbol::StringValue("trembler".to_string()),
        )];

        let bind_rules = CompositeBindRules {
            device_name: "mimid".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "catbird".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "mockingbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![CompositeParent {
                name: "lapwing".to_string(),
                instructions: optional_node_inst,
            }],
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        // Make the composite driver.
        let url =
            cm_types::Url::new("fuchsia-pkg://fuchsia.com/package#driver/dg_matched_composite.cm")
                .unwrap();

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        // Start our index out without any drivers.
        let index = Rc::new(Indexer::new(std::vec![], BaseRepo::NotResolved, false));

        let index_task = run_index_server(index.clone(), stream).fuse();
        let test_task = async move {
            let node_1_bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            let node_1_props_match = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let node_2_bind_rules =
                vec![make_accept("testkey_1", fdf::NodePropertyValue::IntValue(10))];

            let node_2_props_match = vec![
                make_property("catbird", fdf::NodePropertyValue::IntValue(1)),
                make_property(
                    "thrasher",
                    fdf::NodePropertyValue::StringValue("catbird".to_string()),
                ),
            ];

            let optional_1_bind_rules =
                vec![make_accept("testkey_2", fdf::NodePropertyValue::IntValue(10))];

            let optional_1_props_match = vec![fdf::NodeProperty2 {
                key: "thrasher".to_string(),
                value: fdf::NodePropertyValue::StringValue("trembler".to_string()),
            }];

            // When we add the spec it should get not found since there's no drivers.
            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec("test_spec",
                        vec![make_parent_spec(
                                 node_1_bind_rules.clone(),
                                 node_1_props_match.clone(),
                            ),
                            make_parent_spec(
                                node_2_bind_rules.clone(),
                                node_2_props_match,
                            ),
                            make_parent_spec(optional_1_bind_rules.clone(),
                                optional_1_props_match
                        )]))
                    .await
                    .unwrap()
            );

            let device_properties_match = vec![fdf::NodeProperty2 {
                key: "testkey_2".to_string(),
                value: fdf::NodePropertyValue::IntValue(10),
            }];
            let match_args = fdi::MatchDriverArgs {
                properties: Some(device_properties_match),
                ..Default::default()
            };

            // We can see the spec comes back without a matched composite.
            let match_result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            if let fdi::MatchDriverResult::CompositeParents(info) = match_result {
                assert_eq!(None, info[0].composite.as_ref().unwrap().matched_driver);
            } else {
                assert!(false, "Did not get back a spec.");
            }

            // Notify the spec manager of a new composite driver.
            {
                let mut composite_node_spec_manager =
                    index.composite_node_spec_manager.borrow_mut();
                composite_node_spec_manager.new_driver_available(&ResolvedDriver {
                    component_url: url.clone(),
                    bind_rules: rules,
                    bind_bytecode: vec![],
                    colocate: false,
                    device_categories: vec![],
                    fallback: false,
                    package_type: DriverPackageType::Base,
                    package_hash: None,
                    is_dfv2: None,
                    disabled: false,
                });
            }

            // Now when we get it back, it has the matching composite driver on it.
            let match_result = proxy.match_driver(&match_args).await.unwrap().unwrap();
            if let fdi::MatchDriverResult::CompositeParents(info) = match_result {
                assert_eq!(
                    &"mimid".to_string(),
                    info[0]
                        .composite
                        .as_ref()
                        .unwrap()
                        .matched_driver
                        .as_ref()
                        .unwrap()
                        .composite_driver
                        .as_ref()
                        .unwrap()
                        .composite_name
                        .as_ref()
                        .unwrap()
                );
            } else {
                assert!(false, "Did not get back a spec.");
            }
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_duplicate_path() {
        let always_match_rules = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match_rules).unwrap(),
        )
        .unwrap();

        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: cm_types::Url::new(
                "fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm"
            )
            .unwrap(),
            bind_rules: always_match,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();
        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));
        let index_task = run_index_server(index.clone(), stream).fuse();

        let test_task = async move {
            let bind_rules = vec![
                make_accept_list(
                    "testkey_1",
                    vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                ),
                make_accept("lapwing", fdf::NodePropertyValue::StringValue("plover".to_string())),
            ];

            assert_eq!(
                Ok(()),
                proxy
                    .add_composite_node_spec(&make_composite_spec(
                        "test_spec",
                        vec![make_parent_spec(
                            bind_rules,
                            vec![make_property(
                                "trembler",
                                fdf::NodePropertyValue::StringValue("thrasher".to_string())
                            )]
                        )]
                    ))
                    .await
                    .unwrap()
            );

            let duplicate_bind_rules =
                vec![make_accept("dupe_key", fdf::NodePropertyValue::IntValue(2))];

            let node_transform = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let result = proxy
                .add_composite_node_spec(&make_composite_spec(
                    "test_spec",
                    vec![make_parent_spec(duplicate_bind_rules, node_transform)],
                ))
                .await
                .unwrap();
            assert_eq!(Err(Status::ALREADY_EXISTS.into_raw()), result);
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_add_composite_node_spec_duplicate_key() {
        let always_match_rules = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match_rules).unwrap(),
        )
        .unwrap();

        let base_repo = BaseRepo::Resolved(std::vec![ResolvedDriver {
            component_url: cm_types::Url::new(
                "fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm"
            )
            .unwrap(),
            bind_rules: always_match,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        },]);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();
        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));
        let index_task = run_index_server(index.clone(), stream).fuse();

        let test_task = async move {
            let bind_rules = vec![
                fdf::BindRule2 {
                    key: "testkey20".to_string(),
                    condition: fdf::Condition::Accept,
                    values: vec![
                        fdf::NodePropertyValue::IntValue(200),
                        fdf::NodePropertyValue::IntValue(150),
                    ],
                },
                fdf::BindRule2 {
                    key: "testkey20".to_string(),
                    condition: fdf::Condition::Accept,
                    values: vec![fdf::NodePropertyValue::StringValue("plover".to_string())],
                },
            ];

            let node_transform = vec![make_property(
                "trembler",
                fdf::NodePropertyValue::StringValue("thrasher".to_string()),
            )];

            let result = proxy
                .add_composite_node_spec(&make_composite_spec(
                    "test_spec",
                    vec![make_parent_spec(bind_rules, node_transform)],
                ))
                .await
                .unwrap();
            assert_eq!(Err(Status::INVALID_ARGS.into_raw()), result);
        }
        .fuse();

        futures::pin_mut!(index_task, test_task);
        futures::select! {
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_register_and_match_ephemeral_driver() {
        let component_manifest_url =
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm";

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fdi::DriverIndexMarker>();

        let (registrar_proxy, registrar_stream) =
            fidl::endpoints::create_proxy_and_stream::<fdr::DriverRegistrarMarker>();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let full_resolver = Some(resolver);

        let base_repo = BaseRepo::Resolved(std::vec![]);
        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        let index_task = run_index_server(index.clone(), stream).fuse();
        let registrar_task =
            run_driver_registrar_server(index.clone(), registrar_stream, &full_resolver).fuse();

        let test_task = async {
            // Short delay since the resolver server is starting up at the same time.
            fasync::Timer::new(std::time::Duration::from_millis(100)).await;

            // These properties match the bind rules for the "test-bind-componenet.cm".
            let property = fdf::NodeProperty2 {
                key: "fuchsia.BIND_PROTOCOL".to_string(),
                value: fdf::NodePropertyValue::IntValue(1),
            };
            let args =
                fdi::MatchDriverArgs { properties: Some(vec![property]), ..Default::default() };

            // First attempt should fail since we haven't registered it.
            let result = proxy.match_driver(&args).await.unwrap();
            assert_eq!(result, Err(Status::NOT_FOUND.into_raw()));

            // Now register the ephemeral driver.
            registrar_proxy.register(component_manifest_url).await.unwrap().unwrap();

            // Match succeeds now.
            let result = proxy.match_driver(&args).await.unwrap().unwrap();
            let expected_url =
                "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm"
                    .to_string();

            match result {
                fdi::MatchDriverResult::Driver(d) => {
                    assert_eq!(expected_url, d.url.unwrap());
                }
                fdi::MatchDriverResult::CompositeParents(p) => {
                    panic!("Bad match driver: {:#?}", p);
                }
                _ => panic!("Bad case"),
            }
        }
        .fuse();

        futures::pin_mut!(resolver_task, index_task, registrar_task, test_task);
        futures::select! {
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
            result = index_task => {
                panic!("Index task finished: {:?}", result);
            },
            result = registrar_task => {
                panic!("Registrar task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_register_and_get_ephemeral_driver() {
        let component_manifest_url =
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm";

        let (registrar_proxy, registrar_stream) =
            fidl::endpoints::create_proxy_and_stream::<fdr::DriverRegistrarMarker>();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let (development_proxy, development_stream) =
            fidl::endpoints::create_proxy_and_stream::<fdi::DevelopmentManagerMarker>();

        let full_resolver = Some(resolver);

        let base_repo = BaseRepo::Resolved(std::vec![]);
        let index = Rc::new(Indexer::new(std::vec![], base_repo, false));

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        let development_task =
            run_driver_development_server(index.clone(), development_stream).fuse();
        let registrar_task =
            run_driver_registrar_server(index.clone(), registrar_stream, &full_resolver).fuse();
        let test_task = async move {
            // We should not find this before registering it.
            let driver_infos =
                get_driver_info_proxy(&development_proxy, &[component_manifest_url.to_string()])
                    .await;
            assert_eq!(0, driver_infos.len());

            // Short delay since the resolver server starts at the same time.
            fasync::Timer::new(std::time::Duration::from_millis(100)).await;

            // Register the ephemeral driver.
            registrar_proxy.register(component_manifest_url).await.unwrap().unwrap();

            // Now that it's registered we should find it.
            let driver_infos =
                get_driver_info_proxy(&development_proxy, &[component_manifest_url.to_string()])
                    .await;
            assert_eq!(1, driver_infos.len());
            assert_eq!(&component_manifest_url.to_string(), driver_infos[0].url.as_ref().unwrap());
        }
        .fuse();

        futures::pin_mut!(resolver_task, development_task, registrar_task, test_task);
        futures::select! {
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
            result = development_task => {
                panic!("Development task finished: {:?}", result);
            },
            result = registrar_task => {
                panic!("Registrar task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_register_exists_in_base() {
        let always_match_rules = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match_rules).unwrap(),
        )
        .unwrap();

        let component_manifest_url =
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm";

        let (registrar_proxy, registrar_stream) =
            fidl::endpoints::create_proxy_and_stream::<fdr::DriverRegistrarMarker>();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let full_resolver = Some(resolver);

        let boot_repo = std::vec![];
        let base_repo = BaseRepo::Resolved(vec![ResolvedDriver {
            component_url: cm_types::Url::new(component_manifest_url).unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }]);

        let index = Rc::new(Indexer::new(boot_repo, base_repo, false));

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        let registrar_task =
            run_driver_registrar_server(index.clone(), registrar_stream, &full_resolver).fuse();
        let test_task = async move {
            // Try to register the driver.
            let register_result = registrar_proxy.register(component_manifest_url).await.unwrap();

            // The register should have failed.
            assert_eq!(zx::sys::ZX_ERR_ALREADY_EXISTS, register_result.err().unwrap());
        }
        .fuse();

        futures::pin_mut!(resolver_task, registrar_task, test_task);
        futures::select! {
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
            result = registrar_task => {
                panic!("Registrar task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_register_exists_in_boot() {
        let always_match_rules = bind::compiler::BindRules {
            instructions: vec![],
            symbol_table: HashMap::new(),
            use_new_bytecode: true,
            enable_debug: false,
        };
        let always_match = DecodedRules::new(
            bind::bytecode_encoder::encode_v2::encode_to_bytecode_v2(always_match_rules).unwrap(),
        )
        .unwrap();

        let component_manifest_url =
            "fuchsia-pkg://fuchsia.com/driver-index-unittests#meta/test-bind-component.cm";

        let (registrar_proxy, registrar_stream) =
            fidl::endpoints::create_proxy_and_stream::<fdr::DriverRegistrarMarker>();

        let (resolver, resolver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();

        let full_resolver = Some(resolver);

        let boot_repo = vec![ResolvedDriver {
            component_url: cm_types::Url::new(component_manifest_url).unwrap(),
            bind_rules: always_match.clone(),
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Boot,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }];

        let base_repo = BaseRepo::Resolved(std::vec![]);
        let index = Rc::new(Indexer::new(boot_repo, base_repo, false));

        let resolver_task = run_resolver_server(resolver_stream).fuse();
        let registrar_task =
            run_driver_registrar_server(index.clone(), registrar_stream, &full_resolver).fuse();
        let test_task = async move {
            // Try to register the driver.
            let register_result = registrar_proxy.register(component_manifest_url).await.unwrap();

            // The register should have failed.
            assert_eq!(zx::sys::ZX_ERR_ALREADY_EXISTS, register_result.err().unwrap());
        }
        .fuse();

        futures::pin_mut!(resolver_task, registrar_task, test_task);
        futures::select! {
            result = resolver_task => {
                panic!("Resolver task finished: {:?}", result);
            },
            result = registrar_task => {
                panic!("Registrar task finished: {:?}", result);
            },
            () = test_task => {},
        }
    }

    #[fuchsia::test]
    async fn test_get_device_categories_from_component_data() {
        assert_eq!(
            resolved_driver::get_device_categories_from_component_data(&vec![
                fdata::Dictionary {
                    entries: Some(vec![fdata::DictionaryEntry {
                        key: "category".to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("usb".to_string())))
                    }]),
                    ..Default::default()
                },
                fdata::Dictionary {
                    entries: Some(vec![
                        fdata::DictionaryEntry {
                            key: "category".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                "connectivity".to_string()
                            ))),
                        },
                        fdata::DictionaryEntry {
                            key: "subcategory".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                "ethernet".to_string()
                            ))),
                        }
                    ]),
                    ..Default::default()
                }
            ]),
            vec![
                DeviceCategoryDef {
                    category: Some("usb".to_string()),
                    subcategory: None,
                    ..Default::default()
                },
                DeviceCategoryDef {
                    category: Some("connectivity".to_string()),
                    subcategory: Some("ethernet".to_string()),
                    ..Default::default()
                }
            ]
        );
    }
}
