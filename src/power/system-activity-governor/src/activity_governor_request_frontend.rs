// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cpu_manager::CpuManager;
use crate::events::SagEventLogger;
use crate::system_activity_governor::{
    StoredSuspendBlocker, StoredWakeLease, SystemActivityGovernor,
};
use anyhow::Result;
use async_lock::OnceCell;
use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use fuchsia_inspect::Node as INode;
use futures::future::FutureExt;
use futures::{StreamExt, pin_mut, select_biased};
use sag_config::Config;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct ActivityGovernorRequestFrontend {
    sag: Rc<OnceCell<Rc<SystemActivityGovernor>>>,
    stored_wake_leases: RefCell<Vec<StoredWakeLease>>,
    stored_suspend_blockers: RefCell<Vec<StoredSuspendBlocker>>,
    stored_application_activity_leases:
        RefCell<Vec<crate::system_activity_governor::StoredApplicationActivityLease>>,
    get_power_elements_queue: RefCell<Vec<fsystem::ActivityGovernorGetPowerElementsResponder>>,
    get_execution_state_dependency_token_queue:
        RefCell<Vec<fsystem::ExecutionStateManagerGetExecutionStateDependencyTokenResponder>>,
    add_application_activity_dependency_queue: RefCell<
        Vec<(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest,
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyResponder,
        )>,
    >,
}

impl ActivityGovernorRequestFrontend {
    pub fn new(sag: Rc<OnceCell<Rc<SystemActivityGovernor>>>) -> Self {
        Self {
            sag,
            stored_wake_leases: RefCell::new(Vec::new()),
            stored_suspend_blockers: RefCell::new(Vec::new()),
            stored_application_activity_leases: RefCell::new(Vec::new()),
            get_power_elements_queue: RefCell::new(Vec::new()),
            get_execution_state_dependency_token_queue: RefCell::new(Vec::new()),
            add_application_activity_dependency_queue: RefCell::new(Vec::new()),
        }
    }

    // Checks to see if SAG is available. If not, it calls this object's
    // handle_activity_governor_request function. When SAG becomes available, the stream handling
    // is passed to SAG.
    pub async fn handle_activity_governor_stream(
        self: Rc<Self>,
        mut stream: fsystem::ActivityGovernorRequestStream,
    ) {
        let self_clone = self.clone();
        let sag_available_fut = self_clone.sag.wait().fuse();
        pin_mut!(sag_available_fut);

        let sag = loop {
            let mut next_request = stream.next().fuse();
            let result = select_biased! {
                sag = sag_available_fut => break sag,
                next_request = next_request => next_request,
            };

            match result {
                Some(request) => self.clone().handle_activity_governor_request(request).await,
                None => return,
            }
        };

        sag.clone().handle_activity_governor_stream(stream).await;
    }

    pub async fn handle_execution_state_manager_stream(
        self: Rc<Self>,
        mut stream: fsystem::ExecutionStateManagerRequestStream,
    ) {
        let self_clone = self.clone();
        let sag_available_fut = self_clone.sag.wait().fuse();
        pin_mut!(sag_available_fut);

        let sag = loop {
            let mut next_request = stream.next().fuse();
            let result = select_biased! {
                sag = sag_available_fut => break sag,
                next_request = next_request => next_request,
            };

            match result {
                Some(request) => self.clone().handle_execution_state_manager_request(request).await,
                None => return,
            }
        };

        sag.clone().handle_execution_state_manager_stream(stream).await;
    }

