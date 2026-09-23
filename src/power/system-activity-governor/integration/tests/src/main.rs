// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use diagnostics_assertions::{
    AnyProperty, AnyStringProperty, NonZeroUintProperty, TreeAssertion, tree_assertion,
};
use diagnostics_hierarchy::{DiagnosticsHierarchy, Property};
use diagnostics_reader::ArchiveReader;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fidl_fuchsia_hardware_power_suspend as fhsuspend;
use fidl_fuchsia_power_broker::{self as fbroker, LeaseStatus};
use fidl_fuchsia_power_observability as fobs;
use fidl_fuchsia_power_suspend as fsuspend;
use fidl_fuchsia_power_system as fsystem;
use fidl_fuchsia_testing as ftesting;
use fidl_test_suspendcontrol as tsc;
use fidl_test_systemactivitygovernor as ftest;
use fidl_test_systemactivitygovernor::RealmOptions;
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt};
use fuchsia_component::client::connect_to_protocol;
use futures::channel::mpsc;
use futures::{FutureExt, StreamExt};
use power_broker_client::PowerElementContext;
use realm_proxy_client::RealmProxyClient;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use test_case::test_case;
use test_util::assert_leq;

struct AtLeastIntProperty(i64);

impl<K: AsRef<str>> diagnostics_assertions::PropertyAssertion<K> for AtLeastIntProperty {
    fn run(&self, actual: &Property<K>) -> Result<(), anyhow::Error> {
        match actual {
            Property::Int(_, v) if *v >= self.0 => Ok(()),
            Property::Int(_, v) => {
                anyhow::bail!("expected integer >= {}, found {}", self.0, v)
            }
            Property::Uint(_, v) if self.0 < 0 || *v >= self.0 as u64 => Ok(()),
            Property::Uint(_, v) => {
                anyhow::bail!("expected integer >= {}, found {}", self.0, v)
            }
            _ => anyhow::bail!("expected integer property, found {}", actual.discriminant_name()),
        }
    }
}

const REALM_FACTORY_CHILD_NAME: &str = "test_realm_factory";

async fn set_up_default_suspender(device: &tsc::DeviceProxy) {
    device
        .set_suspend_states(&tsc::DeviceSetSuspendStatesRequest {
            suspend_states: Some(vec![fhsuspend::SuspendState {
                resume_latency: Some(0),
                ..Default::default()
            }]),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap()
}

async fn create_realm() -> Result<(RealmProxyClient, String)> {
    create_realm_ext(RealmOptions { use_suspender: Some(true), ..Default::default() }).await
}

async fn create_realm_ext(options: ftest::RealmOptions) -> Result<(RealmProxyClient, String)> {
    let realm_factory = connect_to_protocol::<ftest::RealmFactoryMarker>()?;
    let (client, server) = fidl::endpoints::create_endpoints();
    let result = realm_factory
        .create_realm_ext(options, server)
        .await?
        .map_err(realm_proxy_client::Error::OperationError)?;
    Ok((RealmProxyClient::from(client), result))
}

#[fuchsia::test]
async fn test_stats_returns_default_values() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);
    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_returns_expected_power_elements() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let power_elements = activity_governor.get_power_elements().await?;

    let aa_element = power_elements.application_activity.unwrap();
    let aa_assertive_token = aa_element.assertive_dependency_token.unwrap();
    assert!(!aa_assertive_token.is_invalid());

    Ok(())
}

async fn create_suspend_topology(realm: &RealmProxyClient) -> Result<Arc<PowerElementContext>> {
    let topology = realm.connect_to_protocol::<fbroker::TopologyMarker>().await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let power_elements = activity_governor.get_power_elements().await?;
    let aa_token = power_elements.application_activity.unwrap().assertive_dependency_token.unwrap();

    let (element_runner_client, element_runner) =
        create_endpoints::<fbroker::ElementRunnerMarker>();
    let suspend_controller = Arc::new(
        PowerElementContext::builder(
            &topology,
            "suspend_controller",
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
        .await?,
    );
    let sc_context = suspend_controller.clone();
    fasync::Task::local(async move {
        sc_context.run(element_runner, None /* inspect_node */, None /* update_fn */).await;
    })
    .detach();

    Ok(suspend_controller)
}

async fn lease(controller: &PowerElementContext, level: u8) -> Result<fbroker::LeaseControlProxy> {
    let lease_control =
        controller.lessor.lease(level).await?.map_err(|e| anyhow::anyhow!("{e:?}"))?.into_proxy();

    let mut lease_status = LeaseStatus::Unknown;
    while lease_status != LeaseStatus::Satisfied {
        lease_status = lease_control.watch_status(lease_status).await.unwrap();
    }

    Ok(lease_control)
}

// Report prolonged match delay after this many loops.
const DELAY_NOTIFICATION: usize = 10;

// Spend no more than this many loop turns before giving up for the inspect to match.
const MAX_LOOPS_COUNT: usize = 20;

const RESTART_DELAY: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(1);
const CRASH_REPORT_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(30);

macro_rules! block_until_inspect_matches {
    ($loop_iter:expr, $sag_moniker:expr, $($tree:tt)+) => {{
        let mut reader = ArchiveReader::inspect();

        let moniker = if $sag_moniker.is_empty() {
            REALM_FACTORY_CHILD_NAME.to_string()
        } else {
            format!("{}/{}", REALM_FACTORY_CHILD_NAME, $sag_moniker)
        };
        reader
            .select_all_for_component(moniker)
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
                    if  i >= $loop_iter {  // upper bound, so test terminates on mismatch
                        // Print the actual, so we know why the match failed if it does.
                        let mut sorted = data.clone();
                        sorted.sort();
                        return Err(anyhow::anyhow!("err: {}: last observed:\n{}", error, serde_json::to_string_pretty(&sorted).unwrap()));
                    }
                }
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(RESTART_DELAY)).await;
        }
    }};
    ($sag_moniker:expr, $($tree:tt)+) => {{
        block_until_inspect_matches!(MAX_LOOPS_COUNT, $sag_moniker, $($tree)+);
    }};
}

