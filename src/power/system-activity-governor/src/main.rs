// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod activity_governor_request_frontend;
mod cpu_element_manager;
mod cpu_manager;
mod events;
mod power_observability;
mod system_activity_governor;

use crate::activity_governor_request_frontend::ActivityGovernorRequestFrontend;
use crate::cpu_element_manager::{CpuElementManager, SystemActivityGovernorFactory};
use crate::events::SagEventLogger;
use crate::power_observability::WakeSourceObservability;
use crate::system_activity_governor::SystemActivityGovernor;
use anyhow::{Context, Result};
use async_lock::OnceCell;
use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_hardware_platform_bus as ffhpb;
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fidl_fuchsia_hardware_power_suspend as fhsuspend;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fidl_fuchsia_power_suspend as fsuspend;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_component::client::{Service, connect_to_protocol};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::health::Reporter;
use fuchsia_inspect::{BoolProperty as IBool, Property};
use futures::{FutureExt, StreamExt, TryStreamExt};
use inspect_format::constants::DEFAULT_VMO_SIZE_BYTES as DEFAULT_INSPECT_VMO;
use sag_config::Config;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use zx::MonotonicDuration;

const SUSPEND_DEVICE_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(10);
const SUSPENDER_CONNECT_RETRY_DELAY: Duration = Duration::from_secs(3);
const ROLE_NAME: &str = "fuchsia.power.system.activity_governor";

async fn connect_to_suspender() -> Result<fhsuspend::SuspenderProxy> {
    Service::open(fhsuspend::SuspendServiceMarker)?
        .watch_for_any()
        .on_timeout(SUSPEND_DEVICE_TIMEOUT.after_now(), || {
            Err(anyhow::anyhow!("Timeout waiting for next watcher message."))
        })
        .await?
        .connect_to_suspender()
        .map_err(|e| anyhow::anyhow!("Failed to connect to suspender: {:?}", e))
}

async fn connect_to_interrupt_attributor() -> Result<ffhpb::InterruptAttributorProxy> {
    Service::open(ffhpb::ObservabilityServiceMarker)?
        .watch_for_any()
        .on_timeout(SUSPEND_DEVICE_TIMEOUT.after_now(), || {
            Err(anyhow::anyhow!("Timeout waiting for next watcher message."))
        })
        .await
        .context("while connecting to fuchsia.hardware.platform.bus/ObservabilityService")?
        .connect_to_interrupt()
        .map_err(|e| anyhow::anyhow!("Failed to connect to interrupt_attributor: {:?}", e))
}

enum IncomingService {
    ActivityGovernor(fsystem::ActivityGovernorRequestStream),
    ExecutionStateManager(fsystem::ExecutionStateManagerRequestStream),
    BootControl(fsystem::BootControlRequestStream),
    CpuElementManager(fsystem::CpuElementManagerRequestStream),
    Stats(fsuspend::StatsRequestStream),
    ElementInfoProviderService(fbroker::ElementInfoProviderServiceRequest),
}

async fn run<F>(
    cpu_service: Rc<CpuElementManager<F>>,
    booting_node: Rc<IBool>,
    front_end: Rc<ActivityGovernorRequestFrontend>,
) -> Result<()>
where
    F: SystemActivityGovernorFactory,
{
    let mut service_fs = ServiceFs::new_local();

    service_fs
        .dir("svc")
        .add_fidl_service(IncomingService::ActivityGovernor)
        .add_fidl_service(IncomingService::ExecutionStateManager)
        .add_fidl_service(IncomingService::BootControl)
        .add_fidl_service(IncomingService::Stats)
        .add_fidl_service(IncomingService::CpuElementManager)
        .add_fidl_service_instance(
            "system_activity_governor",
            IncomingService::ElementInfoProviderService,
        );
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    service_fs
        .for_each_concurrent(None, move |request: IncomingService| {
            let cpu_service = cpu_service.clone();
            let booting_node = booting_node.clone();
            let front_end = front_end.clone();
            // Before constructing the SystemActivityGovernor type, the system-activity-governor
            // component must receive a token from another component. To ensure components that
            // depend on fuchsia.power.system.ActivityGovernor, et. al. have consistent behavior,
            // this component only handles messages from fuchsia.power.system.CpuElementManager
            // until the SystemActivityGovernor type is constructed.
            async move {
                match request {
                    IncomingService::ActivityGovernor(stream) => {
                        front_end.handle_activity_governor_stream(stream).await
                    }
                    IncomingService::ExecutionStateManager(stream) => {
                        front_end.handle_execution_state_manager_stream(stream).await
                    }
                    IncomingService::BootControl(stream) => {
                        cpu_service
                            .sag()
                            .await
                            .handle_boot_control_stream(stream, booting_node)
                            .await
                    }
                    IncomingService::CpuElementManager(stream) => {
                        cpu_service.handle_cpu_element_manager_stream(stream).await
                    }
                    IncomingService::Stats(stream) => {
                        cpu_service.sag().await.handle_stats_stream(stream).await
                    }
                    IncomingService::ElementInfoProviderService(
                        fbroker::ElementInfoProviderServiceRequest::StatusProvider(stream),
                    ) => cpu_service.sag().await.handle_element_info_provider_stream(stream).await,
                }
            }
        })
        .await;

    Ok(())
}