    /// Handles the requests for the `fuchsia.power.system.ActivityGovernor` protocol.
    /// For lease-related requests these always return immediately even before
    /// SystemActivityGovernor is created. Lease tokens are created and will be registered
    /// when SystemActivityGovernor is created.
    ///
    /// For RegisterSuspendBlocker and GetPowerElements the requests are accumulated and they will
    /// get a response after SystemActivityGovernor is created.
    async fn handle_activity_governor_request(
        &self,
        request: Result<fsystem::ActivityGovernorRequest, fidl::Error>,
    ) {
        match request {
            Ok(fsystem::ActivityGovernorRequest::GetPowerElements { responder }) => {
                self.get_power_elements_queue.borrow_mut().push(responder);
            }
            Ok(fsystem::ActivityGovernorRequest::TakeApplicationActivityLease {
                responder,
                name,
            }) => {
                let (server_token, client_token) = fsystem::LeaseToken::create();
                let _ = responder.send(client_token);
                self.stored_application_activity_leases.borrow_mut().push(
                    crate::system_activity_governor::StoredApplicationActivityLease {
                        name,
                        server_token,
                    },
                );
            }
            Ok(fsystem::ActivityGovernorRequest::AcquireWakeLease { responder, name }) => {
                let (server_token, client_token) = fsystem::LeaseToken::create();
                let _ = responder.send(Ok(client_token));
                self.stored_wake_leases.borrow_mut().push(StoredWakeLease {
                    name,
                    server_token,
                    is_unmonitored: false,
                });
            }
            Ok(fsystem::ActivityGovernorRequest::AcquireWakeLeaseWithToken {
                responder,
                name,
                server_token,
            }) => {
                let _ = responder.send(Ok(()));
                self.stored_wake_leases.borrow_mut().push(StoredWakeLease {
                    name,
                    server_token,
                    is_unmonitored: false,
                });
            }
            Ok(fsystem::ActivityGovernorRequest::AcquireUnmonitoredWakeLease {
                responder,
                name,
            }) => {
                let (server_token, client_token) = fsystem::LeaseToken::create();
                let _ = responder.send(Ok(client_token));
                self.stored_wake_leases.borrow_mut().push(StoredWakeLease {
                    name,
                    server_token,
                    is_unmonitored: true,
                });
            }
            Ok(fsystem::ActivityGovernorRequest::RegisterSuspendBlocker { responder, payload }) => {
                match (payload.suspend_blocker, payload.name) {
                    (Some(suspend_blocker), Some(name)) => {
                        let (server_token, client_token) = fsystem::LeaseToken::create();
                        let _ = responder.send(Ok(client_token));
                        self.stored_suspend_blockers.borrow_mut().push(StoredSuspendBlocker {
                            name,
                            suspend_blocker: suspend_blocker.into_proxy(),
                            server_token,
                        });
                    }
                    _ => {
                        let _ =
                            responder.send(Err(fsystem::RegisterSuspendBlockerError::InvalidArgs));
                    }
                }
            }
            Ok(fsystem::ActivityGovernorRequest::_UnknownMethod { ordinal, .. }) => {
                log::warn!(ordinal:?; "Unknown ActivityGovernorRequest method in frontend");
            }
            Err(error) => {
                log::error!(error:?; "Error handling ActivityGovernor request stream in frontend");
            }
        }
    }

    async fn handle_execution_state_manager_request(
        &self,
        request: Result<fsystem::ExecutionStateManagerRequest, fidl::Error>,
    ) {
        match request {
            Ok(fsystem::ExecutionStateManagerRequest::GetExecutionStateDependencyToken {
                responder,
            }) => {
                self.get_execution_state_dependency_token_queue.borrow_mut().push(responder);
            }
            Ok(fsystem::ExecutionStateManagerRequest::AddApplicationActivityDependency {
                payload,
                responder,
            }) => {
                self.add_application_activity_dependency_queue
                    .borrow_mut()
                    .push((payload, responder));
            }
            Ok(fsystem::ExecutionStateManagerRequest::_UnknownMethod { ordinal, .. }) => {
                log::warn!(ordinal:?; "Unknown ExecutionStateManagerRequest method in frontend");
            }
            Err(error) => {
                log::error!(error:?; "Error handling ExecutionStateManager request stream in frontend");
            }
        }
    }

