// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::shutdown_mocks::{LeaseState, Signal, new_mocks_provider};
use anyhow::Error;
use assert_matches::assert_matches;
use fidl::marker::SourceBreaking;
use fidl_fuchsia_boot as fboot;
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fidl_fuchsia_hardware_power_statecontrol::{
    RebootReason2, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fidl_fuchsia_power as fpower;
use fidl_fuchsia_power_internal as fpower_internal;
use fidl_fuchsia_power_system as fsystem;
use fidl_fuchsia_sys2 as fsys;
use fidl_fuchsia_system_state as fdevicemanager;
use fuchsia_async as fasync;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::StreamExt;
use futures::channel::mpsc;
use test_case::{test_case, test_matrix};

use crate::reboot_watcher_client::{
    RebootWatcherClient, ShutdownWatcherClient, TerminalStateWatcherClient,
};

mod reboot_watcher_client;
mod shutdown_mocks;

const SHUTDOWN_SHIM_URL: &'static str =
    "fuchsia-pkg://fuchsia.com/shutdown-shim-integration-tests#meta/shutdown-shim.cm";

// Makes a new realm containing a shutdown shim and a mocks server.
//
//              root
//             /    \
// shutdown-shim    mocks-server
//
// The mocks server is seeded with an mpsc::UnboundedSender, and whenever the shutdown-shim calls a
// function on the mocks server the mocks server will emit a shutdown_mocks::Signal over the
// channel.
//
// The shutdown-shim always receives logging from above the root, along with mock component_manager
// protocols from the mocks-server (because these are always present in prod). The `variant` field
// determines whether the shim receives a functional version of the power_manager mocks.
async fn new_realm(
    is_power_framework_available: bool,
) -> Result<(RealmInstance, mpsc::UnboundedReceiver<Signal>), Error> {
    let (mocks_provider, recv_signals) = new_mocks_provider(is_power_framework_available);
    let builder = RealmBuilder::new().await?;

    let shutdown_shim =
        builder.add_child("shutdown-shim", SHUTDOWN_SHIM_URL, ChildOptions::new()).await?;
    let mocks_server =
        builder.add_local_child("mocks-server", mocks_provider, ChildOptions::new()).await?;

    // Give config capabilities.
    if is_power_framework_available {
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.power.SuspendEnabled".parse()?,
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(true)),
            }))
            .await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.power.SuspendEnabled"))
                    .from(Ref::self_())
                    .to(&shutdown_shim),
            )
            .await?;
    } else {
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.power.SuspendEnabled"))
                    .from(Ref::void())
                    .to(&shutdown_shim),
            )
            .await?;
    }

    // Give the shim logging
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fboot::WriteOnlyLogMarker>())
                .from(Ref::parent())
                .to(&shutdown_shim),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&mocks_server),
        )
        .await?;
    // Expose the shim's statecontrol.Admin so test cases can access it
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fstatecontrol::AdminMarker>())
                .from(&shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;
    // Expose the shim's Collaborative Reboot protocols so test cases can access
    // them.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<
                    fpower_internal::CollaborativeRebootSchedulerMarker,
                >())
                .from(&shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fpower::CollaborativeRebootInitiatorMarker>())
                .from(&shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;
    // Expose the shim's statecontrol.RebootMethodsWatcherRegister so test cases can access it
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::protocol::<fstatecontrol::RebootMethodsWatcherRegisterMarker>(),
                )
                .from(&shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;
    // Expose the shim's statecontrol.ShutdownWatcherRegister so test cases can access it
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fstatecontrol::ShutdownWatcherRegisterMarker>())
                .from(&shutdown_shim)
                .to(Ref::parent()),
        )
        .await?;
    // Give the shim the component_manager mock, as it is always available to the shim in prod
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys::SystemControllerMarker>())
                .from(&mocks_server)
                .to(&shutdown_shim),
        )
        .await?;

    if is_power_framework_available {
        // Give the shim access to mock activity governor.
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fsystem::ActivityGovernorMarker>())
                    .from(&mocks_server)
                    .to(&shutdown_shim),
            )
            .await?;
    }

    Ok((builder.build().await?, recv_signals))
}

async fn verify_shutdown_shim_common_behavior(
    realm_instance: RealmInstance,
    is_power_framework_available: bool,
    mut recv_signals: mpsc::UnboundedReceiver<Signal>,
) -> Result<(), Error> {
    // Conditionally checks the mock receives the AcquireWakeLease call
    if is_power_framework_available {
        assert_matches!(
            recv_signals.next().await,
            Some(Signal::ShutdownControlLease(LeaseState::Acquired))
        );
    }

    // Verify the system controller service gets the shutdown request from shutdown-shim
    assert_matches!(recv_signals.next().await, Some(Signal::Sys2Shutdown(_)));

    // Conditionally checks the mock detects the lease drop.
    if is_power_framework_available {
        assert_matches!(
            recv_signals.next().await,
            Some(Signal::ShutdownControlLease(LeaseState::Dropped))
        );
    } else {
        drop(realm_instance);
        assert_matches!(recv_signals.next().await, None);
    }
    Ok(())
}

