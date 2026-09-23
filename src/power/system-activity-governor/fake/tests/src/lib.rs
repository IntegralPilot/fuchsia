// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::{DiscoverableProtocolMarker, create_endpoints};
use fidl_fuchsia_power_broker::{self as fbroker, LeaseStatus};
use fidl_fuchsia_power_system::{self as fsystem, ApplicationActivityLevel, ExecutionStateLevel};
use fidl_test_sagcontrol as fctrl;
use fuchsia_async as fasync;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::StreamExt;
use futures::channel::mpsc;
use log::*;
use power_broker_client::PowerElementContext;

struct TestEnv {
    realm_instance: RealmInstance,
}
impl TestEnv {
    /// Connects to a protocol exposed by a component within the RealmInstance.
    pub fn connect_to_protocol<P: DiscoverableProtocolMarker>(&self) -> P::Proxy {
        self.realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap()
    }
}

async fn create_test_env() -> TestEnv {
    info!("building the test env");

    let builder = RealmBuilder::new().await.unwrap();
    let component_ref = builder
        .add_child(
            "fake-system-activity-governor",
            "fake-system-activity-governor#meta/fake-system-activity-governor.cm",
            ChildOptions::new(),
        )
        .await
        .expect("Failed to add child: fake-system-activity-governor");

    let power_broker_ref = builder
        .add_child("power-broker", "#meta/power-broker.cm", ChildOptions::new())
        .await
        .expect("Failed to add child: power-broker");

    let fake_shutdown_shim_ref = builder
        .add_child("fake-shutdown-shim", "#meta/fake-shutdown-shim.cm", ChildOptions::new())
        .await
        .expect("Failed to add child: fake-shutdown-shim");

    // Expose capabilities from power-broker.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.broker.Topology"))
                .from(&power_broker_ref)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    // Expose config capabilities to system-activity-governor.
    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.UseSuspender".parse().unwrap(),
            value: false.into(),
        }))
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.SuspendResumeStuckWarningTimeout".parse().unwrap(),
            value: 60u32.into(),
        }))
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.LongWakeLeaseTimeout".parse().unwrap(),
            value: 300u32.into(),
        }))
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.UseSuspender"))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.power.SuspendResumeStuckWarningTimeout",
                ))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.RebootOnStalledSuspendBlocker".parse().unwrap(),
            value: false.into(),
        }))
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.power.RebootOnStalledSuspendBlocker",
                ))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.LongWakeLeaseTimeout"))
                .from(Ref::self_())
                .to(&component_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.WaitForSuspendingToken"))
                .from(Ref::void())
                .to(&component_ref),
        )
        .await
        .unwrap();

    // Expose capabilities from power-broker to fake-system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.broker.Topology"))
                .from(&power_broker_ref)
                .to(&component_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.hardware.power.statecontrol.ShutdownWatcherRegister",
                ))
                .from(&fake_shutdown_shim_ref)
                .to(&component_ref),
        )
        .await
        .unwrap();

    // Expose capabilities from fake-system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.power.broker.ElementInfoProvider",
                ))
                .capability(Capability::protocol_by_name("test.sagcontrol.State"))
                .capability(Capability::protocol_by_name("fuchsia.power.suspend.Stats"))
                .capability(Capability::protocol_by_name("fuchsia.power.system.ActivityGovernor"))
                .from(&component_ref)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    let realm_instance = builder.build().await.expect("Failed to build RealmInstance");
    TestEnv { realm_instance }
}