#[fuchsia::test]
async fn test_activity_governor_with_no_suspender_returns_not_supported_after_suspend_attempt()
-> Result<()> {
    let (realm, activity_governor_moniker) =
        create_realm_ext(ftest::RealmOptions { use_suspender: Some(false), ..Default::default() })
            .await?;
    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;

    // First watch should return immediately with default values.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            config: contains {
                use_suspender: false,
            }
        }
    );

    // Indicate that the boot has complete
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(1), current_stats.fail_count);
    assert_eq!(Some(zx::Status::NOT_SUPPORTED.into_raw()), current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_increments_suspend_success_on_application_activity_lease_drop()
-> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;
    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;

    // First watch should return immediately with default values.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);

    let suspend_controller = create_suspend_topology(&realm).await?;
    let suspend_lease_control = lease(&suspend_controller, 1).await?;
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    power_level: 2u64,
                },
                application_activity: {
                    power_level: 1u64,
                },
                cpu: {
                    power_level: 1u64,
                },
            },
            suspend_stats: {
               ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
               ref fobs::SUSPEND_FAIL_COUNT: 0u64,
               ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
               ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
               ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
               ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                 "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    drop(suspend_lease_control);
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    // Retrieve realm ID from moniker.
    let realm_id = activity_governor_moniker.split('/').next().unwrap().split(':').nth(1).unwrap();

    // Check that boost is active before resume.
    block_until_inspect_matches!(
        "",
        root: contains {
            ref realm_id: contains {
                "fake-boost": contains {
                    active: true,
                }
            }
        }
    );

    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Take a lease to prevent the system from suspending again before we can check Inspect.
    let keep_awake_lease = lease(&suspend_controller, 1).await?;

    let current_stats = stats.watch().await?;
    assert_eq!(Some(1), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(Some(2), current_stats.last_time_in_suspend);
    assert_eq!(Some(2), current_stats.total_time_in_suspend);

    // Check that boost becomes inactive after resume.
    block_until_inspect_matches!(
        "",
        root: contains {
            ref realm_id: contains {
                "fake-boost": contains {
                    active: false,
                }
            }
        }
    );

    // Drop the lease to let the system suspend again as intended by the test.
    drop(keep_awake_lease);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    // Due to timeout of the resume lease (and dropping keep_awake_lease), expect this to be 0.
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 1u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: 2u64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 2u64,
                ref fobs::SUSPEND_LAST_DURATION: 1u64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "3": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "4": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "5": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "6": {
                    ref fobs::SUSPEND_RESUMED_AT: AnyProperty,
                    ref fobs::SUSPEND_LAST_TIMESTAMP: AnyProperty,
                    ref fobs::SUSPEND_CUMULATIVE_DURATION: AnyProperty,
                },
                "7": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "8": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "9": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "10": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "11": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "12": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "13": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    // Suspend again and check if the success counter increments.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(3i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    let current_stats = stats.watch().await?;
    assert_eq!(Some(2), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(Some(3), current_stats.last_time_in_suspend);
    assert_eq!(Some(5), current_stats.total_time_in_suspend);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 2u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: 3u64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 5u64,
                ref fobs::SUSPEND_LAST_DURATION: 1u64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "14": {
                    ref fobs::SUSPEND_RESUMED_AT: AnyProperty,
                    ref fobs::SUSPEND_LAST_TIMESTAMP: 3u64,
                    ref fobs::SUSPEND_CUMULATIVE_DURATION: 5u64,
                },
                "15": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "16": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "17": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "18": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "19": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "20": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "21": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_increments_fail_count_on_suspend_error() -> Result<()> {
    let suspend_states = vec![
        fhsuspend::SuspendState { resume_latency: Some(430), ..Default::default() },
        fhsuspend::SuspendState { resume_latency: Some(320), ..Default::default() },
        fhsuspend::SuspendState { resume_latency: Some(21), ..Default::default() },
    ];
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    suspend_device
        .set_suspend_states(&tsc::DeviceSetSuspendStatesRequest {
            suspend_states: Some(suspend_states),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let suspend_controller = create_suspend_topology(&realm).await?;
    let suspend_lease_control = lease(&suspend_controller, 1).await?;
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    power_level: 2u64,
                },
                application_activity: {
                    power_level: 1u64,
                },
                cpu: {
                    power_level: 1u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                 "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    drop(suspend_lease_control);
    assert_eq!(0u64, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device.resume(&tsc::DeviceResumeRequest::Error(7)).await.unwrap().unwrap();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 1u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 7u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "3": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "4": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "5": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "6": {
                    ref fobs::SUSPEND_FAILED_AT: AnyProperty,
                },
                "7": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "8": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "9": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "10": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "11": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "12": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "13": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_suspends_successfully_after_failure() -> Result<()> {
    let suspend_states = vec![
        fhsuspend::SuspendState { resume_latency: Some(430), ..Default::default() },
        fhsuspend::SuspendState { resume_latency: Some(320), ..Default::default() },
        fhsuspend::SuspendState { resume_latency: Some(21), ..Default::default() },
    ];
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    suspend_device
        .set_suspend_states(&tsc::DeviceSetSuspendStatesRequest {
            suspend_states: Some(suspend_states),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let suspend_controller = create_suspend_topology(&realm).await?;
    let suspend_lease_control = lease(&suspend_controller, 1).await?;
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    power_level: 2u64,
                },
                application_activity: {
                    power_level: 1u64,
                },
                cpu: {
                    power_level: 1u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    drop(suspend_lease_control);
    assert_eq!(0u64, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device.resume(&tsc::DeviceResumeRequest::Error(7)).await.unwrap().unwrap();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 1u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 7u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "3": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "4": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                 },
                "5": {
                   ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "6": {
                    ref fobs::SUSPEND_FAILED_AT: AnyProperty,
                },
                "7": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "8": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "9": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "10": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "11": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "12": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "13": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    assert_eq!(0u64, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 1u64,
                ref fobs::SUSPEND_FAIL_COUNT: 1u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 7u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: 2u64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 2u64,
                ref fobs::SUSPEND_LAST_DURATION: 1u64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "14": {
                    ref fobs::SUSPEND_RESUMED_AT: AnyProperty,
                    ref fobs::SUSPEND_LAST_TIMESTAMP: 2u64,
                    ref fobs::SUSPEND_CUMULATIVE_DURATION: 2u64,
                },
                "15": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "16": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "17": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "18": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "19": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "20": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "21": {
                   ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_suspends_after_suspend_blocker_hanging_on_resume() -> Result<()> {
    let (realm, activity_governor_moniker) =
        create_realm_ext(ftest::RealmOptions { use_fake_clock: Some(true), ..Default::default() })
            .await?;
    let fake_clock_control =
        realm.connect_to_protocol::<ftesting::FakeClockControlMarker>().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    // First watch should return immediately with default values.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);

    let blocker_name = "test_suspend_blocker".to_string();
    let (blocker_client_end, mut blocker_stream) = fidl::endpoints::create_request_stream();
    let registration_lease = activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(blocker_client_end),
            name: Some(blocker_name.clone()),
            ..Default::default()
        })
        .await
        .expect("RegisterSuspendBlocker failed");

    let (before_suspend_tx, mut before_suspend_rx) = mpsc::channel(1);
    let (after_resume_tx, mut after_resume_rx) = mpsc::channel(1);

    fasync::Task::local(async move {
        let mut before_suspend_tx = before_suspend_tx;
        let mut after_resume_tx = after_resume_tx;

        while let Some(Ok(req)) = blocker_stream.next().await {
            match req {
                fsystem::SuspendBlockerRequest::AfterResume { responder, .. } => {
                    // AfterResume hangs until the responder is dropped later.
                    after_resume_tx.try_send(responder).unwrap();
                }
                fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                    responder.send().unwrap();
                    before_suspend_tx.try_send(()).unwrap();
                }
                fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Unexpected method: {}", ordinal);
                }
            }
        }
    })
    .detach();

    // Stabilize async state: Wait for registration wake lease to be satisfied BEFORE
    // triggering BootControl drop. This ensures ExecutionState goes
    // Active -> Suspending -> Inactive predictably, rather than bouncing unpredictably due to IPC
    // race conditions.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "4": {
                    ref fobs::WAKE_LEASE_SATISFIED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "test_suspend_blocker",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "fuchsia.inspect.Stats": contains {},
            }
        }
    );

    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }
    drop(registration_lease);

    // Await SAG's power elements to drop their power levels.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "2": {
                    ref fobs::SUSPEND_BLOCKER_ACQUIRED_AT: AnyProperty,
               },
                "3": {
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "test_suspend_blocker",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "4": {
                    ref fobs::WAKE_LEASE_SATISFIED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "test_suspend_blocker",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "5": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "test_suspend_blocker",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "6": {
                    ref fobs::SUSPEND_BLOCKER_DROPPED_AT: AnyProperty,
                },
                "7": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "8": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "9": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "10": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "power_observability_state_recorders": contains {},
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
            "suspend_blockers": {
                "names": vec![blocker_name.clone()],
            },
        }
    );

    // BeforeSuspend should have been called once.
    before_suspend_rx.next().await.unwrap();

    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Should only have been 1 suspend after all suspend blocker handling.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(1), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(Some(2), current_stats.last_time_in_suspend);
    assert_eq!(Some(2), current_stats.total_time_in_suspend);

    // AfterResume should have been called once.
    let after_resume_responder = after_resume_rx.next().await.unwrap();

    // Pause fake clock and advance past SAG's internal RESUME_SUSPENDING_LEASE_DROP_DELAY
    // (100ms) to allow the post-resume control lease to expire deterministically.
    fake_clock_control.pause().await?;
    fake_clock_control
        .advance(&ftesting::Increment::Determined(
            zx::MonotonicDuration::from_millis(500).into_nanos(),
        ))
        .await?
        .map_err(|e| anyhow::anyhow!("failed to advance fake clock: {:?}", e))?;
    fake_clock_control
        .resume_with_increments(
            zx::MonotonicDuration::from_millis(10).into_nanos(),
            &ftesting::Increment::Determined(zx::MonotonicDuration::from_millis(10).into_nanos()),
        )
        .await?
        .map_err(|e| anyhow::anyhow!("failed to resume fake clock: {:?}", e))?;

    // Now that the resume control lease is gone, the stuck AfterResume call is the only thing
    // blocking suspension. Once we drop `after_resume_responder`, the call completes, and SAG is
    // free to suspend.
    assert!(suspend_device.await_suspend().now_or_never().is_none());
    drop(after_resume_responder);
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_handles_boot_signal() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;
    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;

    // First watch should return immediately with default values.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);

    // Initial state should show execution_state is active and booting is true.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: true,
            is_shutting_down: false,
            power_elements: {
                execution_state: {
                    power_level: 2u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 1u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                 "fuchsia.inspect.Stats": contains {},
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    // Trigger "boot complete" signal.
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Now execution_state should have dropped and booting is false.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "3": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "4": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "5": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                }
            },
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_element_info_provider() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;

    let suspend_states = vec![
        fhsuspend::SuspendState { resume_latency: Some(1000), ..Default::default() },
        fhsuspend::SuspendState { resume_latency: Some(100), ..Default::default() },
        fhsuspend::SuspendState { resume_latency: Some(10), ..Default::default() },
    ];

    suspend_device
        .set_suspend_states(&tsc::DeviceSetSuspendStatesRequest {
            suspend_states: Some(suspend_states),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let suspend_controller = create_suspend_topology(&realm).await?;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    assert_eq!(
        [
            fbroker::ElementPowerLevelNames {
                identifier: Some("cpu".into()),
                levels: Some(vec![
                    fbroker::PowerLevelName {
                        level: Some(0),
                        name: Some("Inactive".into()),
                        ..Default::default()
                    },
                    fbroker::PowerLevelName {
                        level: Some(1),
                        name: Some("Active".into()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            fbroker::ElementPowerLevelNames {
                identifier: Some("execution_state".into()),
                levels: Some(vec![
                    fbroker::PowerLevelName {
                        level: Some(0),
                        name: Some("Inactive".into()),
                        ..Default::default()
                    },
                    fbroker::PowerLevelName {
                        level: Some(1),
                        name: Some("Suspending".into()),
                        ..Default::default()
                    },
                    fbroker::PowerLevelName {
                        level: Some(2),
                        name: Some("Active".into()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            fbroker::ElementPowerLevelNames {
                identifier: Some("application_activity".into()),
                levels: Some(vec![
                    fbroker::PowerLevelName {
                        level: Some(0),
                        name: Some("Inactive".into()),
                        ..Default::default()
                    },
                    fbroker::PowerLevelName {
                        level: Some(1),
                        name: Some("Active".into()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            fbroker::ElementPowerLevelNames {
                identifier: Some("boot_control".into()),
                levels: Some(vec![
                    fbroker::PowerLevelName {
                        level: Some(0),
                        name: Some("Inactive".into()),
                        ..Default::default()
                    },
                    fbroker::PowerLevelName {
                        level: Some(1),
                        name: Some("Active".into()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        ],
        TryInto::<[fbroker::ElementPowerLevelNames; 4]>::try_into(
            element_info_provider.get_element_power_level_names().await?.unwrap()
        )
        .unwrap()
    );

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let es_status = status_endpoints.get("execution_state").unwrap();
    let aa_status = status_endpoints.get("application_activity").unwrap();
    let bc_status = status_endpoints.get("boot_control").unwrap();

    // First watch should return immediately with default values.
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);
    assert_eq!(aa_status.watch_power_level().await?.unwrap(), 0);
    assert_eq!(bc_status.watch_power_level().await?.unwrap(), 1);

    // Trigger "boot complete" logic.
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let suspend_lease_control = lease(&suspend_controller, 1).await?;

    assert_eq!(aa_status.watch_power_level().await?.unwrap(), 1);
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    assert_eq!(bc_status.watch_power_level().await?.unwrap(), 0);
    drop(suspend_lease_control);

    // Level may drop to 1 or 0 depending on power element response order, so check the level
    // again if it is not 0.
    if es_status.watch_power_level().await?.unwrap() != 0 {
        assert_eq!(es_status.watch_power_level().await?.unwrap(), 0);
    }

    assert_eq!(aa_status.watch_power_level().await?.unwrap(), 0);

    // Check suspend is triggered and resume.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Raise Execution State to Active then drop to trigger a suspend.
    let suspend_lease_control = lease(&suspend_controller, 1).await?;
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);
    drop(suspend_lease_control);

    // Level may drop to 1 or 0 depending on power element response order, so check the level
    // again if it is not 0.
    if es_status.watch_power_level().await?.unwrap() != 0 {
        assert_eq!(es_status.watch_power_level().await?.unwrap(), 0);
    }

    // Check suspend is triggered and resume.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Raise Execution State to Active then drop to trigger a suspend.
    let suspend_lease_control = lease(&suspend_controller, 1).await?;
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);
    drop(suspend_lease_control);

    // Level may drop to 1 or 0 depending on power element response order, so check the level
    // again if it is not 0.
    if es_status.watch_power_level().await?.unwrap() != 0 {
        assert_eq!(es_status.watch_power_level().await?.unwrap(), 0);
    }

    // Check suspend is triggered and resume.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    Ok(())
}

// It is not possible to deterministically catch a bad initial state with current APIs.
// Instead, ensure that the simplest connect and assert always passes.
// If the initial state is not correct at least some of the time, this test will flake.
#[fuchsia::test]
async fn test_execution_state_always_starts_at_active_power_level() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let es_status = status_endpoints.get("execution_state").unwrap();
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);
    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_acquire_wake_lease_raises_execution_state_to_wake_handling()
-> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let es_status = status_endpoints.get("execution_state").unwrap();
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let wake_lease_name = "wake_lease";
    let wake_lease = activity_governor.acquire_wake_lease(wake_lease_name).await.unwrap().unwrap();

    // Trigger "boot complete" signal.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    assert_eq!(es_status.watch_power_level().await?.unwrap(), 1);

    let server_token_koid = &wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease.koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: wake_lease_name,
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    }
                }
            },
        }
    );

    drop(wake_lease);
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 0);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    // Confirm that the device is called after the wake lease is dropped. In particular, this
    // guarantees that SAG's internal suspend-blocking logic does not prevent suspension.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_acquire_wake_lease_raises_execution_state_to_suspending()
-> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let es_status = status_endpoints.get("execution_state").unwrap();
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let wake_lease_name = "wake_lease";
    let wake_lease = activity_governor.acquire_wake_lease(wake_lease_name).await.unwrap().unwrap();

    // Trigger "boot complete" signal.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Execution State should be at the "Suspending" power level, 1.
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 1);

    let server_token_koid = &wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease.koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: wake_lease_name,
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    }
                }
            },
        }
    );

    drop(wake_lease);
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 0);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_take_application_activity_lease() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let aa_status = status_endpoints.get("application_activity").unwrap();
    assert_eq!(
        aa_status.watch_power_level().await?.unwrap(),
        0 /* ApplicationActivityLevel::Inactive */
    );

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let application_activity_lease_name = "application_activity_lease";
    let application_activity_lease =
        activity_governor.take_application_activity_lease(application_activity_lease_name).await?;

    // Trigger "boot complete" signal.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    assert_eq!(
        aa_status.watch_power_level().await?.unwrap(),
        1 /* ApplicationActivityLevel::Active */
    );

    let server_token_koid =
        &application_activity_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: application_activity_lease.koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: application_activity_lease_name,
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                        "is_unmonitored_lease": true,
                    }
                }
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "2": {
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: application_activity_lease_name,
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "3": {
                    ref fobs::WAKE_LEASE_SATISFIED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: application_activity_lease_name,
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "fuchsia.inspect.Stats": contains {},
            }
        }
    );

    drop(application_activity_lease);
    assert_eq!(aa_status.watch_power_level().await?.unwrap(), 0);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
        }
    );

    Ok(())
}

// Places an element between (Execution State, Active) and (Application Activity, Active).
#[fuchsia::test]
async fn test_activity_governor_add_application_activity_dependency() -> Result<()> {
    let (realm, _activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let execution_state_manager =
        realm.connect_to_protocol::<fsystem::ExecutionStateManagerMarker>().await?;

    let topology = realm.connect_to_protocol::<fbroker::TopologyMarker>().await?;
    let (element_runner_client, element_runner_server) =
        create_endpoints::<fbroker::ElementRunnerMarker>();
    let mut element_runner_stream = element_runner_server.into_stream();

    let execution_state_token = execution_state_manager
        .get_execution_state_dependency_token()
        .await?
        .dependency_token
        .unwrap();

    let test_element =
        PowerElementContext::builder(&topology, "test_element", &[0, 1], element_runner_client)
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(1),
                requires_token: Some(execution_state_token),
                requires_level_by_preference: Some(vec![
                    fsystem::ExecutionStateLevel::Active.into_primitive(),
                ]),
                ..Default::default()
            }])
            .build()
            .await?;

    let dependency_token = test_element.assertive_dependency_token().unwrap();

    execution_state_manager
        .add_application_activity_dependency(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
                dependency_token: Some(dependency_token),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await?
        .unwrap();

    // Open status channel for test_element.
    let (status_client, status_server) = fidl::endpoints::create_proxy::<fbroker::StatusMarker>();
    test_element.element_control.open_status_channel(status_server)?;

    // Start a no-op element runner.
    let _runner_task = fasync::Task::local(async move {
        while let Some(Ok(request)) = element_runner_stream.next().await {
            match request {
                fbroker::ElementRunnerRequest::SetLevel { responder, .. } => {
                    responder.send().unwrap();
                }
                _ => {}
            }
        }
    });

    // Confirm the Application Activity dependency on the test element:
    //  1. Verify the test element starts at level 0.
    //  2. Take an Application Activity lease.
    //  3. Confirm that the test element rises to level 1.
    //  4. Drop the lease.
    //  5. Confirm that the test element drops to level 0.
    assert_eq!(status_client.watch_power_level().await?.unwrap(), 0);
    let lease_name = "test_app_activity_lease";
    let lease = activity_governor.take_application_activity_lease(lease_name).await?;
    assert_eq!(status_client.watch_power_level().await?.unwrap(), 1);
    drop(lease);
    assert_eq!(status_client.watch_power_level().await?.unwrap(), 0);

    // Confirm the dependency on Execution State:
    //  1. Lease the test element, and wait for it to reach level 1.
    //  2. Trigger the boot complete signal.
    //  3. Confirm that Execution State is at Active.
    //  4. Drop the lease.
    //  5. Confirm that a suspend call is issued.
    let test_element_lease = self::lease(&test_element, 1).await?;
    assert_eq!(status_client.watch_power_level().await?.unwrap(), 1);

    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");
    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();
    let es_status = status_endpoints.get("execution_state").unwrap();
    assert_eq!(
        es_status.watch_power_level().await?.unwrap(),
        fsystem::ExecutionStateLevel::Active.into_primitive()
    );

    drop(test_element_lease);

    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_add_application_activity_dependency_errors() -> Result<()> {
    let (realm, _activity_governor_moniker) = create_realm().await?;
    let execution_state_manager =
        realm.connect_to_protocol::<fsystem::ExecutionStateManagerMarker>().await?;

    let topology = realm.connect_to_protocol::<fbroker::TopologyMarker>().await?;
    let (element_runner_client, _element_runner_server) =
        create_endpoints::<fbroker::ElementRunnerMarker>();

    let execution_state_token = execution_state_manager
        .get_execution_state_dependency_token()
        .await?
        .dependency_token
        .unwrap();

    let test_element =
        PowerElementContext::builder(&topology, "test_element", &[0, 1], element_runner_client)
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(1),
                requires_token: Some(execution_state_token),
                requires_level_by_preference: Some(vec![
                    fsystem::ExecutionStateLevel::Active.into_primitive(),
                ]),
                ..Default::default()
            }])
            .build()
            .await?;

    let dependency_token = test_element.assertive_dependency_token().unwrap();

    // Trigger AlreadyExists by adding the dependency twice.
    execution_state_manager
        .add_application_activity_dependency(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
                dependency_token: Some(dependency_token.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await?
        .unwrap();

    let res = execution_state_manager
        .add_application_activity_dependency(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
                dependency_token: Some(dependency_token.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(res, Err(fsystem::AddApplicationActivityDependencyError::AlreadyExists));

    // Trigger Invalid by providing an invalid power level with a valid token.
    let res = execution_state_manager
        .add_application_activity_dependency(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
                dependency_token: Some(dependency_token),
                power_level: Some(99),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(res, Err(fsystem::AddApplicationActivityDependencyError::Invalid));

    Ok(())
}

#[fuchsia::test]
async fn test_execution_state_manager_requests_queued_before_sag_initialized() -> Result<()> {
    let (realm, _activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;

    let execution_state_manager =
        realm.connect_to_protocol::<fsystem::ExecutionStateManagerMarker>().await?;
    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let topology = realm.connect_to_protocol::<fbroker::TopologyMarker>().await?;
    let (element_runner_client, _element_runner_server) =
        create_endpoints::<fbroker::ElementRunnerMarker>();

    let (cpu_driver_controller, _, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();
    let cpu_driver_token = cpu_driver_controller.assertive_dependency_token().unwrap();

    let test_element = PowerElementContext::builder(
        &topology,
        "test_element_queued",
        &[0, 1],
        element_runner_client,
    )
    .build()
    .await?;
    let dependency_token = test_element.assertive_dependency_token().unwrap();

    // Invoke ExecutionStateManager methods. Since SAG is not initialized yet (waiting for
    // suspending token), these calls should queue and block.
    let token_fut = execution_state_manager.get_execution_state_dependency_token();
    let dep_fut = execution_state_manager.add_application_activity_dependency(
        fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest {
            dependency_token: Some(dependency_token),
            power_level: Some(1),
            ..Default::default()
        },
    );

    // Verify that the futures are blocked and don't complete within a short window
    let mut token_fut = token_fut.boxed_local().fuse();
    let mut dep_fut = dep_fut.boxed_local().fuse();
    let mut timer =
        Box::pin(fasync::Timer::new(fasync::MonotonicDuration::from_millis(200))).fuse();
    futures::select! {
        _ = token_fut => panic!("token_fut completed prematurely"),
        _ = dep_fut => panic!("dep_fut completed prematurely"),
        _ = timer => {}, // Expected: timer fires first because the futures are queued/blocked
    }

    // Initialize SAG by providing the execution state dependency to CpuElementManager.
    cpu_element_manager
        .add_execution_state_dependency(
            fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                dependency_token: Some(cpu_driver_token),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await?
        .unwrap();

    // Now that SAG is initialized, the queued calls should complete.
    let execution_state_token_res = token_fut.await?;
    assert!(execution_state_token_res.dependency_token.is_some());

    let add_dep_res = dep_fut.await?;
    assert_eq!(add_dep_res, Ok(()));

    Ok(())
}

async fn get_diagnostics_hierarchy_for(moniker: &str) -> Result<DiagnosticsHierarchy> {
    let mut reader = ArchiveReader::inspect();
    reader
        .select_all_for_component(format!("{}/{}", REALM_FACTORY_CHILD_NAME, moniker))
        .with_minimum_schema_count(1);

    let inspect = reader
        .snapshot()
        .await?
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .ok_or_else(|| anyhow::anyhow!("expected one inspect hierarchy"))
        .unwrap();
    Ok(inspect)
}

// Ratio is in per-ten-thousands, valid values are [0, 10000].
fn get_vmo_util_ratio(inspect: &DiagnosticsHierarchy) -> u64 {
    let vmo_util_node_path = ["fuchsia.inspect.Stats", "utilization_per_ten_k"];
    inspect
        .get_property_by_path(&vmo_util_node_path)
        .expect("property")
        .uint()
        .ok_or(0u64)
        .expect("u64")
}

#[fuchsia::test]
async fn test_activity_governor_handles_1000_wake_leases() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let mut root_assertion = TreeAssertion::new("root", false);
    let mut wake_leases_child = TreeAssertion::new("wake_leases", true);
    let mut oldest_active_child = TreeAssertion::new("oldest_active", true);
    let mut wake_leases = Vec::new();

    for i in 0..1000u64 {
        let wake_lease_name = format!("wake_lease{}", i);
        let wake_lease =
            activity_governor.acquire_wake_lease(&wake_lease_name).await.unwrap().unwrap();

        let server_token_koid =
            &wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();
        let client_token_koid = &wake_lease.koid().unwrap().raw_koid();

        if i < 10 {
            let mut wake_lease_child = TreeAssertion::new(server_token_koid, false);
            wake_lease_child.add_property_assertion(
                fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT,
                Arc::new(NonZeroUintProperty),
            );
            wake_lease_child.add_property_assertion(
                fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID,
                Arc::new(*client_token_koid),
            );
            wake_lease_child
                .add_property_assertion(fobs::WAKE_LEASE_ITEM_NAME, Arc::new(wake_lease_name));
            wake_lease_child.add_property_assertion(fobs::WAKE_LEASE_ITEM_ID, Arc::new(i));
            wake_lease_child
                .add_property_assertion(fobs::WAKE_LEASE_ITEM_TYPE, Arc::new(AnyStringProperty));
            wake_lease_child
                .add_property_assertion(fobs::WAKE_LEASE_ITEM_STATUS, Arc::new(AnyStringProperty));
            oldest_active_child.add_child_assertion(wake_lease_child);
        }

        wake_leases.push(wake_lease);
    }
    wake_leases_child.add_child_assertion(oldest_active_child);
    wake_leases_child.add_property_assertion("active_count", Arc::new(1000u64));
    root_assertion.add_child_assertion(wake_leases_child);

    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let vmo_util = get_vmo_util_ratio(&inspect);
    log::info!("Current VMO utilization rate is {}/10000", vmo_util);
    assert_leq!(vmo_util, 9700); // If we reach 100% util, we'll silently lose data.

    root_assertion.run(&inspect).unwrap(); // Now check all the data.

    drop(wake_leases);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_inspect_buffer_not_exceeded() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let max_suspend_events_to_log = inspect
        .get_property_by_path(&["config", "max_suspend_events_to_log"])
        .expect("property max_suspend_events_to_log not found")
        .uint()
        .expect("max_suspend_events_to_log is not a uint");

    // Rapidly acquire and drop wake leases in batches to prevent handle/lease pile-up in SAG.
    // The batch size of 100 is chosen to limit open handles while minimizing Inspect query overhead
    // (avoiding test timeouts under high CQ load).
    let batch_size = 100;

    for i in 0..max_suspend_events_to_log {
        let _ = activity_governor.acquire_wake_lease(&format!("wake_lease{}", i)).await?;
        if (i + 1) % batch_size == 0 {
            // Wait for all of the batched wake leases to be dropped and processed.
            block_until_inspect_matches!(
                activity_governor_moniker,
                root: contains {
                    ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
                }
            );
        }
    }

    // Wait for any remaining wake leases to be dropped and processed.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
        }
    );

    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;

    let root_failed_allocations = inspect
        .get_property_by_path(&["fuchsia.inspect.Stats", "failed_allocations"])
        .expect("property failed_allocations not found")
        .uint()
        .expect("failed_allocations is not a uint");

    assert_eq!(root_failed_allocations, 0);

    let suspend_events_failed_allocations = inspect
        .get_property_by_path(&["suspend_events", "fuchsia.inspect.Stats", "failed_allocations"])
        .expect("property failed_allocations not found")
        .uint()
        .expect("failed_allocations is not a uint");

    assert_eq!(suspend_events_failed_allocations, 0);

    // Make sure the event list is full.
    let suspend_events_node =
        inspect.get_child_by_path(&[fobs::SUSPEND_EVENTS_NODE]).expect("property not found");
    // fuchsia.inspect.Stats is a sibling node for the events, so we subtract 1 from the count.
    assert_eq!(suspend_events_node.children.len() - 1, max_suspend_events_to_log as usize);

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_handles_1000_acquired_wake_leases() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let mut root_assertion = TreeAssertion::new("root", false);
    let mut wake_leases_child = TreeAssertion::new("wake_leases", true);
    let mut oldest_active_child = TreeAssertion::new("oldest_active", true);
    let mut wake_leases = Vec::new();

    for i in 0..1000u64 {
        let wake_lease_name = format!("wake_lease{}", i);
        let wake_lease = activity_governor.acquire_wake_lease(&wake_lease_name).await?.unwrap();

        let server_token_koid =
            &wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();
        let client_token_koid = &wake_lease.koid().unwrap().raw_koid();

        if i < 10 {
            let mut wake_lease_child = TreeAssertion::new(server_token_koid, false);
            wake_lease_child.add_property_assertion(
                fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT,
                Arc::new(NonZeroUintProperty),
            );
            wake_lease_child.add_property_assertion(
                fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID,
                Arc::new(*client_token_koid),
            );
            wake_lease_child
                .add_property_assertion(fobs::WAKE_LEASE_ITEM_NAME, Arc::new(wake_lease_name));
            wake_lease_child.add_property_assertion(fobs::WAKE_LEASE_ITEM_ID, Arc::new(i));
            wake_lease_child
                .add_property_assertion(fobs::WAKE_LEASE_ITEM_TYPE, Arc::new(AnyStringProperty));
            wake_lease_child
                .add_property_assertion(fobs::WAKE_LEASE_ITEM_STATUS, Arc::new(AnyStringProperty));
            oldest_active_child.add_child_assertion(wake_lease_child);
        }
        wake_leases.push(wake_lease);
    }
    wake_leases_child.add_child_assertion(oldest_active_child);
    wake_leases_child.add_property_assertion("active_count", Arc::new(1000u64));
    root_assertion.add_child_assertion(wake_leases_child);

    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let vmo_util = get_vmo_util_ratio(&inspect);
    log::info!("Current VMO utilization rate is {}/10000", vmo_util);
    assert_leq!(vmo_util, 9700); // If we reach 100% util, we'll silently lose data.

    root_assertion.run(&inspect).unwrap(); // Now check all the data.

    drop(wake_leases);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_acquire_wake_lease_returns_error_on_empty_name() -> Result<()> {
    let (realm, _activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    assert_eq!(
        fsystem::AcquireWakeLeaseError::InvalidName,
        activity_governor.acquire_wake_lease("").await?.unwrap_err()
    );

    // Second call should succeed.
    activity_governor.acquire_wake_lease("test").await.unwrap().unwrap();
    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_acquire_wake_lease_with_token_returns_error_on_empty_name()
-> Result<()> {
    let (realm, _activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let (mut _client_side, mut server_side) = zx::EventPair::create();
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    assert_eq!(
        fsystem::AcquireWakeLeaseError::InvalidName,
        activity_governor.acquire_wake_lease_with_token("", server_side).await?.unwrap_err()
    );

    (_client_side, server_side) = zx::EventPair::create();
    // Second call should succeed.
    activity_governor.acquire_wake_lease_with_token("test", server_side).await.unwrap().unwrap();
    Ok(())
}

async fn create_cpu_driver_topology(
    realm: &RealmProxyClient,
) -> Result<(Arc<PowerElementContext>, Arc<Cell<fbroker::PowerLevel>>, fasync::Task<()>)> {
    let topology = realm.connect_to_protocol::<fbroker::TopologyMarker>().await?;
    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let cpu_element_token = cpu_element_manager
        .get_cpu_dependency_token()
        .await
        .unwrap()
        .assertive_dependency_token
        .unwrap();

    let (element_runner_client, element_runner) =
        create_endpoints::<fbroker::ElementRunnerMarker>();
    let cpu_driver_controller = Arc::new(
        PowerElementContext::builder(&topology, "cpu_driver", &[0, 1], element_runner_client)
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(1),
                requires_token: Some(cpu_element_token),
                requires_level_by_preference: Some(vec![1]),
                ..Default::default()
            }])
            .build()
            .await?,
    );

    let cpu_driver_context = cpu_driver_controller.clone();
    let cpu_driver_power_level = Arc::new(Cell::new(0));
    let cpu_driver_power_level2 = cpu_driver_power_level.clone();

    let cpu_driver_task = fasync::Task::local(async move {
        let cpu_driver_power_level = cpu_driver_power_level2.clone();

        cpu_driver_context
            .run(
                element_runner,
                None, /* inspect_node */
                Some(Box::new(move |new_power_level: fbroker::PowerLevel| {
                    let cpu_driver_power_level = cpu_driver_power_level.clone();

                    async move {
                        cpu_driver_power_level.set(new_power_level);
                    }
                    .boxed_local()
                })),
            )
            .await;
    });

    Ok((cpu_driver_controller, cpu_driver_power_level, cpu_driver_task))
}

#[fuchsia::test]
async fn test_activity_governor_cpu_element_and_execution_state_interaction() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let (cpu_driver_controller, cpu_driver_power_level, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();

    fasync::Task::local(async move {
        cpu_element_manager
            .add_execution_state_dependency(
                fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                    dependency_token: Some(
                        cpu_driver_controller.assertive_dependency_token().unwrap(),
                    ),
                    power_level: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
    })
    .detach();

    // This call should not be processed until the topology is set up.
    let _wake_lease = activity_governor.acquire_wake_lease("wake_lease").await.unwrap().unwrap();

    // Trigger "boot complete" signal.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 1u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 1u64,
                },
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
        }
    );

    assert_eq!(1u8, cpu_driver_power_level.get());

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_cpu_element_returns_invalid_args() -> Result<()> {
    let (realm, _) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;

    // Empty request should return InvalidArgs.
    assert_eq!(
        fsystem::AddExecutionStateDependencyError::InvalidArgs,
        cpu_element_manager
            .add_execution_state_dependency(
                fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                    dependency_token: None,
                    power_level: None,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap_err()
    );

    // Missing token should return InvalidArgs.
    assert_eq!(
        fsystem::AddExecutionStateDependencyError::InvalidArgs,
        cpu_element_manager
            .add_execution_state_dependency(
                fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                    dependency_token: None,
                    power_level: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap_err()
    );

    // Missing power level should return InvalidArgs.
    assert_eq!(
        fsystem::AddExecutionStateDependencyError::InvalidArgs,
        cpu_element_manager
            .add_execution_state_dependency(
                fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                    dependency_token: Some(zx::Event::create()),
                    power_level: None,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap_err()
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_cpu_element_returns_bad_state() -> Result<()> {
    let (realm, _) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let (cpu_driver_controller, _, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();

    cpu_element_manager
        .add_execution_state_dependency(
            fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                dependency_token: Some(cpu_driver_controller.assertive_dependency_token().unwrap()),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        fsystem::AddExecutionStateDependencyError::BadState,
        cpu_element_manager
            .add_execution_state_dependency(
                fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                    dependency_token: Some(
                        cpu_driver_controller.assertive_dependency_token().unwrap()
                    ),
                    power_level: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap_err()
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_cpu_element_allows_leases_during_boot() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let (cpu_driver_controller, cpu_driver_power_level, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();

    // The CPU power element should be powered up on boot.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            power_elements: {
                cpu: {
                    power_level: 1u64,
                },
            },
        }
    );

    assert_eq!(0u8, cpu_driver_power_level.get());
    let _cpu_lease = lease(&cpu_driver_controller, 1).await.unwrap();
    assert_eq!(1u8, cpu_driver_power_level.get());
    Ok(())
}

#[fuchsia::test]
async fn test_acquire_wake_lease_blocks_during_suspend() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    // Spawn an AcquireWakeLease request, and ensure that it's still blocked after a brief wait.
    let mut task = fasync::Task::local(async move {
        activity_governor.acquire_wake_lease("some_wake_lease").await
    });
    fasync::Timer::new(fasync::MonotonicDuration::from_seconds(1)).await;
    assert!(futures::poll!(&mut task).is_pending());

    // Allow the system to resume and confirm that AcquireWakeLease returns.
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();
    let _ = task.await;

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "3": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "4": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "5": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "6": {
                    ref fobs::SUSPEND_LAST_TIMESTAMP: 2u64,
                    ref fobs::SUSPEND_RESUMED_AT: AnyProperty,
                    ref fobs::SUSPEND_CUMULATIVE_DURATION: 2u64,
                },
                "7": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "8": {
                    ref fobs::SUSPEND_BLOCKER_ACQUIRED_AT: AnyProperty,
                },
                "9": {
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "some_wake_lease",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "10": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "11": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "12": {
                    ref fobs::WAKE_LEASE_SATISFIED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "some_wake_lease",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "13": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "some_wake_lease",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "14": {
                    ref fobs::SUSPEND_BLOCKER_DROPPED_AT: AnyProperty,
                },
                "15": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "16": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "17": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "18": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_last_wake_lease_blocks_suspend_lifo() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let mut wake_leases = Vec::new();

    for i in 0..2 {
        let wake_lease_name = format!("wake_lease{}", i);
        let wake_lease = activity_governor.acquire_wake_lease(&wake_lease_name).await?.unwrap();
        let server_token_koid =
            wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

        wake_leases.push((wake_lease, server_token_koid));
    }

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Ensure the wake lease is holding Execution State up.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 1u64,
                },
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_BLOCKER_ACQUIRED_AT: AnyProperty,
                },
                "3": {
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease0",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                // Events 2-4 cover the creation and satisfaction of the two
                // wake leases. These events could occur in any order.
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    drop(wake_leases.pop());
    let last_token_koid = &wake_leases[0].1;

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 1u64,
                },
            },
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var last_token_koid: contains {
                        ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease0",
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                    },
                }
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                // Events 4-6 cover the creation and satisfaction of the two
                // wake leases. These events could occur in any order.
                "7": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease1",
                    ref fobs::WAKE_LEASE_ITEM_ID: 1u64,
                },
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    drop(wake_leases.pop());

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 0u64,
                },
            },
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "8": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease0",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "9": {
                    ref fobs::SUSPEND_BLOCKER_DROPPED_AT: AnyProperty,
                },
                "10": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "11": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "12": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "13": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_last_wake_lease_blocks_suspend_fifo() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let mut wake_leases = Vec::new();

    for i in 0..2 {
        let wake_lease_name = format!("wake_lease{}", i);
        let wake_lease = activity_governor.acquire_wake_lease(&wake_lease_name).await?.unwrap();
        let server_token_koid =
            wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

        wake_leases.insert(0, (wake_lease, server_token_koid));
    }

    let koid_0 = &wake_leases[0].1;
    let koid_1 = &wake_leases[1].1;

    // Stabilize async state: Wait for both wake leases to be satisfied BEFORE
    // triggering BootControl drop. This ensures ExecutionState goes Active -> Suspending,
    // avoiding an intermittent dip to Inactive that produces extra tracking events.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var koid_0: contains {
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    },
                    var koid_1: contains {
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    },
                }
            }
        }
    );

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Ensure the wake lease is holding Execution State up.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 1u64,
                },
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "2": {
                    ref fobs::SUSPEND_BLOCKER_ACQUIRED_AT: AnyProperty,
                },
                "3": {
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease0",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                // Events 4-6 cover the creation and satisfaction of the two
                // wake leases. These events could occur in any order.
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    drop(wake_leases.pop());
    let last_token_koid = &wake_leases[0].1;

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 1u64,
                },
            },
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var last_token_koid: contains {
                        ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease1",
                        ref fobs::WAKE_LEASE_ITEM_ID: 1u64,
                    },
                }
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "7": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease0",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    drop(wake_leases.pop());

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: contains {
                execution_state: {
                    power_level: 0u64,
                },
            },
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "8": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "wake_lease1",
                    ref fobs::WAKE_LEASE_ITEM_ID: 1u64,
                },
                "9": {
                    ref fobs::SUSPEND_BLOCKER_DROPPED_AT: AnyProperty,
                },
                "10": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "11": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "12": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "13": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
            },
            "suspend_events_stats": contains {},
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[derive(Debug, PartialEq)]
enum SuspendBlockerRequestType {
    BeforeSuspend,
    AfterResume,
}