#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn test_poweroff(is_power_framework_available: bool) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;
    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    let _task = fasync::Task::spawn(async move {
        shim_statecontrol.poweroff().await.expect_err(
            "the shutdown shim should close the channel when manual shutdown driving is complete",
        );
    });
    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn test_mexec(is_power_framework_available: bool) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;
    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    let _task = fasync::Task::spawn(async move {
        let kernel_zbi = zx::Vmo::create(0).unwrap();
        let data_zbi = zx::Vmo::create(0).unwrap();
        shim_statecontrol.mexec(kernel_zbi, data_zbi).await.expect_err(
            "the shutdown shim should close the channel when manual shutdown driving is complete",
        );
    });

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_matrix(
    [true, false],
    [RebootReason2::SystemUpdate, RebootReason2::OutOfMemory, RebootReason2::SessionFailure]
)]
#[fuchsia::test]
async fn test_reboot(
    is_power_framework_available: bool,
    reason: RebootReason2,
) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;
    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    fasync::Task::spawn(async move {
        shim_statecontrol.perform_reboot(&fstatecontrol::RebootOptions {
            reasons: Some(vec![reason]),
            ..Default::default()
        }).await.expect_err(
            "the shutdown shim should close the channel when manual shutdown driving is complete",
        );
    })
    .detach();

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_matrix(
    [true, false],
    [ShutdownAction::Reboot, ShutdownAction::RebootToBootloader, ShutdownAction::RebootToRecovery, ShutdownAction::Poweroff])]
#[fuchsia::test]
async fn test_shutdown_request_shutdown_watchers(
    is_power_framework_available: bool,
    action: ShutdownAction,
) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;

    let mut shutdown_watcher = ShutdownWatcherClient::new(&realm_instance).await;

    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    fasync::Task::spawn(async move {
        shim_statecontrol.shutdown(&ShutdownOptions {
            action: Some(action),
            reasons: Some(vec![ShutdownReason::SystemUpdate]),
            __source_breaking: SourceBreaking
        })
        .await
        .expect_err(
            "the shutdown shim should close the channel when manual shutdown driving is complete",
        );
    })
    .detach();

    // Verify shutdown-shim forwards the shutdown reason to the watcher client.
    let options = shutdown_watcher.get_shutdown_options().await;
    assert_eq!(options.action.unwrap(), action);
    assert_eq!(&options.reasons.unwrap()[..], [fstatecontrol::ShutdownReason::SystemUpdate]);

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_matrix(
    [true, false],
    [ShutdownAction::Reboot, ShutdownAction::RebootToBootloader, ShutdownAction::RebootToRecovery, ShutdownAction::Poweroff])]
#[fuchsia::test]
async fn test_shutdown_request_terminal_state_watchers(
    is_power_framework_available: bool,
    action: ShutdownAction,
) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;

    let mut terminal_watcher = TerminalStateWatcherClient::new(&realm_instance).await;

    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    fasync::Task::spawn(async move {
        shim_statecontrol
            .shutdown(&ShutdownOptions {
                action: Some(action),
                reasons: Some(vec![ShutdownReason::SystemUpdate]),
                __source_breaking: SourceBreaking,
            })
            .await
            .expect_err(
                "the shutdown shim should close the channel when manual shutdown driving is complete",
            );
    })
    .detach();

    // Verify shutdown-shim notifies the watcher client.
    terminal_watcher.wait_for_terminal_state_transition_started().await;

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn shutdown_request_action_required(is_power_framework_available: bool) {
    let (realm_instance, _recv_signals) = new_realm(is_power_framework_available).await.unwrap();

    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let result = shim_statecontrol
        .shutdown(&ShutdownOptions {
            action: None,
            reasons: Some(vec![ShutdownReason::SystemUpdate]),
            __source_breaking: SourceBreaking,
        })
        .await;

    assert_matches!(
        result.unwrap(),
        Err(status_code) if status_code == zx::Status::INVALID_ARGS.into_raw()
    );
}