async fn register_terminal_state_watcher(
    is_shutting_down_node: fuchsia_inspect::BoolProperty,
) -> Rc<Cell<bool>> {
    let is_shutting_down = Rc::new(Cell::new(false));

    log::info!("Attempting to connect to ShutdownWatcherRegister...");
    let shutdown_watcher_register =
        connect_to_protocol::<fstatecontrol::ShutdownWatcherRegisterMarker>()
            .expect("Failed to connect to ShutdownWatcherRegister");

    let is_shutting_down_clone = is_shutting_down.clone();
    let (client_end, mut stream) = fidl::endpoints::create_request_stream();
    shutdown_watcher_register
        .register_terminal_state_watcher(client_end)
        .await
        .expect("Failed to register TerminalStateWatcher");

    fasync::Task::local(async move {
        log::info!("Waiting for OnTerminalStateTransitionStarted...");
        while let Ok(Some(req)) = stream.try_next().await {
            match req {
                fstatecontrol::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted {
                    responder,
                } => {
                    log::info!("Received OnTerminalStateTransitionStarted request");
                    is_shutting_down_clone.set(true);
                    is_shutting_down_node.set(true);
                    responder
                        .send()
                        .expect("Failed to send OnTerminalStateTransitionStarted response");
                }
                _ => {
                    log::warn!("Unexpected TerminalStateWatcherRequest");
                }
            }
        }
    })
    .detach();

    is_shutting_down
}