#[fuchsia::test]
async fn test_suspend_blocker_receives_calls_on_suspend_resume() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    let (mut state_tx, mut state_rx) = mpsc::channel(1);

    fasync::Task::local(async move {
        while let Some(req) = suspend_blocker_stream.next().await {
            match req {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::BeforeSuspend).unwrap();
                    responder.send().unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::AfterResume).unwrap();
                    responder.send().unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    })
    .detach();

    // Call SetBootComplete to allow SAG to start suspending.
    {
        activity_governor
            .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(suspend_blocker_client_end),
                name: Some("test_suspend_blocker_receives_calls_on_suspend_resume".to_string()),
                ..Default::default()
            })
            .await
            .unwrap()
            .unwrap();

        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    assert_eq!(SuspendBlockerRequestType::BeforeSuspend, state_rx.next().await.unwrap());
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    // Allow the system to resume and confirm that AcquireWakeLease returns.
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(SuspendBlockerRequestType::AfterResume, state_rx.next().await.unwrap());

    Ok(())
}

#[fuchsia::test]
async fn test_suspend_blocker_receives_no_calls_during_shutdown_with_simple_topology() -> Result<()>
{
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    let (mut state_tx, mut state_rx) = mpsc::channel(1);

    fasync::Task::local(async move {
        while let Some(req) = suspend_blocker_stream.next().await {
            match req {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::BeforeSuspend).unwrap();
                    responder.send().unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::AfterResume).unwrap();
                    responder.send().unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    })
    .detach();

    // Register a suspend blocker and call SetBootComplete to allow SAG to start suspending.
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some("test_suspend_blocker_receives_calls_on_suspend_resume".to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let admin = realm.connect_to_protocol::<fstatecontrol::AdminMarker>().await?;
    admin
        .shutdown(&fstatecontrol::ShutdownOptions {
            action: Some(fstatecontrol::ShutdownAction::Reboot),
            reasons: Some(vec![fstatecontrol::ShutdownReason::UserRequest]),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }
    // Wait 3 seconds and assert no BeforeSuspend calls were made.
    fasync::Timer::new(std::time::Duration::from_millis(3000)).await;
    assert!(state_rx.try_next().is_err());
    Ok(())
}

#[fuchsia::test]
async fn test_suspend_blocker_receives_no_calls_during_shutdown_with_execution_state_dependency()
-> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let (cpu_driver_controller, _, cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();
    let cpu_driver_token = cpu_driver_controller.assertive_dependency_token().unwrap();

    // With wait_for_suspending_token, SAG will not allow other FIDL calls until it receives the
    // suspending token. We add an execution state dependency to simulate this.
    fasync::Task::local(async move {
        cpu_element_manager
            .add_execution_state_dependency(
                fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                    dependency_token: Some(cpu_driver_token),
                    power_level: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
    })
    .detach();

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    let (mut state_tx, mut state_rx) = mpsc::channel(1);

    fasync::Task::local(async move {
        while let Some(req) = suspend_blocker_stream.next().await {
            match req {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::BeforeSuspend).unwrap();
                    responder.send().unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::AfterResume).unwrap();
                    responder.send().unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    })
    .detach();

    // Register a suspend blocker and call SetBootComplete to allow SAG to start suspending.
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some("test_suspend_blocker_receives_calls_on_suspend_resume".to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Ensure SAG is serving power elements before shutting down.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: true,
            is_shutting_down: false,
            power_elements: contains {
                execution_state: {
                    power_level: 2u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 1u64,
                },
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
        }
    );

    let admin = realm.connect_to_protocol::<fstatecontrol::AdminMarker>().await?;
    admin
        .shutdown(&fstatecontrol::ShutdownOptions {
            action: Some(fstatecontrol::ShutdownAction::Reboot),
            reasons: Some(vec![fstatecontrol::ShutdownReason::UserRequest]),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // During shutdown, dependencies of execution_state will be terminated which will affect SAG's
    // topology. We drop the cpu_driver_controller to simulate this.
    drop(cpu_driver_controller);
    drop(cpu_driver_task);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: true,
            is_shutting_down: true,
        }
    );

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Wait for SAG to finish processing the shutdown.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            is_shutting_down: true,
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
        }
    );

    // Wait 3 seconds and assert no BeforeSuspend/AfterResume calls were made.
    fasync::Timer::new(std::time::Duration::from_secs(3)).await;
    assert!(state_rx.try_next().is_err());
    Ok(())
}

#[fuchsia::test]
async fn test_register_suspend_blocker_responds_with_invalid_args_error_when_missing_args()
-> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    // No args.
    assert_eq!(
        fsystem::RegisterSuspendBlockerError::InvalidArgs,
        activity_governor
            .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                ..Default::default()
            },)
            .await
            .unwrap()
            .unwrap_err()
    );

    // No blocker.
    assert_eq!(
        fsystem::RegisterSuspendBlockerError::InvalidArgs,
        activity_governor
            .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                name: Some("abc".to_string()),
                ..Default::default()
            },)
            .await
            .unwrap()
            .unwrap_err()
    );

    let (suspend_blocker_client_end, _suspend_blocker_server_end) =
        fidl::endpoints::create_endpoints();

    // Invalid name.
    assert_eq!(
        fsystem::RegisterSuspendBlockerError::InvalidArgs,
        activity_governor
            .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(suspend_blocker_client_end),
                name: Some("".to_string()),
                ..Default::default()
            },)
            .await
            .unwrap()
            .unwrap_err()
    );

    Ok(())
}