#[fuchsia::test]
async fn test_fsystem_activity_governor_suspend_blocker_and_get_power_element() -> Result<()> {
    let env = create_test_env().await;

    let activity_governor = env.connect_to_protocol::<fsystem::ActivityGovernorMarker>();
    let sag_ctrl_state = env.connect_to_protocol::<fctrl::StateMarker>();
    let topology = env.connect_to_protocol::<fbroker::TopologyMarker>();

    // Check initial booting state [2, 0].
    assert_eq!(
        sag_ctrl_state.watch().await.unwrap(),
        fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Active),
            application_activity_level: Some(ApplicationActivityLevel::Inactive),
            ..Default::default()
        }
    );

    let power_elements = activity_governor.get_power_elements().await?;
    let application_activity_token =
        power_elements.application_activity.unwrap().assertive_dependency_token.unwrap();
    let (td_runner_client, td_runner) = create_endpoints::<fbroker::ElementRunnerMarker>();
    let mut td_runner_stream = td_runner.into_stream();

    let test_driver =
        PowerElementContext::builder(&topology, "test_driver", &[0, 1], td_runner_client)
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(1),
                requires_token: Some(application_activity_token),
                requires_level_by_preference: Some(vec![1]),
                ..Default::default()
            }])
            .build()
            .await?;
    let (required_level, responder) = td_runner_stream
        .next()
        .await
        .unwrap()
        .expect("ElementRunnerRequestStream next failed")
        .into_set_level()
        .unwrap();
    assert_eq!(0, required_level);
    assert!(responder.send().is_ok());

    let (tdc_runner_client, tdc_runner) = create_endpoints::<fbroker::ElementRunnerMarker>();
    let mut tdc_runner_stream = tdc_runner.into_stream();
    let test_driver_controller = PowerElementContext::builder(
        &topology,
        "test_driver_controller",
        &[0, 1],
        tdc_runner_client,
    )
    .dependencies(vec![fbroker::LevelDependency {
        dependent_level: Some(1),
        requires_token: Some(test_driver.assertive_dependency_token().unwrap()),
        requires_level_by_preference: Some(vec![1]),
        ..Default::default()
    }])
    .build()
    .await?;
    let (required_level, responder) = tdc_runner_stream
        .next()
        .await
        .unwrap()
        .expect("ElementRunnerRequestStream next failed")
        .into_set_level()
        .unwrap();
    assert_eq!(0, required_level);
    assert!(responder.send().is_ok());

    let (blocker_client_end, mut blocker_stream) = fidl::endpoints::create_request_stream();
    let registration_lease = activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(blocker_client_end),
            name: Some("test_suspend_blocker".into()),
            ..Default::default()
        })
        .await
        .expect("RegisterSuspendBlocker failed");
    drop(registration_lease);

    let (on_suspend_tx, mut on_suspend_rx) = mpsc::channel(1);
    let (on_resume_tx, mut on_resume_rx) = mpsc::channel(1);

    fasync::Task::local(async move {
        let mut on_suspend_tx = on_suspend_tx;
        let mut on_resume_tx = on_resume_tx;

        while let Some(Ok(req)) = blocker_stream.next().await {
            match req {
                fsystem::SuspendBlockerRequest::AfterResume { responder } => {
                    responder.send().unwrap();
                    on_resume_tx.try_send(()).unwrap();
                }
                fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                    responder.send().unwrap();
                    on_suspend_tx.try_send(()).unwrap();
                }
                fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Unexpected method: {}", ordinal);
                }
            }
        }
    })
    .detach();

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            application_activity_level: Some(ApplicationActivityLevel::Active),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Trigger "boot complete" logic and a suspend/resume cycle.
    let () =
        sag_ctrl_state.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    let mut current_state = fctrl::SystemActivityGovernorState {
        execution_state_level: Some(ExecutionStateLevel::Active),
        application_activity_level: Some(ApplicationActivityLevel::Active),
        ..Default::default()
    };
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Inactive),
            application_activity_level: Some(ApplicationActivityLevel::Inactive),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();
    current_state.application_activity_level.replace(ApplicationActivityLevel::Inactive);
    current_state.execution_state_level.replace(ExecutionStateLevel::Inactive);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    // OnSuspendStarted and OnResume should have been called once.
    on_suspend_rx.next().await.unwrap();
    on_resume_rx.next().await.unwrap();

    let lease_control = test_driver_controller
        .lessor
        .lease(1)
        .await?
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .into_proxy();

    assert_eq!(
        LeaseStatus::Pending,
        lease_control.watch_status(LeaseStatus::Unknown).await.unwrap()
    );

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Active),
            application_activity_level: Some(ApplicationActivityLevel::Active),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();
    current_state.application_activity_level.replace(ApplicationActivityLevel::Active);
    current_state.execution_state_level.replace(ExecutionStateLevel::Active);

    let (required_level, responder) = td_runner_stream
        .next()
        .await
        .unwrap()
        .expect("ElementRunnerRequestStream next failed")
        .into_set_level()
        .unwrap();
    assert_eq!(1, required_level);
    assert_eq!(
        LeaseStatus::Pending,
        lease_control.watch_status(LeaseStatus::Unknown).await.unwrap()
    );
    assert!(responder.send().is_ok());
    assert_eq!(
        LeaseStatus::Pending,
        lease_control.watch_status(LeaseStatus::Unknown).await.unwrap()
    );
    let (required_level, responder) = tdc_runner_stream
        .next()
        .await
        .unwrap()
        .expect("ElementRunnerRequestStream next failed")
        .into_set_level()
        .unwrap();
    assert_eq!(1, required_level);
    assert!(responder.send().is_ok());
    assert_eq!(
        LeaseStatus::Satisfied,
        lease_control.watch_status(LeaseStatus::Pending).await.unwrap()
    );

    // TODO(didis): Add test for setting ExecutionStateLevel to Inactive after
    // fxr/1014552 lands.

    Ok(())
}

