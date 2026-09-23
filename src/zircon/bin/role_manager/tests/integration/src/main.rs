// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use diagnostics_reader::ArchiveReader;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_scheduler::{
    Parameter, ParameterValue, RoleManagerGetProfileForRoleRequest,
    RoleManagerGetProfileForRoleResponse, RoleManagerMarker, RoleManagerSetRoleRequest,
    RoleManagerSetRoleResponse, RoleName, RoleTarget, RoleType,
};
use fidl_test_rolemanager as ftest;
use fuchsia_async::Timer;
use fuchsia_component::client::connect_to_protocol;
use realm_proxy_client::RealmProxyClient;

macro_rules! retry_if_none {
    ($expr:expr, $err_msg:expr, $last_failure_reason:ident, $timer:expr) => {
        match $expr {
            Some(val) => val,
            None => {
                $last_failure_reason = Some($err_msg.to_string());
                $timer.await;
                continue;
            }
        }
    };
    ($expr:expr, $err_msg:expr, $last_failure_reason:ident) => {
        retry_if_none!(
            $expr,
            $err_msg,
            $last_failure_reason,
            Timer::new(zx::MonotonicDuration::from_millis(500))
        )
    };
}

macro_rules! retry_if_not_equal {
    ($expr:expr, $expected:expr, $name:expr, $last_failure_reason:ident, $timer:expr) => {
        match $expr {
            Ok(actual) => {
                if actual != $expected {
                    $last_failure_reason = Some(format!(
                        "{}:\n\texpected: {}\n\tactual: {}",
                        $name, $expected, actual
                    ));
                    $timer.await;
                    continue;
                }
            }
            Err(e) => {
                $last_failure_reason = Some(format!("{}", e));
                $timer.await;
                continue;
            }
        }
    };
    ($expr:expr, $expected:expr, $name:expr, $last_failure_reason:ident) => {
        retry_if_not_equal!(
            $expr,
            $expected,
            $name,
            $last_failure_reason,
            Timer::new(zx::MonotonicDuration::from_millis(500))
        )
    };
}

async fn create_realm(options: ftest::RealmOptions) -> Result<RealmProxyClient> {
    let realm_factory = connect_to_protocol::<ftest::RealmFactoryMarker>()?;
    let (client, server) = create_endpoints();
    realm_factory
        .create_realm(options, server)
        .await?
        .map_err(realm_proxy_client::Error::OperationError)?;
    Ok(RealmProxyClient::from(client))
}

fn get_test_thread_handle() -> Result<zx::Thread> {
    fuchsia_runtime::with_thread_self(
        |thread| Ok(thread.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
    )
}

fn get_test_vmar_handle() -> Result<zx::Vmar> {
    Ok(fuchsia_runtime::vmar_root_self().duplicate_handle(zx::Rights::SAME_RIGHTS)?)
}

fn get_roles_count(
    roles: &diagnostics_reader::DiagnosticsHierarchy,
    role_name: &str,
) -> Result<u64> {
    match roles.get_child(role_name) {
        Some(n) => match n.get_property("request_count") {
            Some(p) => Ok(p.uint().unwrap()),
            None => anyhow::bail!(
                "Could not find \"request_count\" property for role \"{}\".",
                role_name
            ),
        },
        None => anyhow::bail!("Could not find \"{}\" child node.", role_name),
    }
}

async fn validate_inspect(
    expected_thread_request_count: u64,
    expected_memory_request_count: u64,
) -> Result<()> {
    // Give up on waiting for the Inspect update after this amount of tries.
    const MAX_LOOPS_COUNT: usize = 20;
    let mut last_failure_reason = None;

    // Collect inspect in a loop to avoid racing with role_manager's start or with Archivist on
    // subsequent snapshot requests.
    for _ in 0..MAX_LOOPS_COUNT {
        let data = ArchiveReader::inspect()
            .add_selector("test_realm_factory/realm_builder\\:*/role_manager:root")
            .snapshot()
            .await?
            .into_iter()
            .filter(|d| d.payload.is_some())
            .collect::<Vec<_>>();

        if let Some(inspect_data) =
            data.iter().find(|d| d.moniker.to_string().contains("role_manager"))
        {
            if let Some(payload) = &inspect_data.payload {
                // Verify that the deep properties we expect are present.
                let config_node = retry_if_none!(
                    payload.get_child("config"),
                    "Could not find \"config\" child node.",
                    last_failure_reason
                );

                // Verify thread roles exist and that the request count matches the expected value.
                let thread_roles = retry_if_none!(
                    config_node.get_child("thread_roles"),
                    "Could not find \"thread_roles\" child node.",
                    last_failure_reason
                );
                retry_if_not_equal!(
                    get_roles_count(thread_roles, "test.core.a"),
                    expected_thread_request_count,
                    "Thread request count",
                    last_failure_reason
                );

                let affinity_node = retry_if_none!(
                    thread_roles.get_child("test.core.affinity"),
                    "Could not find \"test.core.affinity\" child node.",
                    last_failure_reason
                );

                let affinity_prop = retry_if_none!(
                    affinity_node.get_property("affinity"),
                    "Could not find \"affinity\" property.",
                    last_failure_reason
                );

                // If we found the property, verify its value.
                assert_eq!(affinity_prop.string().unwrap(), "0x3");

                // Verify memory roles exist and that the request count matches the expected value.
                let memory_roles = retry_if_none!(
                    config_node.get_child("memory_roles"),
                    "Could not find \"memory_roles\" child node.",
                    last_failure_reason
                );
                retry_if_not_equal!(
                    get_roles_count(memory_roles, "test.core.a"),
                    expected_memory_request_count,
                    "Memory request count",
                    last_failure_reason
                );
            }
        }

        return Ok(());
    }

    anyhow::bail!(
        "\nInspect did not match after {} tries.\n\n{}",
        MAX_LOOPS_COUNT,
        last_failure_reason.unwrap_or_else(|| "No data found".to_string())
    );
}

#[fuchsia::test]
async fn test_set_role_thread() -> Result<()> {
    // Test that setting a basic role on a thread works.
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.a".to_string() }),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![Parameter {
                key: "set_role".to_string(),
                value: ParameterValue::StringValue("test.core.a".to_string())
            }]),
            ..Default::default()
        })
    );
    Ok(())
}