#[fuchsia::test]
async fn test_register_suspend_blocker_only_before_suspend_called_after_register_during_suspending()
-> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    // Register a suspend blocker to know when suspend transitions start.
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some("suspend_notifier".to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (mut suspend_tx, mut suspend_rx) = mpsc::channel(1);
    let (mut before_suspend_tx, mut before_suspend_rx) = mpsc::channel(1);
    let transition_blocker = fasync::Task::local(async move {
        while let Some(req) = suspend_blocker_stream.next().await {
            match req {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    before_suspend_tx.try_send(()).unwrap();
                    suspend_rx.next().await.unwrap();
                    responder.send().unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    responder.send().unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    });

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    before_suspend_rx.next().await.unwrap();

    // At this point, BeforeSuspend calls are in progress, so register now.

    let (new_blocker_client_end, mut new_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();
    let (mut state_tx, mut state_rx) = mpsc::channel(1);
    fasync::Task::local(async move {
        while let Some(req) = new_blocker_stream.next().await {
            match req {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::BeforeSuspend).unwrap();
                    responder.send().unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    state_tx.try_send(SuspendBlockerRequestType::AfterResume).unwrap();
                    responder.send().unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    })
    .detach();

    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(new_blocker_client_end),
            name: Some("calls_on_suspend_resume".to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Allow the BeforeSuspend call to complete and the system to suspend.
    suspend_tx.try_send(()).unwrap();
    // Drop the blocker to unregister it. We no longer need it.
    drop(transition_blocker);
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    // Allow the system to resume.
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // With no active wake leases, suspend will occur imminently.
    assert_eq!(SuspendBlockerRequestType::BeforeSuspend, state_rx.next().await.unwrap());

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_suspends_after_suspend_blocker_hangs_after_resume() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    // First watch should return immediately with default values.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);

    let blocker_name = "hangs_after_resume".to_string();
    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream();
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some(blocker_name.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (before_suspend_tx, mut before_suspend_rx) = mpsc::channel(1);
    let (after_resume_tx, mut after_resume_rx) = mpsc::channel(1);

    fasync::Task::local(async move {
        let mut before_suspend_tx = before_suspend_tx;
        let mut after_resume_tx = after_resume_tx;

        while let Some(Ok(req)) = suspend_blocker_stream.next().await {
            match req {
                fsystem::SuspendBlockerRequest::AfterResume { .. } => {
                    // AfterResume never responds.
                    // Check SAG state after resume to confirm SAG doesn't block on the AfterResume.
                    after_resume_tx.try_send(()).unwrap();
                }
                fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                    responder.send().unwrap();
                    before_suspend_tx.try_send(()).unwrap();
                }
                fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Unexpected method: {}", ordinal);
                }
            }
        }
    })
    .detach();

    // Stabilize async state: Wait for registration wake lease to be satisfied BEFORE
    // triggering BootControl drop. This ensures ExecutionState goes
    // Active -> Suspending -> Inactive predictably, rather than bouncing unpredictably due to IPC
    // race conditions.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "4": {
                    ref fobs::WAKE_LEASE_SATISFIED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "hangs_after_resume",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "fuchsia.inspect.Stats": contains {},
            }
        }
    );

    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Await SAG's power elements to drop their power levels.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 0u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: -1i64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 0u64,
                ref fobs::SUSPEND_LAST_DURATION: -1i64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: {
                "0": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "1": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "2": {
                    ref fobs::SUSPEND_BLOCKER_ACQUIRED_AT: AnyProperty,
                },
                "3": {
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "hangs_after_resume",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "4": {
                    ref fobs::WAKE_LEASE_SATISFIED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "hangs_after_resume",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "5": {
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                    ref fobs::WAKE_LEASE_ITEM_NAME: "hangs_after_resume",
                    ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                },
                "6": {
                    ref fobs::SUSPEND_BLOCKER_DROPPED_AT: AnyProperty,
                },
                "7": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "8": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "9": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "10": {
                    ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "power_observability_state_recorders": contains {},
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
            "suspend_blockers": {
                "names": vec![blocker_name.clone()],
            },
        }
    );

    // BeforeSuspend should have been called once.
    before_suspend_rx.next().await.unwrap();

    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Should only have been 1 suspend after all suspend blocker handling.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(1), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(Some(2), current_stats.last_time_in_suspend);
    assert_eq!(Some(2), current_stats.total_time_in_suspend);

    // AfterResume should have been called once.
    after_resume_rx.next().await.unwrap();

    // AfterResume does not block. SAG raises ExecutionState to Suspending state.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            power_elements: {
                execution_state: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
                application_activity: {
                    power_level: 0u64,
                },
                cpu: {
                    // Due to timeout of the resume lease, expect this to be 0.
                    power_level: 0u64,
                },
            },
            suspend_stats: {
                ref fobs::SUSPEND_SUCCESS_COUNT: 1u64,
                ref fobs::SUSPEND_FAIL_COUNT: 0u64,
                ref fobs::SUSPEND_LAST_FAILED_ERROR: 0u64,
                ref fobs::SUSPEND_LAST_TIMESTAMP: 2u64,
                ref fobs::SUSPEND_CUMULATIVE_DURATION: 2u64,
                ref fobs::SUSPEND_LAST_DURATION: 1u64,
            },
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "11": {
                    ref fobs::SUSPEND_RESUMED_AT: AnyProperty,
                    ref fobs::SUSPEND_LAST_TIMESTAMP: 2u64,
                    ref fobs::SUSPEND_CUMULATIVE_DURATION: 2u64,
                },
                "12": {
                    ref fobs::SUSPEND_LOCK_DROPPED_AT: AnyProperty,
                },
                "13": {
                    ref fobs::RESUME_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "14": {
                    ref fobs::RESUME_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "15": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_START_AT: AnyProperty,
                 },
                "16": {
                    ref fobs::SUSPEND_CALLBACK_PHASE_END_AT: AnyProperty,
                 },
                "17": {
                    ref fobs::SUSPEND_LOCK_ACQUIRED_AT: AnyProperty,
                },
                "18": {
                   ref fobs::SUSPEND_ATTEMPTED_AT: AnyProperty,
                },
                "fuchsia.inspect.Stats": contains {},
            },
            "power_observability_state_recorders": contains {},
            "suspend_events_stats": contains {},
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "fuchsia.inspect.Stats": contains {},
            "suspend_blockers": {
                // When the SuspendBlocker dropped its responder without sending a response, the
                // FIDL channel was closed, causing SAG to unregister the blocker.
                "names": Vec::<String>::new(),
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_cleans_up_suspend_blocker_on_channel_drop() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let (suspend_blocker_client_end, suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    let blocker_name = "test_blocker".to_string();

    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some(blocker_name.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Verify the suspend blocker was actively added to the list in inspect
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            "suspend_blockers": {
                "names": vec![blocker_name.clone()],
            },
        }
    );

    // Drop the server side of the channel, simulating the client disconnecting or dying
    drop(suspend_blocker_stream);

    // Verify the suspend blocker is automatically pruned from the lists without a suspend cycle
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            "suspend_blockers": {
                "names": Vec::<String>::new(),
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_cleans_up_suspend_blocker_on_channel_drop_after_resume()
-> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    let blocker_name = "test_blocker".to_string();

    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some(blocker_name.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (mut after_resume_tx, mut after_resume_rx) = mpsc::channel(1);

    // Since we don't save the lease returned by `register_suspend_blocker`, it drops immediately.
    // This can cause unpredictable suspend/resume cycles depending on when SAG processes the drop.
    // Instead of counting cycles, we use `drop_tx` to control exactly when the suspend blocker
    // stream is dropped, which deterministically unregisters the blocker.
    let (drop_tx, mut drop_rx) = mpsc::channel::<()>(1);

    fasync::Task::local(async move {
        loop {
            futures::select! {
                req = suspend_blocker_stream.next() => {
                    match req {
                        Some(Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder })) => {
                            responder.send().unwrap();
                        }
                        Some(Ok(fsystem::SuspendBlockerRequest::AfterResume { responder })) => {
                            let _ = after_resume_tx.try_send(());
                            responder.send().unwrap();
                        }
                        Some(Ok(fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. })) => {
                            panic!("Unexpected method: {}", ordinal);
                        }
                        _ => break,
                    }
                }
                _ = drop_rx.next() => {
                    break; // Causes suspend_blocker_stream to be dropped
                }
            }
        }
    })
    .detach();

    // Trigger a suspend cycle.
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(2i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Acquire a wake lease after resume is allowed to prevent extra suspend cycles.
    let _wake_lease = activity_governor.acquire_wake_lease("test_wake_lease").await?;

    // Wait for the cycle to complete and AfterResume to be called.
    after_resume_rx.next().await.unwrap();

    // Verify the suspend blocker is still in the inspect node (it was moved to the active list
    // during the suspend cycle but is still alive).
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            "suspend_blockers": {
                "names": vec![blocker_name.clone()],
            },
        }
    );

    // Drop the server side of the channel, simulating the client disconnecting or dying.
    drop(drop_tx);

    // Verify the suspend blocker is automatically pruned from the lists.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            "suspend_blockers": {
                "names": Vec::<String>::new(),
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_blocks_for_before_suspend() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let stats = realm.connect_to_protocol::<fsuspend::StatsMarker>().await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    // First watch should return immediately with default values.
    let current_stats = stats.watch().await?;
    assert_eq!(Some(0), current_stats.success_count);
    assert_eq!(Some(0), current_stats.fail_count);
    assert_eq!(None, current_stats.last_failed_error);
    assert_eq!(None, current_stats.last_time_in_suspend);
    assert_eq!(Some(0), current_stats.total_time_in_suspend);

    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream();
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some("test_activity_governor_blocks_for_before_suspend".to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (before_suspend_tx, mut before_suspend_rx) = mpsc::channel(1);
    fasync::Task::local(async move {
        let mut before_suspend_tx = before_suspend_tx;
        let mut _before_suspend_responder;

        while let Some(Ok(req)) = suspend_blocker_stream.next().await {
            match req {
                fsystem::SuspendBlockerRequest::AfterResume { responder } => {
                    responder.send().unwrap();
                }
                fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                    _before_suspend_responder = responder;
                    before_suspend_tx.try_send(()).unwrap();
                }
                fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Unexpected method: {}", ordinal);
                }
            }
        }
    })
    .detach();

    // Queue up a callback from `suspend_device`, to let us know when
    // SAG requests to suspend the hardware.
    let await_suspend = suspend_device.await_suspend();

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Wait to receive the BeforeSuspend() callback.
    before_suspend_rx.next().await.unwrap();

    // Give SAG some time to take any further suspend actions.
    fasync::Timer::new(fasync::MonotonicDuration::from_millis(1000)).await;

    // Verify that SAG did _not_ suspend the hardware (because we did not
    // respond to the callback).
    assert!(await_suspend.now_or_never().is_none());

    Ok(())
}