    fn drain_accumulated_requests(
        &self,
    ) -> (
        Vec<StoredWakeLease>,
        Vec<StoredSuspendBlocker>,
        Vec<crate::system_activity_governor::StoredApplicationActivityLease>,
        Vec<(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest,
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyResponder,
        )>,
    ) {
        let wake_leases = self.stored_wake_leases.borrow_mut().split_off(0);
        let suspend_blockers = self.stored_suspend_blockers.borrow_mut().split_off(0);
        let application_activity_leases =
            self.stored_application_activity_leases.borrow_mut().split_off(0);
        let add_application_activity_dependencies =
            self.add_application_activity_dependency_queue.borrow_mut().split_off(0);
        (
            wake_leases,
            suspend_blockers,
            application_activity_leases,
            add_application_activity_dependencies,
        )
    }

    pub async fn create_sag(
        self: Rc<Self>,
        topology: &fbroker::TopologyProxy,
        inspect_root: INode,
        sag_event_logger: SagEventLogger,
        cpu_manager: Rc<CpuManager>,
        execution_state_dependencies: Vec<fbroker::LevelDependency>,
        is_shutting_down: Rc<Cell<bool>>,
        crash_reporter: ffeedback::CrashReporterProxy,
        boost_proxy: fcpumanager::BoostProxy,
        admin_proxy: Option<fidl_fuchsia_hardware_power_statecontrol::AdminProxy>,
        config: &Config,
    ) -> Result<()> {
        log::info!("Creating activity governor server from frontend...");
        let sag = SystemActivityGovernor::new(
            topology,
            inspect_root,
            sag_event_logger,
            cpu_manager,
            execution_state_dependencies,
            is_shutting_down,
            crash_reporter,
            boost_proxy,
            admin_proxy,
            config,
        )
        .await?;

        // Set the OnceCell which will trigger a call to `run` elsewhere.
        self.sag.set(sag.clone()).await.unwrap_or_else(|_| panic!("SAG OnceCell already set"));

        // Flush accumulated requests.
        let (
            wake_leases,
            suspend_blockers,
            application_activity_leases,
            add_application_activity_dependencies,
        ) = self.drain_accumulated_requests();

        // Process leases and suspend blockers in a spawned task since they may require relatively
        // lengthy interaction.
        let sag_clone = sag.clone();
        fasync::Task::local(async move {
            sag_clone
                .process_accumulated_requests(
                    wake_leases,
                    suspend_blockers,
                    application_activity_leases,
                    add_application_activity_dependencies,
                )
                .await;
        })
        .detach();

        // GetPowerElements and GetExecutionStateDependencyToken are fast, so just do all the work
        // here.
        let queue = self.get_power_elements_queue.borrow_mut().split_off(0);
        for responder in queue {
            sag.handle_get_power_elements(responder);
        }
        let queue = self.get_execution_state_dependency_token_queue.borrow_mut().split_off(0);
        for responder in queue {
            sag.handle_get_execution_state_dependency_token(responder);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;

    #[fuchsia::test]
    async fn test_stored_wake_leases_accumulation() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        let (proxy, stream) = create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_activity_governor_stream(stream).await;
        });

        // Send a TakeWakeLease request.
        let _token = proxy.acquire_wake_lease("test_wake_lease");

        loop {
            if frontend.stored_wake_leases.borrow().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }

