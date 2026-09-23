// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_utils::hanging_get::server::HangingGet;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_power_broker::{self as fbroker, LeaseStatus};
use fidl_fuchsia_power_system::{self as fsystem, ApplicationActivityLevel, ExecutionStateLevel};
use fuchsia_component::client as fclient;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use futures::lock::Mutex;
use futures::prelude::*;
use log::error;
use power_broker_client::PowerElementContext;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use fidl_test_sagcontrol as fctrl;
use fuchsia_async as fasync;

// TODO(b/336692041): Set up a more complex topology to allow fake SAG to override power element
// states.

type NotifyFn =
    Box<dyn Fn(&fctrl::SystemActivityGovernorState, fctrl::StateWatchResponder) -> bool>;
type StateHangingGet =
    HangingGet<fctrl::SystemActivityGovernorState, fctrl::StateWatchResponder, NotifyFn>;

async fn lease(controller: &PowerElementContext, level: u8) -> Result<fbroker::LeaseControlProxy> {
    let lease_control =
        controller.lessor.lease(level).await?.map_err(|e| anyhow::anyhow!("{e:?}"))?.into_proxy();

    let mut lease_status = LeaseStatus::Unknown;
    while lease_status != LeaseStatus::Satisfied {
        lease_status = lease_control.watch_status(lease_status).await.unwrap();
    }

    Ok(lease_control)
}

pub struct SystemActivityGovernorControl {
    application_activity_controller: Arc<PowerElementContext>,

    hanging_get: RefCell<StateHangingGet>,

    application_activity_lease: RefCell<Option<fbroker::LeaseControlProxy>>,

    boot_complete: Rc<Mutex<bool>>,
    current_state: Rc<Mutex<fctrl::SystemActivityGovernorState>>,
    required_state: Rc<Mutex<fctrl::SystemActivityGovernorState>>,
    sag_proxy: fsystem::ActivityGovernorProxy,
    boot_control_proxy: fsystem::BootControlProxy,
    suspending_token: RefCell<Option<fsystem::LeaseToken>>,
}