#[fuchsia::test]
async fn test_acquire_wake_lease_doesnt_deadlock_in_before_suspend() -> Result<()> {
    let (realm, _) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream();
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker_client_end),
            name: Some("test_acquire_wake_lease_doesnt_deadlock_in_before_suspend".to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Define the suspend blocker such that:
    //  - AfterResume notifies after_resume_tx.
    //  - BeforeSuspend calls AcquireWakeLease and passes the lease to before_suspend_rx.
    let (after_resume_tx, mut after_resume_rx) = mpsc::channel(1);
    let (before_suspend_tx, mut before_suspend_rx) = mpsc::channel(1);
    fasync::Task::local(async move {
        let mut after_resume_tx = after_resume_tx;
        let mut before_suspend_tx = before_suspend_tx;

        while let Some(Ok(req)) = suspend_blocker_stream.next().await {
            match req {
                fsystem::SuspendBlockerRequest::AfterResume { responder } => {
                    log::info!("Running AfterResume");
                    after_resume_tx.try_send(()).unwrap();
                    responder.send().unwrap();
                }
                fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                    log::info!("Running BeforeSuspend");

                    // Ensure that multiple leases can be obtained. In http://fxbug.dev/470037379,
                    // lease requests beyond the first would block.
                    let mut leases = Vec::new();
                    for _ in 0..10 {
                        let lease = activity_governor
                            .acquire_wake_lease("before_suspend_wake_lease")
                            .await
                            .unwrap()
                            .unwrap();
                        leases.push(lease);
                    }

                    before_suspend_tx.try_send(leases).unwrap();
                    responder.send().unwrap()
                }
                fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Unexpected method: {}", ordinal);
                }
            }
        }
    })
    .detach();

    // Call SetBootComplete to allow SAG to start suspending.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Wait to receive the wake leases from BeforeSuspend.
    let _wake_leases = before_suspend_rx.next().await.unwrap();

    // Verify that SAG did not call Suspender.Suspend due to the existence of the wake lease.
    assert!(suspend_device.await_suspend().now_or_never().is_none());

    // Now wait for the resume callback resulting from BeforeSuspend's wake lease.
    after_resume_rx.next().await.unwrap();

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_captures_inspect_event_buffer_stats() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let max_suspend_events_to_log = inspect
        .get_property_by_path(&["config", "max_suspend_events_to_log"])
        .expect("property max_suspend_events_to_log not found")
        .uint()
        .expect("max_suspend_events_to_log is not a uint");

    // Rapidly acquire and drop wake leases in batches to prevent handle/lease pile-up in SAG.
    let batch_size = 1000;
    let mut futures = Vec::new();
    for i in 0..max_suspend_events_to_log {
        futures.push(activity_governor.acquire_wake_lease(&format!("wake_lease{}", i)));
        if futures.len() == batch_size {
            // Wait for all acquisitions in this batch to resolve in parallel.
            let leases = futures::future::try_join_all(std::mem::take(&mut futures)).await?;
            // Drop them all to release the leases.
            drop(leases);

            // Wait for all of the batched wake leases to be dropped and processed.
            block_until_inspect_matches!(
                activity_governor_moniker,
                root: contains {
                    ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
                }
            );
        }
    }

    // Process any remaining futures in the final incomplete batch.
    if !futures.is_empty() {
        let leases = futures::future::try_join_all(futures).await?;
        drop(leases);

        block_until_inspect_matches!(
            activity_governor_moniker,
            root: contains {
                ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            }
        );
    }

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            booting: false,
            "suspend_events_stats": {
                event_capacity: 6144u64,
                history_duration_seconds: AnyProperty,
                at_capacity_history_duration_seconds: AnyProperty,
            },
            "fuchsia.inspect.Health": contains {
                status: "OK",
            },
            "suspend_events": contains {
                "fuchsia.inspect.Stats": contains {},
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_acquire_unmonitored_wake_lease_raises_execution_state_to_suspending()
-> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let es_status = status_endpoints.get("execution_state").unwrap();
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let wake_lease_name = "wake_lease";
    let wake_lease =
        activity_governor.acquire_unmonitored_wake_lease(wake_lease_name).await.unwrap().unwrap();

    // Trigger "boot complete" signal.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Execution State should be at the "Suspending" power level, 1.
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 1);

    let server_token_koid = &wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var server_token_koid: contains {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease.koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: wake_lease_name,
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    "is_unmonitored_lease": true,
                }
            }
        },
        }
    );

    drop(wake_lease);
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 0);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains { active_count: 0u64 },
            config: {
                use_suspender: true,
                wait_for_suspending_token: false,
                max_suspend_events_to_log: 6144u64,
                suspend_resume_stuck_warning_timeout: 60u64,
                max_active_wake_leases_to_log: 10u64,
                reboot_on_stalled_suspend_blocker: false,
                suspend_loop_max_attempts: 10u64,
                long_wake_lease_timeout: 60u64,
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_acquire_and_drop_wake_lease_during_before_suspend() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let (suspend_blocker_client_end, mut suspend_blocker_stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();

    let (before_suspend_received_tx, mut before_suspend_received_rx) =
        futures::channel::mpsc::unbounded();
    let (respond_before_suspend_tx, mut respond_before_suspend_rx) =
        futures::channel::mpsc::unbounded();
    let (after_resume_received_tx, mut after_resume_received_rx) =
        futures::channel::mpsc::unbounded();

    fasync::Task::local(async move {
        while let Some(req) = suspend_blocker_stream.next().await {
            match req {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    before_suspend_received_tx.unbounded_send(()).unwrap();
                    respond_before_suspend_rx.next().await.unwrap();
                    responder.send().unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    let _ = after_resume_received_tx.unbounded_send(());
                    responder.send().unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    })
    .detach();

    // Call SetBootComplete to allow SAG to start suspending.
    {
        activity_governor
            .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(suspend_blocker_client_end),
                name: Some("test_acquire_and_drop_wake_lease_during_before_suspend".to_string()),
                ..Default::default()
            })
            .await
            .unwrap()
            .unwrap();

        // Wait for the blocker's wake lease to be cleaned up before proceeding.
        block_until_inspect_matches!(
            activity_governor_moniker,
            root: contains {
                ref fobs::WAKE_LEASES_NODE: contains {
                    active_count: 0u64,
                },
            }
        );

        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Wait for BeforeSuspend to be received.
    before_suspend_received_rx.next().await.unwrap();

    // Acquire a wake lease while BeforeSuspend is running.
    let wake_lease_name = "test_wake_lease";
    let wake_lease = activity_governor.acquire_wake_lease(wake_lease_name).await.unwrap().unwrap();
    let server_token_koid = &wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    // Verify that the wake lease is registered with SAG.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                active_count: 1u64,
                oldest_active: contains {
                    var server_token_koid: contains {
                        ref fobs::WAKE_LEASE_ITEM_NAME: wake_lease_name,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_AWAITING_SATISFACTION,
                    }
                }
            },
        }
    );

    // Drop the lease BEFORE responding to BeforeSuspend.
    drop(wake_lease);

    // Verify that the wake lease is no longer registered with SAG.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                active_count: 0u64,
            },
        }
    );

    // Now respond to BeforeSuspend.
    respond_before_suspend_tx.unbounded_send(()).unwrap();

    // Verify that SAG proceeds to suspend because the lease was dropped before BeforeSuspend completed.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    // Verify AfterResume is not called.
    assert!(after_resume_received_rx.next().now_or_never().is_none());

    // Verify all expected events are registered.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::SUSPEND_EVENTS_NODE: contains {
                "9": contains {
                    ref fobs::WAKE_LEASE_ITEM_NAME: wake_lease_name,
                    ref fobs::WAKE_LEASE_CREATED_AT: AnyProperty,
                },
                "10": contains {
                    ref fobs::WAKE_LEASE_ITEM_NAME: wake_lease_name,
                    ref fobs::WAKE_LEASE_DROPPED_AT: AnyProperty,
                },
            },
        }
    );

    Ok(())
}