        assert_eq!(frontend.stored_wake_leases.borrow()[0].name, "test_wake_lease");
    }

    #[fuchsia::test]
    async fn test_stored_suspend_blockers_accumulation() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        let (proxy, stream) = create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_activity_governor_stream(stream).await;
        });

        let (client_end, _server_end) =
            fidl::endpoints::create_endpoints::<fsystem::SuspendBlockerMarker>();
        let _ = proxy.register_suspend_blocker(
            fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(client_end),
                name: Some("test_blocker".to_string()),
                ..Default::default()
            },
        );

        loop {
            if frontend.stored_suspend_blockers.borrow().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }
        assert_eq!(frontend.stored_suspend_blockers.borrow()[0].name, "test_blocker");
    }

    #[fuchsia::test]
    async fn test_stored_application_activity_leases_accumulation() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        let (proxy, stream) = create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_activity_governor_stream(stream).await;
        });

        // Send a TakeApplicationActivityLease request.
        let _token = proxy.take_application_activity_lease("test_app_activity_lease");

        loop {
            if frontend.stored_application_activity_leases.borrow().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }

        assert_eq!(
            frontend.stored_application_activity_leases.borrow()[0].name,
            "test_app_activity_lease"
        );
    }

    #[fuchsia::test]
    async fn test_get_power_elements_accumulation() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        let (proxy, stream) = create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_activity_governor_stream(stream).await;
        });

        let _elements = proxy.get_power_elements();

        loop {
            if frontend.get_power_elements_queue.borrow().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }
    }

    #[fuchsia::test]
    async fn test_get_execution_state_dependency_token_accumulation() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        let (proxy, stream) = create_proxy_and_stream::<fsystem::ExecutionStateManagerMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_execution_state_manager_stream(stream).await;
        });

        let _token = proxy.get_execution_state_dependency_token();

        loop {
            if frontend.get_execution_state_dependency_token_queue.borrow().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }
    }

    #[fuchsia::test]
    async fn test_add_application_activity_dependency_accumulation() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        let (proxy, stream) = create_proxy_and_stream::<fsystem::ExecutionStateManagerMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_execution_state_manager_stream(stream).await;
        });

        let event = zx::Event::create();
        let _ = proxy.add_application_activity_dependency(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
                dependency_token: Some(event),
                power_level: Some(1),
                ..Default::default()
            },
        );

        loop {
            if frontend.add_application_activity_dependency_queue.borrow().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }

        assert_eq!(
            frontend.add_application_activity_dependency_queue.borrow()[0].0.power_level,
            Some(1)
        );
    }

    #[fuchsia::test]
    async fn test_drain_accumulated_requests() {
        let sag_cell = Rc::new(OnceCell::new());
        let frontend = Rc::new(ActivityGovernorRequestFrontend::new(sag_cell));

        // Accumulate ActivityGovernor requests.
        let (ag_proxy, ag_stream) = create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();
        let frontend_clone = frontend.clone();
        let _ag_task = fasync::Task::local(async move {
            frontend_clone.handle_activity_governor_stream(ag_stream).await;
        });

        let _token = ag_proxy.acquire_wake_lease("test_lease");

        let (client_end, _server_end) =
            fidl::endpoints::create_endpoints::<fsystem::SuspendBlockerMarker>();
        let _ = ag_proxy.register_suspend_blocker(
            fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(client_end),
                name: Some("test_blocker".to_string()),
                ..Default::default()
            },
        );

        let _token = ag_proxy.take_application_activity_lease("test_app_lease");

        // Accumulate ExecutionStateManager requests.
        let (esm_proxy, stream) = create_proxy_and_stream::<fsystem::ExecutionStateManagerMarker>();

        let frontend_clone = frontend.clone();
        let _task = fasync::Task::local(async move {
            frontend_clone.handle_execution_state_manager_stream(stream).await;
        });

        let event = zx::Event::create();
        let _ = esm_proxy.add_application_activity_dependency(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
                dependency_token: Some(event),
                power_level: Some(1),
                ..Default::default()
            },
        );

        loop {
            if frontend.stored_wake_leases.borrow().len() == 1
                && frontend.stored_suspend_blockers.borrow().len() == 1
                && frontend.stored_application_activity_leases.borrow().len() == 1
                && frontend.add_application_activity_dependency_queue.borrow().len() == 1
            {
                break;
            }
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await;
        }

        let (
            wake_leases,
            suspend_blockers,
            application_activity_leases,
            add_application_activity_dependencies,
        ) = frontend.drain_accumulated_requests();

        assert_eq!(wake_leases.len(), 1);
        assert_eq!(suspend_blockers.len(), 1);
        assert_eq!(application_activity_leases.len(), 1);
        assert_eq!(add_application_activity_dependencies.len(), 1);
        assert!(frontend.stored_wake_leases.borrow().is_empty());
        assert!(frontend.stored_suspend_blockers.borrow().is_empty());
        assert!(frontend.stored_application_activity_leases.borrow().is_empty());
        assert!(frontend.add_application_activity_dependency_queue.borrow().is_empty());
    }
}