#[fuchsia::test]
async fn test_get_profile_for_role_thread() -> Result<()> {
    // Test that getting a profile for a thread role works.
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let request = RoleManagerGetProfileForRoleRequest {
        target: Some(RoleType::Task),
        role: Some(RoleName { role: "test.core.a".to_string() }),
        ..Default::default()
    };
    let response = role_manager.get_profile_for_role(request).await?;
    match response {
        Ok(RoleManagerGetProfileForRoleResponse {
            profile: Some(_),
            output_parameters: Some(params),
            ..
        }) => {
            assert_eq!(
                params,
                vec![Parameter {
                    key: "set_role".to_string(),
                    value: ParameterValue::StringValue("test.core.a".to_string())
                }]
            );
        }
        _ => panic!("Unexpected response: {:?}", response),
    }
    Ok(())
}

#[fuchsia::test]
async fn test_set_role_vmar() -> Result<()> {
    // Test that setting a basic role on a vmar works.
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Vmar(get_test_vmar_handle()?)),
        role: Some(RoleName { role: "test.core.mem.default".to_string() }),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![Parameter {
                key: "set_role".to_string(),
                value: ParameterValue::StringValue("test.core.mem.default".to_string())
            }]),
            ..Default::default()
        })
    );
    Ok(())
}

#[fuchsia::test]
async fn test_input_parameters() -> Result<()> {
    // Test that passing in input parameters selects the correct role.
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    // First, verify that not passing input parameters to a parameterized role fails.
    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.parameterized.role".to_string() }),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(response, Err(zx::sys::ZX_ERR_NOT_FOUND));

    // Next, verify that passing in input parameters selects the correct role. We do so by passing
    // in the same role twice with different input parameters and verify that we get the correct
    // output parameters back.
    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.parameterized.role".to_string() }),
        input_parameters: Some(vec![Parameter {
            key: "input".to_string(),
            value: ParameterValue::StringValue("foo".to_string()),
        }]),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![
                Parameter { key: "output1".to_string(), value: ParameterValue::IntValue(1) },
                Parameter { key: "output2".to_string(), value: ParameterValue::FloatValue(2.5) },
            ]),
            ..Default::default()
        })
    );
    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.parameterized.role".to_string() }),
        input_parameters: Some(vec![Parameter {
            key: "input".to_string(),
            value: ParameterValue::StringValue("bar".to_string()),
        }]),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![
                Parameter { key: "output1".to_string(), value: ParameterValue::IntValue(5) },
                Parameter { key: "output2".to_string(), value: ParameterValue::FloatValue(42.6) },
            ]),
            ..Default::default()
        })
    );

    // Finally, verify that the order of input parameters does not change the outcome.
    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.parameterized.role".to_string() }),
        input_parameters: Some(vec![
            Parameter {
                key: "param1".to_string(),
                value: ParameterValue::StringValue("foo".to_string()),
            },
            Parameter {
                key: "param2".to_string(),
                value: ParameterValue::StringValue("bar".to_string()),
            },
            Parameter {
                key: "param3".to_string(),
                value: ParameterValue::StringValue("baz".to_string()),
            },
        ]),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![
                Parameter { key: "output1".to_string(), value: ParameterValue::IntValue(489) },
                Parameter { key: "output2".to_string(), value: ParameterValue::FloatValue(297.5) },
                Parameter {
                    key: "output3".to_string(),
                    value: ParameterValue::StringValue("Hello, World!".to_string())
                },
            ]),
            ..Default::default()
        })
    );
    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.parameterized.role".to_string() }),
        input_parameters: Some(vec![
            Parameter {
                key: "param2".to_string(),
                value: ParameterValue::StringValue("bar".to_string()),
            },
            Parameter {
                key: "param3".to_string(),
                value: ParameterValue::StringValue("baz".to_string()),
            },
            Parameter {
                key: "param1".to_string(),
                value: ParameterValue::StringValue("foo".to_string()),
            },
        ]),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![
                Parameter { key: "output1".to_string(), value: ParameterValue::IntValue(489) },
                Parameter { key: "output2".to_string(), value: ParameterValue::FloatValue(297.5) },
                Parameter {
                    key: "output3".to_string(),
                    value: ParameterValue::StringValue("Hello, World!".to_string())
                },
            ]),
            ..Default::default()
        })
    );

    Ok(())
}