#[test_case(false, "test-blocker"; "reboot_disabled")]
#[test_case(true, "test-blocker-reboot"; "reboot_enabled")]
#[fuchsia::test]
async fn test_activity_governor_reports_on_suspend_blocker_stall(
    reboot_on_stalled_suspend_blocker: bool,
    blocker_name: &str,
) -> Result<()> {
    let (realm, _) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        stuck_warning_timeout_seconds: Some(1),
        reboot_on_stalled_suspend_blocker: Some(reboot_on_stalled_suspend_blocker),
        ..Default::default()
    })
    .await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let (client_end, mut stream) =
        fidl::endpoints::create_request_stream::<fsystem::SuspendBlockerMarker>();
    let (tx, _rx) = futures::channel::mpsc::unbounded();

    fasync::Task::local(async move {
        while let Some(request) = stream.next().await {
            match request {
                Ok(fsystem::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    log::info!("Fake suspend blocker received BeforeSuspend. Stalling...");
                    tx.unbounded_send(responder).unwrap();
                }
                Ok(fsystem::SuspendBlockerRequest::AfterResume { responder }) => {
                    responder.send().unwrap();
                }
                _ => {
                    log::warn!("Received unknown request");
                }
            }
        }
    })
    .detach();

    let token = activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(client_end),
            name: Some(blocker_name.to_string()),
            ..Default::default()
        })
        .await?
        .map_err(|e| anyhow::anyhow!("Register failed: {:?}", e))?;

    let suspend_controller = create_suspend_topology(&realm).await?;
    let suspend_lease_control = lease(&suspend_controller, 1).await?;

    let (terminal_client, mut terminal_stream) =
        fidl::endpoints::create_request_stream::<fstatecontrol::TerminalStateWatcherMarker>();
    let register_proxy =
        realm.connect_to_protocol::<fstatecontrol::ShutdownWatcherRegisterMarker>().await?;
    register_proxy.register_terminal_state_watcher(terminal_client).await.unwrap();

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear the dirty bit by calling watch_file once.
    let _ = querier.watch_file().await?;

    drop(token);
    drop(suspend_lease_control);

    let num_filed = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(60).after_now(), || {
            panic!("Timeout waiting for next watcher message.");
        })
        .await?;

    assert_eq!(num_filed, 1);

    if reboot_on_stalled_suspend_blocker {
        let req = terminal_stream.next().await.unwrap().unwrap();
        match req {
            fstatecontrol::TerminalStateWatcherRequest::OnTerminalStateTransitionStarted {
                responder,
            } => {
                responder.send().unwrap();
            }
            _ => panic!("Unexpected request"),
        }
    }

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_files_crash_report_on_normal_wake_lease() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        long_wake_lease_timeout_seconds: Some(1),
        ..Default::default()
    })
    .await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear the dirty bit by calling watch_file once.
    let _ = querier.watch_file().await?;

    let token = activity_governor
        .acquire_wake_lease("test-normal-lease")
        .await?
        .map_err(|e| anyhow::anyhow!("Acquire failed: {:?}", e))?;

    // We give 29 extra seconds to avoid flakes.
    let num_filed = querier
        .watch_file()
        .on_timeout(CRASH_REPORT_TIMEOUT.after_now(), || {
            panic!("Timeout waiting for next watcher message.");
        })
        .await?;

    assert_eq!(num_filed, 1);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            long_wake_leases: contains {
                occurrences: contains {
                    "test-normal-lease": 1u64,
                },
                latest_long_wake_leases: contains {
                    "0": contains {
                        name: "test-normal-lease",
                        start_time_ns: AnyProperty,
                    }
                }
            }
        }
    );

    drop(token);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            long_wake_leases: contains {
                latest_long_wake_leases: contains {
                    "0": contains {
                        name: "test-normal-lease",
                        start_time_ns: AnyProperty,
                        duration_ns: AtLeastIntProperty(1_000_000_000),
                    }
                }
            }
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_rates_limit_long_wake_lease_crash_reports() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        long_wake_lease_timeout_seconds: Some(1),
        ..Default::default()
    })
    .await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear the dirty bit by calling watch_file once.
    let _ = querier.watch_file().await?;

    // Acquire lease A twice, verifying that a crash report is filed each time.
    for i in 1..=2 {
        let token = activity_governor
            .acquire_wake_lease("test-lease-a")
            .await?
            .map_err(|e| anyhow::anyhow!("Acquire failed: {:?}", e))?;

        let num_filed = querier
            .watch_file()
            .on_timeout(CRASH_REPORT_TIMEOUT.after_now(), || {
                panic!("Timeout waiting for crash report {} for lease A.", i);
            })
            .await?;

        assert_eq!(num_filed, i as u64);
        drop(token);
    }

    // Acquire lease A the third time. It should be suppressed.
    let token3 = activity_governor
        .acquire_wake_lease("test-lease-a")
        .await?
        .map_err(|e| anyhow::anyhow!("Acquire failed: {:?}", e))?;

    // Verify NO new reports were filed for lease A.
    let res = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(3).after_now(), || Ok(0))
        .await;

    assert_eq!(res.unwrap(), 0);
    drop(token3);

    // Acquire lease B once. It should NOT be suppressed because it has a different name.
    let token_b = activity_governor
        .acquire_wake_lease("test-lease-b")
        .await?
        .map_err(|e| anyhow::anyhow!("Acquire failed: {:?}", e))?;

    let num_filed = querier
        .watch_file()
        .on_timeout(CRASH_REPORT_TIMEOUT.after_now(), || {
            panic!("Timeout waiting for crash report for lease B.");
        })
        .await?;

    assert_eq!(num_filed, 3);

    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            long_wake_leases: contains {
                occurrences: contains {
                    "test-lease-a": 3u64,
                    "test-lease-b": 1u64,
                },
                latest_long_wake_leases: contains {
                    "0": contains {
                        name: "test-lease-a",
                        start_time_ns: AnyProperty,
                    },
                    "1": contains {
                        name: "test-lease-a",
                        start_time_ns: AnyProperty,
                    },
                    "2": contains {
                        name: "test-lease-a",
                        start_time_ns: AnyProperty,
                    },
                    "3": contains {
                        name: "test-lease-b",
                        start_time_ns: AnyProperty,
                    }
                }
            }
        }
    );

    drop(token_b);

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_does_not_file_crash_report_on_explicit_unmonitored_wake_lease()
-> Result<()> {
    let (realm, _) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        long_wake_lease_timeout_seconds: Some(1),
        ..Default::default()
    })
    .await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear the dirty bit by calling watch_file once.
    let _ = querier.watch_file().await?;

    let token = activity_governor
        .acquire_unmonitored_wake_lease("test-long-lease")
        .await?
        .map_err(|e| anyhow::anyhow!("Acquire failed: {:?}", e))?;

    // Verify NO reports were filed
    let mut watch_fut = Box::pin(querier.watch_file().fuse());
    let mut timeout_fut =
        Box::pin(fasync::Timer::new(fasync::MonotonicDuration::from_seconds(5).after_now()).fuse());

    let filed = futures::select! {
        _num = watch_fut => true,
        () = timeout_fut => false,
    };

    assert!(!filed);

    drop(token);

    Ok(())
}

#[fuchsia::test]
async fn test_unmonitored_lease_reset() -> Result<()> {
    // Set timeout to 5 seconds via RealmOptions
    let (realm, _activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        long_wake_lease_timeout_seconds: Some(5),
        ..Default::default()
    })
    .await?;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear initial state if any
    let _ = querier.watch_file().await?;

    // 1. Acquire a regular wake lease
    log::info!("Before acquire_wake_lease regular");
    let regular_lease = activity_governor.acquire_wake_lease("regular_lease").await?;
    log::info!("After acquire_wake_lease regular");

    // 2. Acquire an unmonitored lease ("arbitrary_unmonitored_lease")
    log::info!("Before acquire_unmonitored_wake_lease");
    let unmonitored_lease = activity_governor
        .acquire_unmonitored_wake_lease("arbitrary_unmonitored_lease")
        .await
        .unwrap()
        .unwrap();
    log::info!("After acquire_unmonitored_wake_lease");

    // Hold unmonitored lease until timeout.
    // Timeout is 5s, so if it didn't reset, it would have fired by another 4 seconds.
    // We wait for 6 seconds to check no report.
    let res = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(6).after_now(), || Ok(0))
        .await;

    // We expect timeout (returning Ok(0)), meaning NO report was filed!
    assert_eq!(res.unwrap(), 0);

    // 3. Drop unmonitored lease
    drop(unmonitored_lease);

    // Wait for timeout again (5s). We wait 30s to be sure.
    let num_filed = querier
        .watch_file()
        .on_timeout(CRASH_REPORT_TIMEOUT.after_now(), || {
            panic!("Timeout waiting for crash report after drop.");
        })
        .await?;

    assert_eq!(num_filed, 1);

    // Cleanup regular lease
    drop(regular_lease);

    Ok(())
}

