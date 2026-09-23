// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_power_cpu as fcpu;
use power_manager_integration_test_lib::{TestEnv, TestEnvBuilder};

const CPU_MANAGER_CONFIG_PATH: &'static str =
    "/pkg/domain_controller_test/cpu_manager_node_config.json5";
const POWER_MANAGER_CONFIG_PATH: &'static str =
    "/pkg/domain_controller_test/power_manager_node_config.json5";

const DOMAIN_NAME: &'static str = "cluster0";
const DOMAIN_ID: u64 = 0;
const LOGICAL_CORE_IDS: &'static [u64] = &[0, 1, 2, 3];
const AVAILABLE_FREQUENCIES_HZ: &'static [u64] =
    &[20 * 10u64.pow(8), 15 * 10u64.pow(8), 15 * 10u64.pow(8)];

async fn create_test_env() -> TestEnv {
    TestEnvBuilder::new()
        .cpu_manager_node_config_path(CPU_MANAGER_CONFIG_PATH)
        .power_manager_node_config_path(POWER_MANAGER_CONFIG_PATH)
        .build()
        .await
}

#[fuchsia::test]
async fn test_list_domains() {
    let mut env = create_test_env().await;
    let domain_controller = env.connect_to_protocol::<fcpu::DomainControllerMarker>();
    let domains = domain_controller.list_domains().await.unwrap();

    assert_eq!(
        domains,
        vec![fcpu::DomainInfo {
            id: Some(DOMAIN_ID),
            core_ids: Some(LOGICAL_CORE_IDS.to_vec()),
            available_frequencies_hz: Some(AVAILABLE_FREQUENCIES_HZ.to_vec()),
            name: Some(DOMAIN_NAME.to_string()),
            ..Default::default()
        }]
    );

    env.destroy().await;
}

#[fuchsia::test]
async fn test_get_and_set_max_frequency() {
    let mut env = create_test_env().await;
    let domain_controller = env.connect_to_protocol::<fcpu::DomainControllerMarker>();
    assert_eq!(0, domain_controller.get_max_frequency(DOMAIN_ID).await.unwrap().unwrap());
    domain_controller.set_max_frequency(DOMAIN_ID, 1).await.unwrap().unwrap();
    assert_eq!(1, domain_controller.get_max_frequency(DOMAIN_ID).await.unwrap().unwrap());
    env.destroy().await;
}