#[fuchsia::test]
async fn test_scope_overrides() -> Result<()> {
    // Test that product role configurations override core role configurations.
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "test.core.product".to_string() }),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(
        response,
        Ok(RoleManagerSetRoleResponse {
            output_parameters: Some(vec![
                Parameter {
                    key: "set_role".to_string(),
                    value: ParameterValue::StringValue("test.core.product".to_string()),
                },
                Parameter {
                    key: "scope".to_string(),
                    value: ParameterValue::StringValue("product".to_string()),
                },
            ]),
            ..Default::default()
        })
    );

    Ok(())
}

#[fuchsia::test]
async fn test_nonexistent_role() -> Result<()> {
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "invalid_role".to_string() }),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(response, Err(zx::sys::ZX_ERR_NOT_FOUND));
    Ok(())
}

#[fuchsia::test]
async fn test_bad_config_extension() -> Result<()> {
    // Test that a profile from a file that does not have the extension `.profiles` was not
    // parsed and therefore does not exist.
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let request = RoleManagerSetRoleRequest {
        target: Some(RoleTarget::Thread(get_test_thread_handle()?)),
        role: Some(RoleName { role: "bad.extension.role".to_string() }),
        ..Default::default()
    };
    let response = role_manager.set_role(request).await?;
    assert_eq!(response, Err(zx::sys::ZX_ERR_NOT_FOUND));
    Ok(())
}

#[fuchsia::test]
async fn test_inspect_exposed() -> Result<()> {
    let realm_options = ftest::RealmOptions::default();
    let realm = create_realm(realm_options).await?;
    let role_manager = realm.connect_to_protocol::<RoleManagerMarker>().await?;

    let mut expected_thread_request_count: u64 = 0;
    let mut expected_memory_request_count: u64 = 0;

    // Validate that all expected properties exist on startup.
    validate_inspect(expected_thread_request_count, expected_memory_request_count).await.unwrap();

    // Issue both thread and memory GetProfileForRole() requests.
    for target in [RoleType::Task, RoleType::Memory] {
        let request = RoleManagerGetProfileForRoleRequest {
            target: Some(target),
            role: Some(RoleName { role: "test.core.a".to_string() }),
            ..Default::default()
        };
        let response = role_manager.get_profile_for_role(request).await?;
        assert!(response.is_ok());
    }

    // Take new snapshot and check that both counters increased by 1.
    expected_thread_request_count += 1;
    expected_memory_request_count += 1;
    validate_inspect(expected_thread_request_count, expected_memory_request_count).await.unwrap();

    // Do the same with the SetRole() request.
    for target in
        [RoleTarget::Thread(get_test_thread_handle()?), RoleTarget::Vmar(get_test_vmar_handle()?)]
    {
        let request = RoleManagerSetRoleRequest {
            target: Some(target),
            role: Some(RoleName { role: "test.core.a".to_string() }),
            ..Default::default()
        };
        let response = role_manager.set_role(request).await?;
        assert!(response.is_ok());
    }

    // Take new snapshot and check that both counters increased by 1.
    expected_thread_request_count += 1;
    expected_memory_request_count += 1;
    validate_inspect(expected_thread_request_count, expected_memory_request_count).await.unwrap();

    Ok(())
}
