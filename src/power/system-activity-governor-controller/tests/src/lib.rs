// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use diagnostics_assertions::{AnyProperty, NonZeroUintProperty, tree_assertion};
use diagnostics_reader::ArchiveReader;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_power_observability as fobs;
use fidl_fuchsia_power_system as fsystem;
use fidl_fuchsia_power_topology_test as fpt;
use fuchsia_async as fasync;
use fuchsia_component_test::{
    Capability, ChildOptions, DEFAULT_COLLECTION_NAME, RealmBuilder, RealmInstance, Ref, Route,
};
use log::*;

// Report prolonged match delay after this many loops.
const DELAY_NOTIFICATION: usize = 10;

// Spend no more than this many loop turns before giving up for the inspect to match.
const MAX_LOOPS_COUNT: usize = 20;

const RESTART_DELAY: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(1);

macro_rules! block_until_inspect_matches {
    ($moniker:expr, $($tree:tt)+) => {{
        let mut reader = ArchiveReader::inspect();

        reader
            .select_all_for_component($moniker.to_string())
            .with_minimum_schema_count(1);

        for i in 1.. {
            let Ok(data) = reader
                .snapshot()
                .await?
                .into_iter()
                .next()
                .and_then(|result| result.payload)
                .ok_or(anyhow::anyhow!("expected one inspect hierarchy")) else {
                continue;
            };

            let tree_assertion = $crate::tree_assertion!($($tree)+);
            match tree_assertion.run(&data) {
                Ok(_) => break,
                Err(error) => {
                    if i == DELAY_NOTIFICATION {
                        log::warn!(error:?; "Still awaiting inspect match after {} tries", DELAY_NOTIFICATION);
                    }
                    if  i >= MAX_LOOPS_COUNT {  // upper bound, so test terminates on mismatch
                        // Print the actual, so we know why the match failed if it does.
                        let mut sorted = data.clone();
                        sorted.sort();
                        return Err(anyhow::anyhow!("err: {}: last observed {}", error, serde_json::to_string_pretty(&sorted).unwrap()));
                    }
                }
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(RESTART_DELAY)).await;
        }
    }};
}

struct TestEnv {
    realm_instance: RealmInstance,
    sag_moniker: String,
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
            "system-activity-governor-controller",
            "#meta/system-activity-governor-controller.cm",
            ChildOptions::new(),
        )
        .await
        .expect("Failed to add child: system-activity-governor-controller");

    let power_broker_ref = builder
        .add_child("power-broker", "#meta/power-broker.cm", ChildOptions::new())
        .await
        .expect("Failed to add child: power-broker");

    let system_activity_governor_ref = builder
        .add_child(
            "system-activity-governor",
            "#meta/system-activity-governor.cm",
            ChildOptions::new(),
        )
        .await
        .expect("Failed to add child: system-activity-governor");

    let fake_shutdown_shim_ref = builder
        .add_child("fake-shutdown-shim", "#meta/fake-shutdown-shim.cm", ChildOptions::new())
        .await
        .expect("Failed to add child: fake-shutdown-shim");

    let config_no_suspender_ref = builder
        .add_child(
            "config-no-suspender",
            "config-no-suspender#meta/config-no-suspender.cm",
            ChildOptions::new(),
        )
        .await
        .expect("Failed to add child: config-no-suspender");

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

    // Expose capabilities from power-broker to system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.broker.Topology"))
                .from(&power_broker_ref)
                .to(&system_activity_governor_ref),
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
                .to(&system_activity_governor_ref),
        )
        .await
        .unwrap();

    // Offer capabilities from config-no-suspender to system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.UseSuspender"))
                .from(&config_no_suspender_ref)
                .to(&system_activity_governor_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.power.SuspendResumeStuckWarningTimeout",
                ))
                .from(&config_no_suspender_ref)
                .to(&system_activity_governor_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.power.RebootOnStalledSuspendBlocker",
                ))
                .from(&config_no_suspender_ref)
                .to(&system_activity_governor_ref),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.LongWakeLeaseTimeout"))
                .from(&config_no_suspender_ref)
                .to(&system_activity_governor_ref),
        )
        .await
        .unwrap();

    // Offer capabilities from void to system-activity-governor.
    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.WaitForSuspendingToken"))
                .from(Ref::void())
                .to(&system_activity_governor_ref),
        )
        .await
        .unwrap();

    // Expose capabilities from system-activity-governor to system-activity-governor-controller.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.system.ActivityGovernor"))
                .from(&system_activity_governor_ref)
                .to(&component_ref),
        )
        .await
        .unwrap();

    // Expose capabilities from system-activity-governor-controller.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.power.topology.test.SystemActivityControl",
                ))
                .from(&component_ref)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.power.system.BootControl"))
                .from(&system_activity_governor_ref)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    let realm_instance = builder.build().await.expect("Failed to build RealmInstance");

    let sag_moniker = format!(
        "{}:{}/{}",
        DEFAULT_COLLECTION_NAME,
        realm_instance.root.child_name(),
        "system-activity-governor"
    );

    TestEnv { realm_instance, sag_moniker }
}

#[fuchsia::test]
async fn test_system_activity_control() -> Result<()> {
    let env = create_test_env().await;

    let system_activity_control = env.connect_to_protocol::<fpt::SystemActivityControlMarker>();
    let _ = system_activity_control.start_application_activity().await.unwrap();

    let boot_control = env.connect_to_protocol::<fsystem::BootControlMarker>();
    let () = boot_control.set_boot_complete().await?;

    block_until_inspect_matches!(
        &env.sag_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                application_activity: {
                    power_level: 1u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
            },
            suspend_events: contains {
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
        }
    );

    let _ = system_activity_control.stop_application_activity().await.unwrap();

    // After a suspend attempt, SAG will auto-suspend since no lease is acquired.
    // The number of suspend attempts is unbounded, so it can be any non zero value.
    block_until_inspect_matches!(
        &env.sag_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                application_activity: {
                    power_level: 0u64,
                },
            },
            suspend_stats: {
               ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
               ref fobs::SUSPEND_FAIL_COUNT: NonZeroUintProperty,
               ref fobs::SUSPEND_LAST_FAILED_ERROR: zx::sys::ZX_ERR_NOT_SUPPORTED as i64,
               ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
               ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
               ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            suspend_events: contains {
                "5": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                },
                "6": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                },
                "7": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "8": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "9": {
                    ref fobs::SUSPEND_FAILED_AT: AnyProperty,
                },
                "10": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
        }
    );

    Ok(())
}