#[fuchsia::test]
async fn test_long_wake_lease_detector_does_not_trigger_on_normal_drop() -> Result<()> {
    let (realm, _) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        long_wake_lease_timeout_seconds: Some(1),
        ..Default::default()
    })
    .await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear initial state
    let _ = querier.watch_file().await?;

    // Acquire a lease
    let wake_lease = activity_governor.acquire_wake_lease("short-lease").await?.unwrap();
    assert!(!wake_lease.is_invalid());

    // Drop it immediately
    drop(wake_lease);

    // Wait for 5 seconds to prove no file reports were triggered!
    let mut watch_fut = Box::pin(querier.watch_file().fuse());
    let mut timeout_fut =
        Box::pin(fasync::Timer::new(fasync::MonotonicDuration::from_seconds(5).after_now()).fuse());

    let filed = futures::select! {
        _num = watch_fut => true,
        () = timeout_fut => false,
    };

    assert!(!filed);

    Ok(())
}

#[fuchsia::test]
async fn test_regular_lease_during_policy_lease() -> Result<()> {
    // Set timeout to 1 second via RealmOptions
    let (realm, _activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        long_wake_lease_timeout_seconds: Some(1),
        ..Default::default()
    })
    .await?;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    let () = boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");

    // Clear initial state
    let _ = querier.watch_file().await?;

    // 1. Acquire an ApplicationActivity lease (policy lease)
    log::info!("Before take_application_activity_lease");
    let app_activity_lease =
        activity_governor.take_application_activity_lease("test-app-activity").await?;
    log::info!("After take_application_activity_lease");

    // 2. Acquire a regular wake lease
    log::info!("Before acquire_wake_lease regular");
    let regular_lease = activity_governor
        .acquire_wake_lease("regular_lease")
        .await?
        .map_err(|e| anyhow::anyhow!("Acquire failed: {:?}", e))?;
    log::info!("After acquire_wake_lease regular");

    // Wait 5 seconds (timeout is 1s)
    // Since policy lease is active, expect NO report!
    let res = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(5).after_now(), || Ok(0))
        .await;
    assert_eq!(res.unwrap(), 0);

    // 3. Drop policy lease
    drop(app_activity_lease);

    // Expect report filed!
    let num_filed = querier
        .watch_file()
        .on_timeout(CRASH_REPORT_TIMEOUT.after_now(), || {
            panic!("Timeout waiting for crash report after policy lease drop.");
        })
        .await?;

    assert_eq!(num_filed, 1);

    // Cleanup
    drop(regular_lease);

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_wake_leases_before_and_after_sag_creation() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let (cpu_driver_controller, _cpu_driver_power_level, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();

    // Before registering the execution state dependency, verify cpu is level 1
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            power_elements: contains {
                cpu: {
                    power_level: 1u64,
                },
            },
        }
    );

    // Issue an acquire_wake_lease to ActivityGovernorRequestFrontend
    let wake_lease = activity_governor.acquire_wake_lease("early_lease").await?;

    // Call add_execution_state_dependency to trigger create_sag.
    cpu_element_manager
        .add_execution_state_dependency(
            fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                dependency_token: Some(cpu_driver_controller.assertive_dependency_token().unwrap()),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

    let server_token_koid =
        &wake_lease.as_ref().unwrap().basic_info().unwrap().related_koid.raw_koid().to_string();

    // Verify the inspect now contains the wake lease once dependencies are satisfied
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: {
                active_count: 1u64,
                oldest_active: {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease.as_ref().unwrap().koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: "early_lease",
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    }
                }
            },
        }
    );

    // Take a second wake lease
    let wake_lease2 = activity_governor.acquire_wake_lease("late_lease").await?;
    let server_token_koid2 =
        &wake_lease2.as_ref().unwrap().basic_info().unwrap().related_koid.raw_koid().to_string();

    // Verify both are present
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: {
                active_count: 2u64,
                oldest_active: {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease.as_ref().unwrap().koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: "early_lease",
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    },
                    var server_token_koid2: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease2.as_ref().unwrap().koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: "late_lease",
                        ref fobs::WAKE_LEASE_ITEM_ID: 1u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    }
                }
            },
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_application_activity_lease_before_sag_creation() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let (cpu_driver_controller, _cpu_driver_power_level, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();

    // Issue a take_application_activity_lease to ActivityGovernorRequestFrontend
    let app_lease = activity_governor.take_application_activity_lease("early_app_lease").await?;

    // Call add_execution_state_dependency to trigger create_sag.
    cpu_element_manager
        .add_execution_state_dependency(
            fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                dependency_token: Some(cpu_driver_controller.assertive_dependency_token().unwrap()),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

    let server_token_koid = &app_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    // Verify the inspect now contains the application activity lease once dependencies are satisfied
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: {
                active_count: 1u64,
                oldest_active: {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: app_lease.koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: "early_app_lease",
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                        "is_unmonitored_lease": true,
                    }
                }
            }
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_early_lease_dropped_before_dependency_registration() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        wait_for_suspending_token: Some(true),
        ..Default::default()
    })
    .await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let cpu_element_manager =
        realm.connect_to_protocol::<fsystem::CpuElementManagerMarker>().await?;
    let (cpu_driver_controller, _cpu_driver_power_level, _cpu_driver_task) =
        create_cpu_driver_topology(&realm).await.unwrap();

    // Acquire two wake leases
    let wake_lease1 = activity_governor.acquire_wake_lease("lease_retained").await?.unwrap();
    let wake_lease2 = activity_governor.acquire_wake_lease("lease_dropped").await?.unwrap();

    assert!(!wake_lease1.is_invalid());
    assert!(!wake_lease2.is_invalid());

    // Drop the second lease before registering execution state dependency
    drop(wake_lease2);

    // Call add_execution_state_dependency to trigger create_sag.
    cpu_element_manager
        .add_execution_state_dependency(
            fsystem::CpuElementManagerAddExecutionStateDependencyRequest {
                dependency_token: Some(cpu_driver_controller.assertive_dependency_token().unwrap()),
                power_level: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

    let server_token_koid = &wake_lease1.basic_info().unwrap().related_koid.raw_koid().to_string();

    // Verify the inspect contains ONLY the retained wake lease
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: {
                active_count: 1u64,
                oldest_active: {
                    var server_token_koid: {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: NonZeroUintProperty,
                        ref fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID: wake_lease1.koid().unwrap().raw_koid(),
                        ref fobs::WAKE_LEASE_ITEM_NAME: "lease_retained",
                        ref fobs::WAKE_LEASE_ITEM_ID: 0u64,
                        ref fobs::WAKE_LEASE_ITEM_TYPE: AnyStringProperty,
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    }
                }
            }
        }
    );

    Ok(())
}

#[fuchsia::test]
async fn test_no_suspend_loop_files_report() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm_ext(ftest::RealmOptions {
        use_suspender: Some(true),
        stuck_warning_timeout_seconds: Some(1),
        ..Default::default()
    })
    .await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;
    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let _suspend_controller = create_suspend_topology(&realm).await?;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await?
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: std::collections::HashMap<String, fbroker::StatusProxy> =
        element_info_provider
            .get_status_endpoints()
            .await?
            .unwrap()
            .into_iter()
            .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
            .collect();

    let aa_status = status_endpoints.get("application_activity").unwrap();

    // Block suspends by holding a wake lease.
    let wake_lease =
        activity_governor.acquire_wake_lease("prevent-suspend").await.unwrap().unwrap();

    let prevent_suspend_koid = wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();
    let prevent_suspend_koid_str = prevent_suspend_koid.as_str();

    // Wait for the wake lease to be fully satisfied in Power Broker before allowing
    // the boot complete signal to trigger transitions.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: {
                active_count: 1u64,
                oldest_active: {
                    var prevent_suspend_koid_str: contains {
                        ref fobs::WAKE_LEASE_ITEM_NAME: "prevent-suspend",
                        ref fobs::WAKE_LEASE_ITEM_STATUS: fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
                    }
                }
            },
        }
    );

    // Trigger "boot complete" signal to allow element transitions.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Clear initial state
    let _ = querier.watch_file().await?;

    // Cycle Application Activity 6 times.
    // This is because a crash report is filed on the 6th lease taken after a resume, which
    // corresponds to 5 complete cycles of dropping and retaking the application activity lease.
    for _ in 0..6 {
        let lease = activity_governor.take_application_activity_lease("cycle-lease").await?;

        // Wait for Active
        while aa_status.watch_power_level().await?.unwrap() != 1 {}

        drop(lease);

        // Wait for Inactive
        while aa_status.watch_power_level().await?.unwrap() != 0 {}

        // Wait for the lease node to disappear from Inspect.
        block_until_inspect_matches!(
            activity_governor_moniker,
            root: contains {
                ref fobs::WAKE_LEASES_NODE: {
                    active_count: 1u64,
                    oldest_active: {
                        var prevent_suspend_koid_str: contains {
                            ref fobs::WAKE_LEASE_ITEM_NAME: "prevent-suspend",
                        }
                    }
                },
            }
        );
    }

    // Verify crash report.
    // The timeout is set to 55 seconds, which is shorter than the 60-second
    // long wake lease watchdog timeout. This ensures that if the loop detector
    // fails to trigger, the test fails on this timeout instead of passing
    // due to a watchdog crash report.
    let mut watch_fut = Box::pin(querier.watch_file().fuse());
    let mut timeout_fut = Box::pin(
        fasync::Timer::new(fasync::MonotonicDuration::from_seconds(55).after_now()).fuse(),
    );

    let num_filed = futures::select! {
        num = watch_fut => num?,
        _ = timeout_fut => return Err(anyhow::anyhow!("Timeout waiting for crash report")),
    };

    assert_eq!(num_filed, 1);

    // Cycle Application Activity again 6 times to verify no new report is filed.
    for _ in 0..6 {
        let lease = activity_governor.take_application_activity_lease("cycle-lease").await?;
        while aa_status.watch_power_level().await?.unwrap() != 1 {}
        drop(lease);
        while aa_status.watch_power_level().await?.unwrap() != 0 {}

        block_until_inspect_matches!(
            activity_governor_moniker,
            root: contains {
                ref fobs::WAKE_LEASES_NODE: {
                    active_count: 1u64,
                    oldest_active: {
                        var prevent_suspend_koid_str: contains {
                            ref fobs::WAKE_LEASE_ITEM_NAME: "prevent-suspend",
                        }
                    }
                },
            }
        );
    }

    // Verify NO new crash report is filed
    let mut watch_fut = Box::pin(querier.watch_file().fuse());
    let mut timeout_fut2 =
        Box::pin(fasync::Timer::new(fasync::MonotonicDuration::from_seconds(1).after_now()).fuse());

    futures::select! {
        num = watch_fut => return Err(anyhow::anyhow!("Unexpected crash report filed: {}", num?)),
        _ = timeout_fut2 => {
            // Timeout is expected!
            log::info!("No new crash report filed as expected.");
        },
    };

    // Drop the wake lease to allow suspend.
    drop(wake_lease);

    // Wait for suspend to be attempted.
    assert_eq!(0, suspend_device.await_suspend().await.unwrap().unwrap().state_index.unwrap());

    // Resume system.
    suspend_device
        .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
            suspend_duration: Some(1i64),
            suspend_overhead: Some(1i64),
            ..Default::default()
        }))
        .await
        .unwrap()
        .unwrap();

    // Take a new wake lease to prevent suspension while we cycle AA.
    // We must do this BEFORE waiting for Inspect to ensure we keep SAG awake,
    // avoiding a race where the 100ms resume_control lease expires, causing SAG
    // to attempt a second suspend (which would deadlock as the test is not watching).
    let wake_lease =
        activity_governor.acquire_wake_lease("prevent-suspend-again").await.unwrap().unwrap();

    let prevent_suspend_again_koid =
        wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();
    let prevent_suspend_again_koid_str = prevent_suspend_again_koid.as_str();

    // Wait for SAG to fully process the resume to ensure the lease counter is reset.
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            suspend_stats: contains {
                ref fobs::SUSPEND_SUCCESS_COUNT: 1u64,
            }
        }
    );

    // Cycle Application Activity again 6 times.
    for _ in 0..6 {
        let lease = activity_governor.take_application_activity_lease("cycle-lease-again").await?;
        while aa_status.watch_power_level().await?.unwrap() != 1 {}
        drop(lease);
        while aa_status.watch_power_level().await?.unwrap() != 0 {}

        block_until_inspect_matches!(
            activity_governor_moniker,
            root: contains {
                ref fobs::WAKE_LEASES_NODE: {
                    active_count: 1u64,
                    oldest_active: {
                        var prevent_suspend_again_koid_str: contains {
                            ref fobs::WAKE_LEASE_ITEM_NAME: "prevent-suspend-again",
                        }
                    }
                },
            }
        );
    }

    // Verify a NEW crash report is filed
    let mut timeout_fut3 = Box::pin(
        fasync::Timer::new(fasync::MonotonicDuration::from_seconds(55).after_now()).fuse(),
    );

    let num_filed = futures::select! {
        num = watch_fut => num?,
        _ = timeout_fut3 => return Err(anyhow::anyhow!("Timeout waiting for NEW crash report")),
    };

    assert_eq!(num_filed, 2);

    drop(wake_lease);
    Ok(())
}