#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn test_shutdown_request_reboot_watchers(
    is_power_framework_available: bool,
) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;

    // Only ShutdownAction::Reboot will forward shutdown reasons to reboot watchers.
    let mut reboot_watcher = RebootWatcherClient::new(&realm_instance).await;

    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    fasync::Task::spawn(async move {
        shim_statecontrol.shutdown(&ShutdownOptions {
            action: Some(ShutdownAction::Reboot),
            reasons: Some(vec![ShutdownReason::SystemUpdate]),
            __source_breaking: SourceBreaking
        })
        .await
        .expect_err(
            "the shutdown shim should close the channel when manual shutdown driving is complete",
        );
    })
    .detach();

    // Verify shutdown-shim forwards the reboot reason to the watcher client
    let reasons = assert_matches!(
        reboot_watcher.get_reboot_options().await,
        fstatecontrol::RebootOptions{ reasons: Some(reasons), ..} => reasons
    );
    assert_eq!(&reasons[..], [RebootReason2::SystemUpdate]);

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn test_reboot_request_terminal_state_watchers(
    is_power_framework_available: bool,
) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;

    let mut terminal_watcher = TerminalStateWatcherClient::new(&realm_instance).await;

    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    fasync::Task::spawn(async move {
        shim_statecontrol
            .perform_reboot(&fstatecontrol::RebootOptions {
                reasons: Some(vec![RebootReason2::SystemUpdate]),
                ..Default::default()
            })
            .await
            .expect_err(
                "the shutdown shim should close the channel when manual shutdown driving is complete",
            );
    })
    .detach();

    // Verify shutdown-shim notifies the watcher client.
    terminal_watcher.wait_for_terminal_state_transition_started().await;

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn test_collaborative_reboot(is_power_framework_available: bool) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;
    let shim_scheduler: fpower_internal::CollaborativeRebootSchedulerProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;
    let shim_initiator: fpower::CollaborativeRebootInitiatorProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;

    shim_scheduler
        .schedule_reboot(fpower_internal::CollaborativeRebootReason::SystemUpdate, None)
        .await
        .expect("failed to schedule reboot");
    fasync::Task::spawn(async move {
        shim_initiator.perform_pending_reboot().await.expect_err(
            "the shutdown shim should close the channel when manual shutdown driving is complete",
        );
    })
    .detach();

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

/// Verifies that `fuchsia.hardware.power.statecontrol/Admin.PerformReboot`
/// requests sent to shutdown-shim are handled as expected:
///     1) forward the reboot reason to connected RebootWatcher clients
///     2) send a shutdown request to the system controller
#[test_case(true; "with_power_framework")]
#[test_case(false; "without_power_framework")]
#[fuchsia::test]
async fn shutdown_test(is_power_framework_available: bool) -> Result<(), Error> {
    let (realm_instance, recv_signals) = new_realm(is_power_framework_available).await?;

    let mut reboot_watcher = RebootWatcherClient::new(&realm_instance).await;

    // Send a reboot request to the shutdown-shim (in a separate Task because it never returns)
    let shim_statecontrol: fstatecontrol::AdminProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir()?;
    let options = fstatecontrol::RebootOptions {
        reasons: Some(vec![RebootReason2::SystemUpdate, RebootReason2::NetstackMigration]),
        __source_breaking: SourceBreaking,
    };
    let _shutdown_task = fasync::Task::local(shim_statecontrol.perform_reboot(&options));

    // Verify shutdown-shim forwards the reboot reason to the watcher client
    let reasons = assert_matches!(
        reboot_watcher.get_reboot_options().await,
        fstatecontrol::RebootOptions{ reasons: Some(reasons), ..} => reasons
    );
    assert_eq!(&reasons[..], [RebootReason2::SystemUpdate, RebootReason2::NetstackMigration]);

    verify_shutdown_shim_common_behavior(realm_instance, is_power_framework_available, recv_signals)
        .await
}

#[fuchsia::test]
// If this test fails it is because a new variant was introduced. If a new
// variant is introduced the `send_command` function in shutdown-shim must be
// updated to support it.
async fn test_variant_coverage() -> Result<(), Error> {
    #[allow(dead_code)] // TODO(https://fxbug.dev/351850765)
    struct Mock {}

    impl std::convert::From<fdevicemanager::SystemPowerState> for Mock {
        fn from(ps: fdevicemanager::SystemPowerState) -> Mock {
            match ps {
                fdevicemanager::SystemPowerState::Reboot
                | fdevicemanager::SystemPowerState::FullyOn
                | fdevicemanager::SystemPowerState::RebootBootloader
                | fdevicemanager::SystemPowerState::RebootRecovery
                | fdevicemanager::SystemPowerState::Poweroff
                | fdevicemanager::SystemPowerState::Mexec
                | fdevicemanager::SystemPowerState::SuspendRam
                | fdevicemanager::SystemPowerState::RebootKernelInitiated => Mock {},
            }
        }
    }
    Ok(())
}
