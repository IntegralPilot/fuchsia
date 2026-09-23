// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cpu_manager::CpuManager;
use crate::events::SagEventLogger;
use crate::system_activity_governor::SystemActivityGovernor;
use anyhow::Result;
use async_lock::OnceCell;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_hardware_power_suspend as fhsuspend;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as INode;
use futures::StreamExt;
use futures::future::LocalBoxFuture;
use power_broker_client::PowerElementContext;
use std::cell::Cell;
use std::rc::Rc;
use zx::Rights;

/// SystemActivityGovernorFactory is a function trait used to construct a new
/// SystemActivityGovernor instance. The parameters are provided by an instance
/// of CpuElementManager.
pub trait SystemActivityGovernorFactory:
    Fn(
        /*cpu_manager:*/ Rc<CpuManager>,
        /*execution_state_level_dependencies:*/ Vec<fbroker::LevelDependency>,
    ) -> LocalBoxFuture<'static, Result<()>>
    + 'static
{
}

// SystemActivityGovernorFactory should be implemented for all functions with
// a matching signature.
impl<T> SystemActivityGovernorFactory for T where
    T: Fn(
            /*cpu_manager:*/ Rc<CpuManager>,
            /*execution_state_level_dependencies:*/ Vec<fbroker::LevelDependency>,
        ) -> LocalBoxFuture<'static, Result<()>>
        + 'static
{
}

pub struct CpuElementManager<F>
where
    F: SystemActivityGovernorFactory,
{
    cpu_assertive_dependency_token: fbroker::DependencyToken,
    cpu_manager: Rc<CpuManager>,
    sag: Rc<OnceCell<Rc<SystemActivityGovernor>>>,
    sag_factory: F,
}