#[fuchsia::test]
async fn test_activity_governor_limits_wake_leases_node() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let element_info_provider = realm
        .connect_to_service_instance::<fbroker::ElementInfoProviderServiceMarker>(
            &"system_activity_governor",
        )
        .await
        .expect("failed to connect to service ElementInfoProviderService")
        .connect_to_status_provider()
        .expect("failed to connect to protocol ElementInfoProvider");

    let status_endpoints: HashMap<String, fbroker::StatusProxy> = element_info_provider
        .get_status_endpoints()
        .await?
        .unwrap()
        .into_iter()
        .map(|s| (s.identifier.unwrap(), s.status.unwrap().into_proxy()))
        .collect();

    let es_status = status_endpoints.get("execution_state").unwrap();
    assert_eq!(es_status.watch_power_level().await?.unwrap(), 2);

    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let max_leases_to_log = inspect
        .get_property_by_path(&["config", "max_active_wake_leases_to_log"])
        .unwrap()
        .uint()
        .unwrap() as usize;

    let mut leases = Vec::new();
    for i in 0..max_leases_to_log + 5 {
        let wake_lease = activity_governor
            .acquire_wake_lease(&format!("wake_lease_{i}"))
            .await
            .unwrap()
            .unwrap();
        leases.push(wake_lease);
    }

    // Trigger "boot complete" signal.
    {
        let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
        let () =
            boot_control.set_boot_complete().await.expect("SetBootComplete should have succeeded");
    }

    // Block until the final retained wake lease (lease 9) is fully populated in Inspect.
    let last_logged_koid =
        &leases[max_leases_to_log - 1].basic_info().unwrap().related_koid.raw_koid().to_string();
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    var last_logged_koid: contains {
                        ref fobs::WAKE_LEASE_ITEM_NAME: format!("wake_lease_{}", max_leases_to_log - 1),
                    }
                }
            }
        }
    );

    // Verify that the 10 oldest leases are in the WAKE_LEASES_NODE Inspect property.
    let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let mut root_assertion = TreeAssertion::new("root", false);
    let mut wake_leases_child = TreeAssertion::new("wake_leases", true);
    let mut oldest_active_child = TreeAssertion::new("oldest_active", true);

    for i in 0..max_leases_to_log {
        let server_token_koid =
            &leases[i].basic_info().unwrap().related_koid.raw_koid().to_string();
        let mut wake_lease_child = TreeAssertion::new(server_token_koid, false);
        wake_lease_child.add_property_assertion(
            fobs::WAKE_LEASE_ITEM_NAME,
            Arc::new(format!("wake_lease_{i}")),
        );
        oldest_active_child.add_child_assertion(wake_lease_child);
    }
    wake_leases_child.add_child_assertion(oldest_active_child);
    wake_leases_child.add_property_assertion("active_count", Arc::new(15u64));
    root_assertion.add_child_assertion(wake_leases_child);

    root_assertion
        .run(&inspect)
        .expect("WAKE_LEASES_NODE should contain exactly the 10 oldest wake leases");

    for i in 0..5 {
        let next_lease_idx = max_leases_to_log + i;
        let _ = leases.remove(0);

        let expected_active_count = (max_leases_to_log + 5 - i - 1) as u64;
        block_until_inspect_matches!(
            activity_governor_moniker,
            root: contains {
                ref fobs::WAKE_LEASES_NODE: contains {
                    active_count: expected_active_count,
                }
            }
        );

        let inspect = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
        let mut root_assertion = TreeAssertion::new("root", false);
        let mut wake_leases_child = TreeAssertion::new("wake_leases", true);
        let mut oldest_active_child = TreeAssertion::new("oldest_active", true);

        for i in 0..max_leases_to_log {
            let server_token_koid =
                &leases[i].basic_info().unwrap().related_koid.raw_koid().to_string();
            let name_idx = next_lease_idx - (max_leases_to_log - 1) + i;
            let mut wake_lease_child = TreeAssertion::new(server_token_koid, false);
            wake_lease_child.add_property_assertion(
                fobs::WAKE_LEASE_ITEM_NAME,
                Arc::new(format!("wake_lease_{name_idx}")),
            );
            oldest_active_child.add_child_assertion(wake_lease_child);
        }
        wake_leases_child.add_child_assertion(oldest_active_child);
        wake_leases_child.add_property_assertion(
            "active_count",
            Arc::new((max_leases_to_log + 5 - i - 1) as u64),
        );
        root_assertion.add_child_assertion(wake_leases_child);

        root_assertion.run(&inspect).unwrap_or_else(|_| {
            panic!(
                "WAKE_LEASES_NODE should contain exactly the {} oldest wake leases",
                max_leases_to_log
            )
        });
    }

    Ok(())
}

#[fuchsia::test]
async fn test_suspend_loop_files_report() -> Result<()> {
    let max_attempts = 10;
    let (realm, _activity_governor_moniker) =
        create_realm_ext(ftest::RealmOptions { use_suspender: Some(true), ..Default::default() })
            .await?;

    // Connect to the Fake Suspender and the Crash Querier
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    // Clear initial crash reporter state
    let _ = querier.watch_file().await?;

    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let blocker_name = "saboteur".to_string();
    let (blocker_client_end, mut blocker_stream) = fidl::endpoints::create_request_stream();
    activity_governor
        .register_suspend_blocker(fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(blocker_client_end),
            name: Some(blocker_name.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (before_suspend_tx, mut before_suspend_rx) = mpsc::channel(1);
    let (mut reply_tx, reply_rx) = mpsc::channel(1);
    let (mut drop_lease_tx, drop_lease_rx) = mpsc::channel(1);

    let activity_governor_clone = activity_governor.clone();
    fasync::Task::local(async move {
        let activity_governor = activity_governor_clone;
        let mut before_suspend_tx = before_suspend_tx;
        let mut reply_rx = reply_rx;
        let mut drop_lease_rx = drop_lease_rx;

        while let Some(Ok(req)) = blocker_stream.next().await {
            match req {
                fsystem::SuspendBlockerRequest::BeforeSuspend { responder } => {
                    // Take a wake lease to block suspend after this callback
                    let lease_token =
                        activity_governor.acquire_wake_lease("saboteur_wake_lease").await.unwrap();

                    // Signal that we took the lease
                    before_suspend_tx.try_send(()).unwrap();

                    // Wait for permission to reply
                    reply_rx.next().await.unwrap();
                    responder.send().unwrap();

                    // Wait for permission to drop lease
                    drop_lease_rx.next().await.unwrap();
                    drop(lease_token);
                }
                fsystem::SuspendBlockerRequest::AfterResume { responder, .. } => {
                    responder.send().unwrap();
                }
                fsystem::SuspendBlockerRequest::_UnknownMethod { ordinal, .. } => {
                    panic!("Unexpected method: {}", ordinal);
                }
            }
        }
    })
    .detach();

    // Trigger "boot complete" so SAG is allowed to suspend
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    boot_control.set_boot_complete().await.expect("SetBootComplete failed");

    // Drive the loop 'max_attempts' times using the Saboteur
    for i in 0..max_attempts {
        // Wait for SAG to reach BeforeSuspend
        before_suspend_rx.next().await.unwrap();

        // Tell handler to reply to BeforeSuspend
        reply_tx.try_send(()).unwrap();

        // Wait for SAG to fail suspend (it returns NotAllowed and doesn't call fake suspender)
        // We expect await_suspend to time out!
        let await_res = suspend_device
            .await_suspend()
            .on_timeout(fasync::MonotonicDuration::from_millis(500).after_now(), || {
                Ok(Err(zx::Status::TIMED_OUT.into_raw())) // Return error on timeout
            })
            .await;

        assert!(await_res.is_err() || await_res.as_ref().unwrap().is_err());

        // Tell handler to drop the lease so SAG can try again
        drop_lease_tx.try_send(()).unwrap();

        log::info!("Completed cycle {}/{}", i + 1, max_attempts);
    }

    // Verify the crash report was filed
    let num_filed = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(60).after_now(), || {
            panic!("Timeout waiting for crash report. Loop detector failed?");
        })
        .await?;

    assert_eq!(num_filed, 1);

    log::info!("First report verified. Starting second loop to verify rate limit.");

    // Run another cycle of 'max_attempts' failures.
    for i in 0..max_attempts {
        // Wait for SAG to reach BeforeSuspend
        before_suspend_rx.next().await.unwrap();

        // Tell handler to reply to BeforeSuspend
        reply_tx.try_send(()).unwrap();

        // Wait for SAG to fail suspend
        let await_res = suspend_device
            .await_suspend()
            .on_timeout(fasync::MonotonicDuration::from_millis(500).after_now(), || {
                Ok(Err(zx::Status::TIMED_OUT.into_raw()))
            })
            .await;

        assert!(await_res.is_err() || await_res.as_ref().unwrap().is_err());

        // Tell handler to drop the lease
        drop_lease_tx.try_send(()).unwrap();

        log::info!("Completed second loop cycle {}/{}", i + 1, max_attempts);
    }

    // Verify that NO crash report was filed because of rate limit.
    // watch_file should block, so we expect a timeout!
    let watch_result = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(1).after_now(), || {
            Ok(0) // Return 0 on timeout to indicate no report filed!
        })
        .await;

    assert_eq!(watch_result.unwrap(), 0);
    log::info!("Rate limiting verified successfully (no second report).");

    Ok(())
}

#[fuchsia::test]
async fn test_suspend_success_no_report() -> Result<()> {
    let max_attempts = 10;
    let (realm, _activity_governor_moniker) =
        create_realm_ext(ftest::RealmOptions { use_suspender: Some(true), ..Default::default() })
            .await?;

    // Connect to the Fake Suspender and the Crash Querier
    let suspend_device = realm.connect_to_protocol::<tsc::DeviceMarker>().await?;
    set_up_default_suspender(&suspend_device).await;

    let querier = realm
        .connect_to_protocol::<fidl_fuchsia_feedback_testing::FakeCrashReporterQuerierMarker>()
        .await?;

    // Clear initial crash reporter state
    let _ = querier.watch_file().await?;

    // Trigger "boot complete" so SAG is allowed to suspend
    let boot_control = realm.connect_to_protocol::<fsystem::BootControlMarker>().await?;
    boot_control.set_boot_complete().await.expect("SetBootComplete failed");

    // Drive the loop 'max_attempts' times
    for i in 0..max_attempts {
        // Expect success on await_suspend!
        let res = suspend_device.await_suspend().await.unwrap().unwrap();
        assert_eq!(0, res.state_index.unwrap());

        // Complete the suspend cycle by calling Resume
        suspend_device
            .resume(&tsc::DeviceResumeRequest::Result(tsc::SuspendResult {
                suspend_duration: Some(1000),
                suspend_overhead: Some(100),
                reason: Some(fhsuspend::WakeReason {
                    wake_vectors: Some(vec![]),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .unwrap()
            .unwrap();

        log::info!("Completed success cycle {}/{}", i + 1, max_attempts);
    }

    // Verify that NO crash report was filed.
    let watch_result = querier
        .watch_file()
        .on_timeout(fasync::MonotonicDuration::from_seconds(1).after_now(), || {
            Ok(0) // Return 0 on timeout to indicate no report filed!
        })
        .await;

    assert_eq!(watch_result.unwrap(), 0);
    log::info!("Verified NO report filed after successes.");

    Ok(())
}

// Ensures that a wake lease's creation timestamp (WAKE_LEASE_ITEM_NODE_CREATED_AT) remains constant
// across multiple Inspect snapshots. In b/522800734, the timestamp was evaluated lazily on each
// snapshot poll, causing the reported value to incorrectly advance.
#[fuchsia::test]
async fn test_activity_governor_wake_lease_created_at_time_does_not_change() -> Result<()> {
    let (realm, activity_governor_moniker) = create_realm().await?;
    let activity_governor = realm.connect_to_protocol::<fsystem::ActivityGovernorMarker>().await?;

    let wake_lease_name = "test_lease_timestamp";
    let wake_lease = activity_governor.acquire_wake_lease(wake_lease_name).await.unwrap().unwrap();
    let server_token_koid = wake_lease.basic_info().unwrap().related_koid.raw_koid().to_string();

    // Wait until the wake lease appears in Inspect
    block_until_inspect_matches!(
        activity_governor_moniker,
        root: contains {
            ref fobs::WAKE_LEASES_NODE: contains {
                oldest_active: contains {
                    ref server_token_koid: contains {
                        ref fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT: AnyProperty,
                    }
                }
            }
        }
    );

    // Fetch the inspect data first time
    let inspect1 = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let created_at_1 = inspect1
        .get_property_by_path(&[
            fobs::WAKE_LEASES_NODE,
            "oldest_active",
            &server_token_koid,
            fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT,
        ])
        .expect("wake_lease_created_at property not found")
        .number_as_int()
        .expect("wake_lease_created_at is not a number");

    // Sleep for a short duration to let boot time advance
    fasync::Timer::new(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(100)))
        .await;

    // Fetch the inspect data second time
    let inspect2 = get_diagnostics_hierarchy_for(&activity_governor_moniker).await?;
    let created_at_2 = inspect2
        .get_property_by_path(&[
            fobs::WAKE_LEASES_NODE,
            "oldest_active",
            &server_token_koid,
            fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT,
        ])
        .expect("wake_lease_created_at property not found")
        .number_as_int()
        .expect("wake_lease_created_at is not a number");

    assert_eq!(created_at_1, created_at_2, "wake_lease_created_at changed between snapshots!");

    Ok(())
}