#[fuchsia::main]
async fn main() -> Result<()> {
    log::info!("started");

    if let Err(e) = fuchsia_scheduler::set_role_for_root_vmar(ROLE_NAME) {
        log::warn!(e:%; "failed to set vmar role");
    }

    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let inspect_vmo_size = (2.5f32 * DEFAULT_INSPECT_VMO as f32) as usize;
    let inspector = fuchsia_inspect::component::init_inspector_with_size(inspect_vmo_size);
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());
    fuchsia_inspect::component::serve_inspect_stats();
    fuchsia_inspect::component::health().set_starting_up();

    let config = Config::take_from_startup_handle();
    let config = std::rc::Rc::new(config);
    inspector.root().record_child("config", |config_node| config.record_inspect(config_node));

    // Set up the SystemActivityGovernor.
    log::info!(config:?; "config");

    let suspender: Rc<OnceCell<Option<fhsuspend::SuspenderProxy>>> = Rc::new(OnceCell::new());
    let suspender_copy = suspender.clone();
    if config.use_suspender {
        fasync::Task::local(async move {
            loop {
                log::info!("Attempting to connect to suspender...");
                match connect_to_suspender().await {
                    Ok(s) => {
                        log::info!("Connected to suspender");
                        suspender_copy
                            .set(Some(s))
                            .await
                            .expect("suspender unexpectedly set twice");
                        break;
                    }
                    Err(e) => {
                        log::error!("Unable to connect to suspender protocol: {e:?}");
                    }
                }
                // Delay retry for some time to reduce log spam.
                fuchsia_async::Timer::new(SUSPENDER_CONNECT_RETRY_DELAY).await;
            }
        })
        .detach();
    } else {
        suspender
            .set(None)
            .await
            .expect("Suspender unexpectedly already set, this should be impossible");
    }

    let topology = connect_to_protocol::<fbroker::TopologyMarker>()?;
    let crash_reporter = connect_to_protocol::<ffeedback::CrashReporterMarker>()?;
    let boost_proxy = connect_to_protocol::<fcpumanager::BoostMarker>()?;
    let sag_event_logger =
        SagEventLogger::new(inspector.root(), config.max_suspend_events_to_log as usize);

    let topology2 = topology.clone();
    let sag_event_logger2 = sag_event_logger.clone();
    let sag_event_logger_obs = sag_event_logger.clone();

    let sag_cell = Rc::new(OnceCell::<Rc<SystemActivityGovernor>>::new());
    let front_end = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell.clone()));

    let is_shutting_down_node = inspector.root().create_bool("is_shutting_down", false);
    let is_shutting_down = register_terminal_state_watcher(is_shutting_down_node).await;
    let is_shutting_down_sag = is_shutting_down.clone();

    let front_end_clone = front_end.clone();
    let inspector_clone = inspector.clone();
    let config_clone = config.clone();
    let sag_factory_fn = move |cpu_manager, execution_state_dependencies| {
        let front_end = front_end_clone.clone();
        let topology = topology2.clone();
        let sag_event_logger = sag_event_logger2.clone();
        let is_shutting_down = is_shutting_down_sag.clone();
        let crash_reporter = crash_reporter.clone();
        let boost_proxy = boost_proxy.clone();
        let inspect_root = inspector_clone.root().clone_weak();
        let config = config_clone.clone();
        async move {
            log::info!("Creating activity governor server...");

            let admin_proxy = if config.reboot_on_stalled_suspend_blocker {
                match connect_to_protocol::<fstatecontrol::AdminMarker>() {
                    Ok(p) => Some(p),
                    Err(e) => {
                        log::error!("Failed to connect to statecontrol::Admin: {:?}", e);
                        None
                    }
                }
            } else {
                None
            };

            front_end
                .create_sag(
                    &topology,
                    inspect_root,
                    sag_event_logger,
                    cpu_manager,
                    execution_state_dependencies,
                    is_shutting_down,
                    crash_reporter,
                    boost_proxy,
                    admin_proxy,
                    &config,
                )
                .await
        }
        .boxed_local()
    };

    let interrupt_attr_cell: Rc<OnceCell<WakeSourceObservability>> = Rc::new(OnceCell::new());
    let interrupt_attributor = interrupt_attr_cell.clone();
    fasync::Task::local(async move {
        let interrupt_attributor = connect_to_interrupt_attributor()
            .await
            .context("while connecting to interrupt_attributor")
            .inspect_err(|err| {
                log::warn!("no interrupt attributor, wake vectors will not be resolved: {err:?}")
            })
            .ok();
        let observer = power_observability::WakeSourceObservability::new(
            interrupt_attributor,
            sag_event_logger_obs,
        );
        interrupt_attr_cell.set(observer).await.expect(
            "Unexpected error setting attribution cell, something else seems to have set it.",
        );
    })
    .detach();

    let cpu_service = if config.wait_for_suspending_token {
        CpuElementManager::new_wait_for_suspending_token(
            &topology,
            inspector.root().clone_weak(),
            sag_event_logger,
            suspender,
            sag_cell.clone(),
            sag_factory_fn,
            interrupt_attributor,
            is_shutting_down,
        )
        .await
    } else {
        CpuElementManager::new(
            &topology,
            inspector.root().clone_weak(),
            sag_event_logger,
            suspender,
            sag_cell.clone(),
            sag_factory_fn,
            interrupt_attributor,
            is_shutting_down,
        )
        .await
    };

    fuchsia_inspect::component::health().set_ok();

    // This future should never complete.
    let booting_node = Rc::new(inspector.root().create_bool("booting", true));
    let result = run(cpu_service, booting_node, front_end).await;
    log::error!(result:?; "Unexpected exit");
    fuchsia_inspect::component::health().set_unhealthy(&format!("Unexpected exit: {:?}", result));
    result
}