#[fuchsia::test]
async fn test_set_valid_sag_states() -> Result<()> {
    let env = create_test_env().await;

    let sag_ctrl_state = env.connect_to_protocol::<fctrl::StateMarker>();

    // Check initial booting state [2, 0].
    let mut current_state = fctrl::SystemActivityGovernorState {
        execution_state_level: Some(ExecutionStateLevel::Active),
        application_activity_level: Some(ApplicationActivityLevel::Inactive),
        ..Default::default()
    };

    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            application_activity_level: Some(ApplicationActivityLevel::Active),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Wait until SAG state changes to [2, 1].
    current_state.application_activity_level.replace(ApplicationActivityLevel::Active);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    // Trigger "boot complete" logic.
    let () =
        sag_ctrl_state.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Suspending),
            application_activity_level: Some(ApplicationActivityLevel::Inactive),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Wait until SAG state changes to [1, 0].
    current_state.execution_state_level.replace(ExecutionStateLevel::Suspending);
    current_state.application_activity_level.replace(ApplicationActivityLevel::Inactive);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Active),
            application_activity_level: Some(ApplicationActivityLevel::Active),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Wait until SAG state changes to [2, 1].
    current_state.execution_state_level.replace(ExecutionStateLevel::Active);
    current_state.application_activity_level.replace(ApplicationActivityLevel::Active);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Inactive),
            application_activity_level: Some(ApplicationActivityLevel::Inactive),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Wait until SAG state changes to [0, 0].
    current_state.execution_state_level.replace(ExecutionStateLevel::Inactive);
    current_state.application_activity_level.replace(ApplicationActivityLevel::Inactive);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Active),
            application_activity_level: Some(ApplicationActivityLevel::Active),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Wait until SAG state changes to [2, 1].
    current_state.execution_state_level.replace(ExecutionStateLevel::Active);
    current_state.application_activity_level.replace(ApplicationActivityLevel::Active);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    let _ = sag_ctrl_state
        .set(&fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Suspending),
            application_activity_level: Some(ApplicationActivityLevel::Inactive),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Wait until SAG state changes to [1, 0].
    current_state.execution_state_level.replace(ExecutionStateLevel::Suspending);
    current_state.application_activity_level.replace(ApplicationActivityLevel::Inactive);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), current_state);

    Ok(())
}

#[fuchsia::test]
async fn test_set_invalid_sag_states() -> Result<()> {
    let env = create_test_env().await;

    let sag_ctrl_state = env.connect_to_protocol::<fctrl::StateMarker>();

    // Check initial booting state [2, 0].
    assert_eq!(
        sag_ctrl_state.watch().await.unwrap(),
        fctrl::SystemActivityGovernorState {
            execution_state_level: Some(ExecutionStateLevel::Active),
            application_activity_level: Some(ApplicationActivityLevel::Inactive),
            ..Default::default()
        }
    );

    let mut state = fctrl::SystemActivityGovernorState {
        execution_state_level: Some(ExecutionStateLevel::Active),
        application_activity_level: Some(ApplicationActivityLevel::Inactive),
        ..Default::default()
    };

    // Trigger "boot complete" logic.
    assert_eq!(
        sag_ctrl_state
            .set(&fctrl::SystemActivityGovernorState {
                application_activity_level: Some(ApplicationActivityLevel::Active),
                ..Default::default()
            },)
            .await
            .unwrap(),
        Ok(())
    );
    state.application_activity_level.replace(ApplicationActivityLevel::Active);
    assert_eq!(sag_ctrl_state.watch().await.unwrap(), state);

    let () =
        sag_ctrl_state.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // After triggering "boot complete" logic, when ExecutionState is Active, ApplicationActivity has to be active.
    assert_eq!(
        sag_ctrl_state
            .set(&fctrl::SystemActivityGovernorState {
                application_activity_level: Some(ApplicationActivityLevel::Inactive),
                ..Default::default()
            },)
            .await
            .unwrap(),
        Err(fctrl::SetSystemActivityGovernorStateError::NotSupported)
    );

    assert_eq!(
        sag_ctrl_state
            .set(&fctrl::SystemActivityGovernorState {
                execution_state_level: Some(ExecutionStateLevel::Active),
                application_activity_level: Some(ApplicationActivityLevel::Inactive),
                ..Default::default()
            },)
            .await
            .unwrap(),
        Err(fctrl::SetSystemActivityGovernorStateError::NotSupported)
    );

    // When ExecutionState is Inactive, everything else need to be inactive.
    assert_eq!(
        sag_ctrl_state
            .set(&fctrl::SystemActivityGovernorState {
                execution_state_level: Some(ExecutionStateLevel::Inactive),
                application_activity_level: Some(ApplicationActivityLevel::Active),
                ..Default::default()
            },)
            .await
            .unwrap(),
        Err(fctrl::SetSystemActivityGovernorStateError::NotSupported)
    );

    Ok(())
}