impl SystemActivityGovernorControl {
    pub async fn new() -> Rc<Self> {
        let topology = connect_to_protocol::<fbroker::TopologyMarker>().unwrap();
        let sag_proxy = connect_to_protocol::<fsystem::ActivityGovernorMarker>().unwrap();
        let sag_power_elements = sag_proxy.get_power_elements().await.unwrap();

        let boot_control_proxy = connect_to_protocol::<fsystem::BootControlMarker>().unwrap();

        let aa_token =
            sag_power_elements.application_activity.unwrap().assertive_dependency_token.unwrap();
        let (element_runner_client, element_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        let application_activity_controller = Arc::new(
            PowerElementContext::builder(
                &topology,
                "application_activity_controller",
                &[0, 1],
                element_runner_client,
            )
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(1),
                requires_token: Some(aa_token),
                requires_level_by_preference: Some(vec![1]),
                ..Default::default()
            }])
            .build()
            .await
            .unwrap(),
        );
        let aac_context = application_activity_controller.clone();
        fasync::Task::local(async move {
            aac_context.run(element_runner, None /* inspect_node */, None /* update_fn */).await;
        })
        .detach();

        let boot_complete = Rc::new(Mutex::new(false));

        let element_info_provider = fclient::connect_to_service_instance::<
            fbroker::ElementInfoProviderServiceMarker,
        >(&"system_activity_governor")
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

        let mut status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
            .get_status_endpoints()
            .await
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
            .collect();

        let es_status = status_endpoints.remove("execution_state").unwrap();
        let initial_execution_state_level = ExecutionStateLevel::from_primitive(
            es_status.watch_power_level().await.unwrap().unwrap(),
        )
        .unwrap();

        let aa_status = status_endpoints.remove("application_activity").unwrap();
        let initial_application_activity_level = ApplicationActivityLevel::from_primitive(
            aa_status.watch_power_level().await.unwrap().unwrap(),
        )
        .unwrap();

        let state = fctrl::SystemActivityGovernorState {
            execution_state_level: Some(initial_execution_state_level),
            application_activity_level: Some(initial_application_activity_level),
            ..Default::default()
        };
        let current_state = Rc::new(Mutex::new(state.clone()));
        let required_state = Rc::new(Mutex::new(state.clone()));

        let hanging_get = StateHangingGet::new(
            state,
            Box::new(
                |state: &fctrl::SystemActivityGovernorState,
                 res: fctrl::StateWatchResponder|
                 -> bool {
                    if let Err(error) = res.send(state) {
                        log::warn!(error:?; "Failed to send SAG state to client");
                    }
                    true
                },
            ),
        );

        let state_publisher = Rc::new(hanging_get.new_publisher());

        let publisher = state_publisher.clone();
        let current_state_clone = current_state.clone();
        let required_state_clone = required_state.clone();
        fasync::Task::local(async move {
            loop {
                let new_status = ExecutionStateLevel::from_primitive(
                    es_status.watch_power_level().await.unwrap().unwrap(),
                )
                .unwrap();
                current_state_clone.lock().await.execution_state_level.replace(new_status);
                let state = current_state_clone.lock().await.clone();
                if state == *required_state_clone.lock().await {
                    publisher.set(state);
                }
            }
        })
        .detach();

        let publisher = state_publisher.clone();
        let current_state_clone = current_state.clone();
        let required_state_clone = required_state.clone();
        fasync::Task::local(async move {
            loop {
                let new_status = ApplicationActivityLevel::from_primitive(
                    aa_status.watch_power_level().await.unwrap().unwrap(),
                )
                .unwrap();
                current_state_clone.lock().await.application_activity_level.replace(new_status);
                let state = current_state_clone.lock().await.clone();
                if state == *required_state_clone.lock().await {
                    publisher.set(state);
                }
            }
        })
        .detach();

        Rc::new(Self {
            application_activity_controller: application_activity_controller.into(),
            hanging_get: RefCell::new(hanging_get),
            application_activity_lease: RefCell::new(None),
            boot_complete,
            current_state,
            required_state,
            sag_proxy,
            boot_control_proxy,
            suspending_token: RefCell::new(None),
        })
    }

    pub async fn run(self: Rc<Self>, fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>) {
        let this = self;
        fs.dir("svc").add_fidl_service(move |mut stream: fctrl::StateRequestStream| {
            let this = this.clone();
            fasync::Task::local(async move {
                let sub = this.hanging_get.borrow_mut().new_subscriber();
                while let Ok(Some(request)) = stream.try_next().await {
                    match request {
                        fctrl::StateRequest::Set { responder, payload } => {
                            let result = this.update_sag_state(payload).await;
                            let _ = responder.send(result);
                        }
                        fctrl::StateRequest::Get { responder } => {
                            let _ = responder.send(&*this.current_state.lock().await);
                        }
                        fctrl::StateRequest::SetBootComplete { responder } => {
                            let () = this
                                .boot_control_proxy
                                .set_boot_complete()
                                .await
                                .expect("SetBootComplete should have succeeded");
                            *this.boot_complete.lock().await = true;
                            responder.send().unwrap();
                        }
                        fctrl::StateRequest::Watch { responder } => {
                            if let Err(error) = sub.register(responder) {
                                log::warn!(error:?; "Failed to register for Watch call");
                            }
                        }
                        fctrl::StateRequest::_UnknownMethod { ordinal, .. } => {
                            log::warn!(ordinal:?; "Unknown StateRequest method");
                        }
                    }
                }
            })
            .detach();
        });
    }

    async fn handle_application_activity_changes(
        self: &Rc<Self>,
        required_application_activity_level: ApplicationActivityLevel,
    ) -> Result<()> {
        match required_application_activity_level {
            ApplicationActivityLevel::Active => {
                let _ = self
                    .application_activity_lease
                    .borrow_mut()
                    .get_or_insert(lease(&self.application_activity_controller, 1).await?);
            }
            ApplicationActivityLevel::Inactive => {
                drop(self.application_activity_lease.borrow_mut().take());
            }
            _ => (),
        }
        Ok(())
    }

    async fn update_sag_state(
        self: &Rc<Self>,
        sag_state: fctrl::SystemActivityGovernorState,
    ) -> fctrl::StateSetResult {
        let required_execution_state_level = if let Some(r) = sag_state.execution_state_level {
            r
        } else {
            self.current_state.lock().await.execution_state_level.unwrap()
        };
        let required_application_activity_level =
            if let Some(r) = sag_state.application_activity_level {
                r
            } else {
                self.current_state.lock().await.application_activity_level.unwrap()
            };
        self.required_state
            .lock()
            .await
            .execution_state_level
            .replace(required_execution_state_level);
        self.required_state
            .lock()
            .await
            .application_activity_level
            .replace(required_application_activity_level);

        match required_execution_state_level {
            ExecutionStateLevel::Inactive => {
                if *self.boot_complete.lock().await == false
                    || required_application_activity_level != ApplicationActivityLevel::Inactive
                {
                    return Err(fctrl::SetSystemActivityGovernorStateError::NotSupported);
                }
                drop(self.suspending_token.borrow_mut().take());
                self.handle_application_activity_changes(required_application_activity_level)
                    .await
                    .map_err(|err| {
                        error!(err:%; "Request failed with internal error");
                        fctrl::SetSystemActivityGovernorStateError::Internal
                    })?;
            }
            ExecutionStateLevel::Suspending => {
                if *self.boot_complete.lock().await == false
                    || required_application_activity_level != ApplicationActivityLevel::Inactive
                {
                    return Err(fctrl::SetSystemActivityGovernorStateError::NotSupported);
                }

                let token = self.sag_proxy.acquire_wake_lease("Suspending").await.unwrap().unwrap();
                let _ = self.suspending_token.borrow_mut().get_or_insert(token);

                self.handle_application_activity_changes(required_application_activity_level)
                    .await
                    .map_err(|err| {
                        error!(err:%; "Request failed with internal error");
                        fctrl::SetSystemActivityGovernorStateError::Internal
                    })?;
            }
            ExecutionStateLevel::Active => {
                if *self.boot_complete.lock().await == true
                    && required_application_activity_level != ApplicationActivityLevel::Active
                {
                    return Err(fctrl::SetSystemActivityGovernorStateError::NotSupported);
                }

                // Take any application activity lease before dropping any wake handling leases.
                self.handle_application_activity_changes(required_application_activity_level)
                    .await
                    .map_err(|err| {
                        error!(err:%; "Request failed with internal error");
                        fctrl::SetSystemActivityGovernorStateError::Internal
                    })?;
            }
            _ => (),
        }
        Ok(())
    }
}
