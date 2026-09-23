// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::{create_endpoints, create_proxy};
use fuchsia_component_test::RealmBuilder;
use power_framework_test_realm::{PowerFrameworkTestRealmBuilder, PowerFrameworkTestRealmInstance};

use {
    fidl_fuchsia_hardware_power_suspend as fhsuspend, fidl_fuchsia_power_broker as fbroker,
    fidl_fuchsia_power_suspend as fsuspend, fidl_fuchsia_power_system as fsystem,
    fidl_test_sagcontrol as ftsagcontrol, fidl_test_suspendcontrol as tsc,
};

#[fuchsia::test]
async fn test_connect_and_call_all_protocols() -> Result<()> {
    let realm_builder = RealmBuilder::new().await?;
    realm_builder.power_framework_test_realm_setup().await?;

    let realm = realm_builder.build().await?;

    let topology: fbroker::TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    let (_element_control, element_control_server_end) =
        create_proxy::<fbroker::ElementControlMarker>();
    let (element_runner_client_end, _element_runner_server_end) =
        create_endpoints::<fbroker::ElementRunnerMarker>();

    topology
        .add_element(fbroker::ElementSchema {
            element_name: Some("power-framework-test-realm-test-element".into()),
            initial_current_level: Some(0),
            valid_levels: Some(vec![0, 1]),
            dependencies: Some(vec![]),
            element_control: Some(element_control_server_end),
            element_runner: Some(element_runner_client_end),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let governor: fsystem::ActivityGovernorProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    let _ = governor.get_power_elements().await?;

    let stats: fsuspend::StatsProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    let _ = stats.watch().await?;

    let sag_state: ftsagcontrol::StateProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    let _ = sag_state.get().await?;

    let suspend_control: tsc::DeviceProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    suspend_control
        .set_suspend_states(&tsc::DeviceSetSuspendStatesRequest {
            suspend_states: Some(vec![fhsuspend::SuspendState {
                resume_latency: Some(100),
                ..Default::default()
            }]),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let suspender = realm.power_framework_test_realm_connect_to_suspender().await.unwrap();
    let suspend_states_resp = suspender.get_suspend_states().await.unwrap().unwrap();
    assert_eq!(1, suspend_states_resp.suspend_states.as_ref().map(|s| s.len()).unwrap());
    assert_eq!(100, suspend_states_resp.suspend_states.unwrap()[0].resume_latency.unwrap());

    Ok(())
}