impl<F> CpuElementManager<F>
where
    F: SystemActivityGovernorFactory,
{
    pub async fn new_wait_for_suspending_token(
        topology: &fbroker::TopologyProxy,
        inspect_root: INode,
        sag_event_logger: SagEventLogger,
        suspender: Rc<OnceCell<Option<fhsuspend::SuspenderProxy>>>,
        sag: Rc<OnceCell<Rc<SystemActivityGovernor>>>,
        sag_factory: F,
        observability: Rc<OnceCell<crate::power_observability::WakeSourceObservability>>,
        is_shutting_down: Rc<Cell<bool>>,
    ) -> Rc<Self> {
        log::info!("Creating CPU power element");
        let (element_runner_client, element_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        let cpu = Rc::new(
            PowerElementContext::builder(
                topology,
                "cpu",
                &[
                    fsystem::CpuLevel::Inactive.into_primitive(),
                    fsystem::CpuLevel::Active.into_primitive(),
                ],
                element_runner_client,
            )
            .build()
            .await
            .expect("PowerElementContext encountered error while building cpu"),
        );

        let power_elements_node = inspect_root.create_child("power_elements");
        let power_elements_node2 = power_elements_node.clone_weak();
        inspect_root.record(power_elements_node);

        let cpu_manager = Rc::new(CpuManager::new(
            cpu.clone(),
            suspender,
            sag_event_logger,
            observability,
            is_shutting_down,
        ));
        cpu_manager.run(element_runner, &power_elements_node2);

        log::info!("Leasing CPU power element");
        let (server_token, cpu_lease) = fbroker::LeaseToken::create();
        topology
            .lease(fbroker::LeaseSchema {
                lease_token: Some(server_token),
                lease_name: Some("cpu_boot_control".into()),
                dependencies: Some(vec![fbroker::LeaseDependency {
                    requires_token: Some(
                        cpu.assertive_dependency_token().expect("token not available"),
                    ),
                    requires_level: Some(fsystem::CpuLevel::Active.into_primitive()),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .await
            .expect("failed to reach topology during startup")
            .expect("failed to lease CPU element during startup");
        log::info!("Leased CPU power element at 'Active'.");

        let sag2 = sag.clone();
        let cpu_manager2 = cpu_manager.clone();

        fasync::Task::local(async move {
            let sag = sag2.wait().await;

            log::info!("Starting activity governor server...");
            cpu_manager2.set_suspend_resume_listener(sag.clone());
            sag.run(&power_elements_node2).await.expect("failed to run activity governor server");

            log::info!("Running activity governor server, dropping CPU lease");
            drop(cpu_lease);
        })
        .detach();

        Rc::new(Self {
            cpu_assertive_dependency_token: cpu
                .assertive_dependency_token()
                .expect("token not available"),
            cpu_manager,
            sag,
            sag_factory,
        })
    }

    pub async fn new(
        topology: &fbroker::TopologyProxy,
        inspect_root: INode,
        sag_event_logger: SagEventLogger,
        suspender: Rc<OnceCell<Option<fhsuspend::SuspenderProxy>>>,
        sag: Rc<OnceCell<Rc<SystemActivityGovernor>>>,
        sag_factory: F,
        observability: Rc<OnceCell<crate::power_observability::WakeSourceObservability>>,
        is_shutting_down: Rc<Cell<bool>>,
    ) -> Rc<Self> {
        let manager = Self::new_wait_for_suspending_token(
            topology,
            inspect_root,
            sag_event_logger,
            suspender,
            sag,
            sag_factory,
            observability,
            is_shutting_down,
        )
        .await;

        // No dependencies will be provided, so construct SystemActivityGovernor now.
        let _ = (manager.sag_factory)(
            manager.cpu_manager.clone(),
            manager.create_execution_state_level_dependencies(Vec::new()),
        )
        .await
        .expect("create sag failed");
        manager
    }

    pub async fn sag(&self) -> Rc<SystemActivityGovernor> {
        self.sag.wait().await.clone()
    }

    fn cpu_assertive_dependency_token(&self) -> fbroker::DependencyToken {
        self.cpu_assertive_dependency_token
            .duplicate_handle(Rights::SAME_RIGHTS)
            .expect("failed to duplicate token")
    }

    fn create_execution_state_level_dependencies(
        &self,
        mut extra_dependencies: Vec<fbroker::LevelDependency>,
    ) -> Vec<fbroker::LevelDependency> {
        extra_dependencies.push(fbroker::LevelDependency {
            dependent_level: Some(fsystem::ExecutionStateLevel::Suspending.into_primitive()),
            requires_token: Some(self.cpu_assertive_dependency_token()),
            requires_level_by_preference: Some(vec![fsystem::CpuLevel::Active.into_primitive()]),
            ..Default::default()
        });
        extra_dependencies
    }

    pub async fn handle_cpu_element_manager_stream(
        &self,
        mut stream: fsystem::CpuElementManagerRequestStream,
    ) {
        while let Some(request) = stream.next().await {
            match request {
                Ok(fsystem::CpuElementManagerRequest::GetCpuDependencyToken { responder }) => {
                    if let Err(error) = responder.send(fsystem::Cpu {
                        assertive_dependency_token: Some(self.cpu_assertive_dependency_token()),
                        ..Default::default()
                    }) {
                        log::warn!(
                            error:?;
                            "Encountered error while responding to GetCpuDependencyToken request"
                        );
                    }
                }
                Ok(fsystem::CpuElementManagerRequest::AddExecutionStateDependency {
                    payload,
                    responder,
                }) => {
                    // To handle this request, both fields must be provided.
                    let response = match payload {
                        fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                            dependency_token: Some(dependency_token),
                            power_level: Some(power_level),
                            ..
                        } => {
                            // SAG can only be created once. If it already exists,
                            // no more dependencies can be added. Trying to add more
                            // dependencies after construction is an error and
                            // indicates a bug in the device's configuration.
                            if self.sag.is_initialized() {
                                log::error!("System Activity Governor is already created.");
                                Err(fsystem::AddExecutionStateDependencyError::BadState)
                            } else {
                                // We have a valid request to add an Execution State
                                // dependency, so we can construct SAG now.
                                log::info!("Adding execution state dependency");

                                match (self.sag_factory)(
                                    self.cpu_manager.clone(),
                                    self.create_execution_state_level_dependencies(vec![
                                        fbroker::LevelDependency {
                                            dependent_level: Some(
                                                fsystem::ExecutionStateLevel::Suspending
                                                    .into_primitive(),
                                            ),
                                            requires_token: Some(dependency_token),
                                            requires_level_by_preference: Some(vec![power_level]),
                                            ..Default::default()
                                        },
                                    ]),
                                )
                                .await
                                {
                                    Ok(()) => Ok(()),
                                    Err(e) => {
                                        log::error!(
                                            "Failed to create System Activity Governor: {e:?}"
                                        );
                                        Err(fsystem::AddExecutionStateDependencyError::BadState)
                                    }
                                }
                            }
                        }
                        payload => {
                            log::warn!(
                                "Invalid payload for AddExecutionStateDependency: {payload:?}"
                            );
                            Err(fsystem::AddExecutionStateDependencyError::InvalidArgs)
                        }
                    };

                    // At this point, SAG is constructed but may not be
                    // handling FIDL requests. Full initialization will
                    // occur asynchronously while responding to the caller.
                    if let Err(error) = responder.send(response) {
                        log::warn!(
                            error:?;
                            "Encountered error while responding to AddExecutionStateDependency request"
                        );
                    }
                }
                Ok(fsystem::CpuElementManagerRequest::_UnknownMethod { ordinal, .. }) => {
                    log::warn!(ordinal:?; "Unknown CpuElementManagerRequest method");
                }
                Err(error) => {
                    log::error!(error:?; "Error handling CpuElementManager request stream");
                }
            }
        }
    }
}
