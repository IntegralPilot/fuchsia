// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use assert_matches::assert_matches;
use fidl::endpoints::{Proxy, create_endpoints, create_proxy};
use fidl_fuchsia_power_broker::{
    self as fpb, BinaryPowerLevel, ElementSchema, LeaseStatus, LevelDependency, StatusMarker,
    TopologyMarker, TopologyProxy,
};
use fuchsia_async as fasync;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures_util::TryStreamExt;
use power_broker_client::BINARY_POWER_LEVELS;
use std::thread;
use std::time::Duration;

async fn build_power_broker_realm() -> Result<RealmInstance, Error> {
    let builder = RealmBuilder::new().await?;
    let power_broker = builder
        .add_child("power_broker", "power-broker#meta/power-broker.cm", ChildOptions::new())
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<TopologyMarker>())
                .from(&power_broker)
                .to(Ref::parent()),
        )
        .await?;
    let realm = builder.build().await?;
    Ok(realm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_power_broker::StatusProxy;
    use fpb::{
        ElementControlMarker, ElementRunnerMarker, ElementRunnerRequestStream, LessorMarker,
    };
    use futures_util::stream::TryNext;

    async fn handle_set_level(
        next: TryNext<'_, ElementRunnerRequestStream>,
    ) -> Result<(u8, fpb::ElementRunnerSetLevelResponder), Error> {
        if let Some(request) = next.await.unwrap() {
            match request {
                fpb::ElementRunnerRequest::SetLevel { level, responder } => {
                    return Ok((level, responder));
                }
                fidl_fuchsia_power_broker::ElementRunnerRequest::_UnknownMethod { .. } => {
                    return Err(Error::msg("ElementRunnerRequest::_UnknownMethod received"));
                }
            }
        } else {
            return Err(Error::msg("ElementRunnerRequest::_UnknownMethod received"));
        }
    }

    /// Verifies that the next ElementRunnerRequest matches the expected required level.
    /// Returns the ElementRunnerSetLevelResponder so that a response can be sent, confirming
    /// the current level.
    async fn assert_set_level_required_eq_and_return_responder(
        next: TryNext<'_, ElementRunnerRequestStream>,
        expect_required: u8,
    ) -> fpb::ElementRunnerSetLevelResponder {
        let (required, current) = handle_set_level(next).await.unwrap();
        assert_eq!(required, expect_required);
        current
    }

    /// Sends the ElementRunner::SetLevel response, then makes a Status::WatchPowerLevel call and
    /// asserts that it matches expect_level.
    async fn assert_send_response_updates_level_to(
        current: fpb::ElementRunnerSetLevelResponder,
        status: &StatusProxy,
        expect_level: u8,
    ) {
        current.send().expect("set_level resp failed");
        let current_level =
            status.watch_power_level().await.unwrap().expect("watch_power_level failed");
        assert_eq!(current_level, expect_level);
    }

    /// Verifies that attempting to add an element without providing an `element_runner` channel
    /// returns an `AddElementError::Invalid` error.
    #[fuchsia::test]
    fn test_element_runner_invalid() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;

        // Create a topology with only two elements and a single dependency:
        // P <- C
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
        let (_, element_control_server) = create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            // Not supplying element_runner is invalid.
            let invalid_resp = topology
                .add_element(ElementSchema {
                    element_name: Some("P".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    element_control: Some(element_control_server),
                    ..Default::default()
                })
                .await;
            assert_matches!(invalid_resp, Ok(Err(fpb::AddElementError::Invalid)));
        });
        Ok(())
    }

    /// Verifies direct parent-child dependency handling: leasing a child powers up its parent
    /// first, then powers up the child to satisfy the lease, and dropping the lease steps both down
    /// in reverse order.
    #[fuchsia::test]
    fn test_direct() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;

        // Create a topology with only two elements and a single dependency:
        // P <- C
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
        let parent_token = zx::Event::create();
        let (parent_element_runner_client, parent_element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut parent_element_runner = parent_element_runner_server.into_stream();
        let (parent_element_control, parent_element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("P".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        element_control: Some(parent_element_control_server),
                        element_runner: Some(parent_element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
            assert!(
                parent_element_control
                    .register_dependency_token(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    )
                    .await
                    .is_ok()
            );
        });
        let parent_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            parent_element_control.open_status_channel(server)?;
            client
        };
        let (child_element_runner_client, child_element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut child_element_runner = child_element_runner_server.into_stream();
        let (child_lessor, lessor_server) = create_proxy::<LessorMarker>();
        let (child_element_control, child_element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("C".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                            requires_token: Some(
                                parent_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![
                                BinaryPowerLevel::On.into_primitive()
                            ]),
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_server),
                        element_control: Some(child_element_control_server),
                        element_runner: Some(child_element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let child_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            child_element_control.open_status_channel(server)?;
            client
        };

        // Initial required level for P & C should be OFF.
        // Update P & C's current level to OFF with PowerBroker.
        executor.run_singlethreaded(async {
            let parent_current = assert_set_level_required_eq_and_return_responder(
                parent_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            let child_current = assert_set_level_required_eq_and_return_responder(
                child_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            parent_current.send().expect("set_level resp failed");
            child_current.send().expect("set_level resp failed");
            assert_eq!(
                parent_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
            assert_eq!(
                child_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
        });

        // Attempt to update with invalid level, this should fail.
        executor.run_singlethreaded(async {
            assert!(child_lessor.lease(100).await.unwrap().is_err());
        });

        // Acquire lease for C.
        // P's required level should become ON.
        // C's required level should remain OFF until P turns ON.
        // Lease should be pending.
        let lease = executor.run_singlethreaded(async {
            child_lessor
                .lease(BinaryPowerLevel::On.into_primitive())
                .await
                .unwrap()
                .expect("Lease response not ok")
                .into_proxy()
        });
        let parent_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                parent_element_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });
        let mut child_element_runner_next = child_element_runner.try_next();
        assert!(executor.run_until_stalled(&mut child_element_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Pending
            );
        });

        // Update P's current level to ON.
        // P's required level should remain ON.
        // C's required level should become ON.
        executor.run_singlethreaded(async {
            parent_current.send().expect("set_level resp failed");
            assert_eq!(
                parent_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::On.into_primitive())
            );
        });
        let mut parent_element_runner_next = parent_element_runner.try_next();
        assert!(executor.run_until_stalled(&mut parent_element_runner_next).is_pending());
        let child_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                child_element_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });

        // Update C's current level to ON.
        // Lease should become satisfied.
        executor.run_singlethreaded(async {
            child_current.send().expect("set_level resp failed");
            assert_eq!(
                child_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::On.into_primitive())
            );
            assert_eq!(
                lease.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Drop lease.
        // C's required level should become OFF.
        let child_current = executor.run_singlethreaded(async {
            drop(lease);
            assert_set_level_required_eq_and_return_responder(
                child_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await
        });

        // Update C's current level to OFF.
        // P's required level should become OFF.
        let parent_current = executor.run_singlethreaded(async {
            child_current.send().expect("set_level resp failed");
            assert_eq!(
                child_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
            assert_set_level_required_eq_and_return_responder(
                parent_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await
        });

        // Update P's current level to OFF.
        executor.run_singlethreaded(async {
            parent_current.send().expect("set_level resp failed");
            assert_eq!(
                parent_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
        });

        // Remove P's element. Status channel should be closed.
        executor.run_singlethreaded(async {
            drop(parent_element_control);
            let status_after_remove = parent_status.watch_power_level().await;
            assert_matches!(status_after_remove, Err(fidl::Error::ClientChannelClosed { .. }));
        });
        // Remove C's element. Status channel should be closed.
        executor.run_singlethreaded(async {
            drop(child_element_control);
            let status_after_remove = child_status.watch_power_level().await;
            assert_matches!(status_after_remove, Err(fidl::Error::ClientChannelClosed { .. }));
        });

        Ok(())
    }

    /// Verifies dynamically adding dependencies to an element: inactive dependencies take no
    /// immediate effect, while active dependencies power up the required element before completing.
    #[fuchsia::test]
    fn test_add_dependency() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;

        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // Create Required element.
        let required_token = zx::Event::create();
        let (required_runner_client, required_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut required_runner = required_runner_server.into_stream();
        let (required_control, required_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("Required".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1, 2]),
                    element_control: Some(required_control_server),
                    element_runner: Some(required_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();

            required_control
                .register_dependency_token(
                    required_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
        });

        let required_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            required_control.open_status_channel(server).unwrap();
            client
        };

        // Create Dependent element.
        let (dependent_runner_client, dependent_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut dependent_runner = dependent_runner_server.into_stream();
        let (dependent_lessor, dependent_lessor_server) = create_proxy::<LessorMarker>();
        let (dependent_control, dependent_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("Dependent".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1, 2]),
                    lessor_channel: Some(dependent_lessor_server),
                    element_control: Some(dependent_control_server),
                    element_runner: Some(dependent_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        let dependent_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            dependent_control.open_status_channel(server).unwrap();
            client
        };

        // Initialize levels to 0.
        executor.run_singlethreaded(async {
            let req_current =
                assert_set_level_required_eq_and_return_responder(required_runner.try_next(), 0)
                    .await;
            let dep_current =
                assert_set_level_required_eq_and_return_responder(dependent_runner.try_next(), 0)
                    .await;
            req_current.send().unwrap();
            dep_current.send().unwrap();

            assert_eq!(required_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(dependent_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // Lease Dependent at 1.
        let lease = executor.run_singlethreaded(async {
            let lease = dependent_lessor.lease(1).await.unwrap().unwrap().into_proxy();
            (assert_set_level_required_eq_and_return_responder(dependent_runner.try_next(), 1)
                .await)
                .send()
                .unwrap();
            assert_eq!(dependent_status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(
                lease.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
            lease
        });

        // The lease's status should remain Satisfied throughout this process.
        let mut lease_status_fut = Box::pin(lease.watch_status(LeaseStatus::Satisfied));

        // Case 1: Unused Dependency.
        // Add dependency that WILL NOT be used: Dependent at 2 requires Required at 2.
        // Since Dependent is only at 1, this should not raise Required's level from 0.
        executor.run_singlethreaded(async {
            dependent_control
                .add_dependency(fpb::LevelDependency {
                    dependent_level: Some(2),
                    requires_token: Some(
                        required_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    ),
                    requires_level_by_preference: Some(vec![2]),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        // Verify Required level is still 0.
        let mut required_runner_next = required_runner.try_next();
        assert!(executor.run_until_stalled(&mut required_runner_next).is_pending());

        // Verify lease status has not changed from Satisfied.
        assert!(executor.run_until_stalled(&mut lease_status_fut).is_pending());

        // Case 2: Used Dependency.
        // Now, add dependency that WILL be used: Dependent at 1 requires Required at 1.
        // Since Dependent is already at level 1, this dependency becomes ACTIVE immediately!
        let mut add_dep_fut = Box::pin(dependent_control.add_dependency(fpb::LevelDependency {
            dependent_level: Some(1),
            requires_token: Some(required_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
            requires_level_by_preference: Some(vec![1]),
            ..Default::default()
        }));

        // add_dependency call should be blocked because it is waiting for Required to go to 1.
        assert!(executor.run_until_stalled(&mut add_dep_fut).is_pending());

        // Verify Required increases to 1.
        executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(required_runner.try_next(), 1)
                .await
                .send()
                .unwrap();
            assert_eq!(required_status.watch_power_level().await.unwrap(), Ok(1));
        });

        // Now add_dep_fut should complete!
        executor.run_singlethreaded(async {
            add_dep_fut.await.unwrap().unwrap();
        });

        // Verify lease status has not changed from Satisfied.
        assert!(executor.run_until_stalled(&mut lease_status_fut).is_pending());

        Ok(())
    }

    /// Verifies that `Topology.Lease` blocks until the requested lease dependencies are satisfied
    /// when `should_return_pending_lease` is false.
    #[fuchsia::test]
    fn test_topology_lease_wait_for_satisfied() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        let (element_runner_client, element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_runner = element_runner_server.into_stream();
        let (element_control, element_control_server) = create_proxy::<ElementControlMarker>();
        let token = zx::Event::create();

        executor.run_singlethreaded(async {
            topology
                .add_element(fpb::ElementSchema {
                    element_name: Some("E".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(element_control_server),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();

            element_control
                .register_dependency_token(token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                .await
                .unwrap()
                .unwrap();
        });

        // Initialize level to 0
        executor.run_singlethreaded(async {
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 0)
                    .await;
            responder.send().unwrap();
        });

        let (lease_token_client, lease_token_server) = zx::EventPair::create();

        // Call Topology.Lease with should_return_pending_lease = false (default)
        // It should block until we update current level to 1.
        let mut lease_fut = Box::pin(topology.lease(fpb::LeaseSchema {
            lease_token: Some(lease_token_server),
            lease_name: Some("test_lease".into()),
            dependencies: Some(vec![fpb::LeaseDependency {
                requires_token: Some(token),
                requires_level: Some(1),
                ..Default::default()
            }]),
            ..Default::default()
        }));

        assert!(executor.run_until_stalled(&mut lease_fut).is_pending());

        // Now update current level to 1
        let responder = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 1).await
        });
        responder.send().unwrap();

        // Now the lease future should complete
        executor.run_singlethreaded(async {
            lease_fut.await.unwrap().unwrap();
        });

        executor.run_singlethreaded(async {
            fasync::OnSignals::new(
                &lease_token_client,
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
            )
            .await
            .unwrap();
        });

        Ok(())
    }

    /// Verifies that `Topology.Lease` returns immediately with a pending lease when
    /// `should_return_pending_lease` is true, and signals satisfaction once dependencies are met.
    #[fuchsia::test]
    fn test_topology_lease_return_pending() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        let (element_runner_client, element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_runner = element_runner_server.into_stream();
        let (element_control, element_control_server) = create_proxy::<ElementControlMarker>();
        let token = zx::Event::create();

        executor.run_singlethreaded(async {
            topology
                .add_element(fpb::ElementSchema {
                    element_name: Some("E".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(element_control_server),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();

            element_control
                .register_dependency_token(token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
                .await
                .unwrap()
                .unwrap();
        });

        // Initialize level to 0
        executor.run_singlethreaded(async {
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 0)
                    .await;
            responder.send().unwrap();
        });

        let (lease_token_client, lease_token_server) = zx::EventPair::create();

        // Call Topology.Lease with should_return_pending_lease = true
        // It should return immediately.
        executor.run_singlethreaded(async {
            topology
                .lease(fpb::LeaseSchema {
                    lease_token: Some(lease_token_server),
                    lease_name: Some("test_lease".into()),
                    dependencies: Some(vec![fpb::LeaseDependency {
                        requires_token: Some(token),
                        requires_level: Some(1),
                        ..Default::default()
                    }]),
                    should_return_pending_lease: Some(true),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        assert_matches!(
            lease_token_client.wait_one(
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
                zx::MonotonicInstant::INFINITE_PAST,
            ),
            zx::WaitResult::TimedOut(_)
        );

        // Now update current level to 1
        let responder = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 1).await
        });
        responder.send().unwrap();

        executor.run_singlethreaded(async {
            fasync::OnSignals::new(
                &lease_token_client,
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
            )
            .await
            .unwrap();
        });

        Ok(())
    }

    /// Verifies transitive dependency propagation across a multi-element chain (A <- B <- C):
    /// elements power up sequentially from root to leaf and step down in reverse order when the
    /// lease is dropped.
    #[fuchsia::test]
    fn test_transitive() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;

        // Create a four element topology with the following dependencies:
        // C depends on B, which in turn depends on A.
        // D has no dependencies or dependents.
        // A <- B <- C   D
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
        let element_a_token = zx::Event::create();
        let (element_a_runner_client, element_a_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_a_runner = element_a_runner_server.into_stream();
        let (element_a_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("A".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        element_control: Some(element_control_server),
                        element_runner: Some(element_a_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
            assert!(
                element_a_element_control
                    .register_dependency_token(
                        element_a_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    )
                    .await
                    .is_ok()
            );
        });
        let element_a_status = {
            let (client, server) = create_endpoints::<StatusMarker>();
            element_a_element_control.open_status_channel(server)?;
            client.into_proxy()
        };
        let element_b_token = zx::Event::create();
        let (element_b_runner_client, element_b_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_b_runner = element_b_runner_server.into_stream();
        let (element_b_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("B".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                            requires_token: Some(
                                element_a_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![
                                BinaryPowerLevel::On.into_primitive()
                            ]),
                            ..Default::default()
                        }]),
                        element_control: Some(element_control_server),
                        element_runner: Some(element_b_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
            assert!(
                element_b_element_control
                    .register_dependency_token(
                        element_b_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    )
                    .await
                    .is_ok()
            );
        });
        let element_b_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            element_b_element_control.open_status_channel(server)?;
            client.into_proxy()
        };
        let (element_c_runner_client, element_c_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_c_runner = element_c_runner_server.into_stream();
        let (element_c_lessor, lessor_server) = create_proxy::<LessorMarker>();
        let (element_c_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("C".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                            requires_token: Some(
                                element_b_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![
                                BinaryPowerLevel::On.into_primitive()
                            ]),
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_server),
                        element_control: Some(element_control_server),
                        element_runner: Some(element_c_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let element_c_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            element_c_element_control.open_status_channel(server)?;
            client.into_proxy()
        };
        let (element_d_runner_client, element_d_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_d_runner = element_d_runner_server.into_stream();
        let (element_d_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("D".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        element_control: Some(element_control_server),
                        element_runner: Some(element_d_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let element_d_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            element_d_element_control.open_status_channel(server)?;
            client.into_proxy()
        };

        // Initial required level for each element should be OFF.
        // Set managed elements' current level to OFF.
        for (status, runner) in [
            (&element_a_status, &mut element_a_runner),
            (&element_b_status, &mut element_b_runner),
            (&element_c_status, &mut element_c_runner),
            (&element_d_status, &mut element_d_runner),
        ] {
            executor.run_singlethreaded(async {
                let current = assert_set_level_required_eq_and_return_responder(
                    runner.try_next(),
                    BinaryPowerLevel::Off.into_primitive(),
                )
                .await;
                current.send().expect("set_level resp failed");
                let power_level =
                    status.watch_power_level().await.unwrap().expect("watch_power_level failed");
                assert_eq!(power_level, BinaryPowerLevel::Off.into_primitive());
            });
        }
        let element_a_runner_next = element_a_runner.try_next();
        let mut element_b_runner_next = element_b_runner.try_next();
        let mut element_c_runner_next = element_c_runner.try_next();
        let mut element_d_runner_next = element_d_runner.try_next();

        // Acquire lease for C.
        // A's required level should become ON.
        // B's required level should remain OFF because A is not yet ON.
        // C's required level should remain OFF because B is not yet ON.
        // D's required level should remain OFF.
        let lease = executor.run_singlethreaded(async {
            element_c_lessor
                .lease(BinaryPowerLevel::On.into_primitive())
                .await
                .unwrap()
                .expect("Lease response not ok")
                .into_proxy()
        });
        let element_a_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                element_a_runner_next,
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });
        let mut element_a_runner_next = element_a_runner.try_next();
        assert!(executor.run_until_stalled(&mut element_b_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_c_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Pending
            );
        });

        // Update A's current level to ON.
        // A's required level should remain ON.
        // B's required level should become ON because A is now ON.
        // C's required level should remain OFF because B is not yet ON.
        // D's required level should remain OFF.
        executor.run_singlethreaded(async {
            assert_send_response_updates_level_to(
                element_a_current,
                &element_a_status,
                BinaryPowerLevel::On.into_primitive(),
            )
            .await;
        });
        assert!(executor.run_until_stalled(&mut element_a_runner_next).is_pending());
        let element_b_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                element_b_runner_next,
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });
        let mut element_b_runner_next = element_b_runner.try_next();
        assert!(executor.run_until_stalled(&mut element_c_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());

        // Update B's current level to ON.
        // A's required level should remain ON.
        // B's required level should remain ON.
        // C's required level should become ON because B is now ON.
        // D's required level should remain OFF.
        executor.run_singlethreaded(async {
            assert_send_response_updates_level_to(
                element_b_current,
                &element_b_status,
                BinaryPowerLevel::On.into_primitive(),
            )
            .await;
        });
        assert!(executor.run_until_stalled(&mut element_a_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_b_runner_next).is_pending());
        let element_c_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                element_c_runner_next,
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });
        let mut element_c_runner_next = element_c_runner.try_next();
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());

        // Update C's current level to ON.
        // A's required level should remain ON.
        // B's required level should remain ON.
        // C's required level should remain ON.
        // D's required level should remain OFF.
        // Lease should become satisfied.
        executor.run_singlethreaded(async {
            assert_send_response_updates_level_to(
                element_c_current,
                &element_c_status,
                BinaryPowerLevel::On.into_primitive(),
            )
            .await;
        });
        assert!(executor.run_until_stalled(&mut element_a_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_b_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_c_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Drop lease for C.
        // A's required level should remain ON.
        // B's required level should remain ON.
        // C's required level should become OFF because the lease was dropped.
        // D's required level should remain OFF.
        executor.run_singlethreaded(async {
            drop(lease);
        });
        assert!(executor.run_until_stalled(&mut element_a_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_b_runner_next).is_pending());
        let element_c_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                element_c_runner_next,
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await
        });
        let mut element_c_runner_next = element_c_runner.try_next();
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());

        // Lower C's current level to OFF.
        // A's required level should remain ON.
        // B's required level should become OFF.
        // C's required level should remain OFF.
        // D's required level should remain OFF.
        executor.run_singlethreaded(async {
            assert_send_response_updates_level_to(
                element_c_current,
                &element_c_status,
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
        });
        assert!(executor.run_until_stalled(&mut element_a_runner_next).is_pending());
        let element_b_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                element_b_runner_next,
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await
        });
        let mut element_b_runner_next = element_b_runner.try_next();
        assert!(executor.run_until_stalled(&mut element_c_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());

        // Lower B's current level to OFF.
        // A's required level should become OFF because B is no longer dependent.
        // B's required level should remain OFF.
        // C's required level should remain OFF.
        // D's required level should remain OFF.
        executor.run_singlethreaded(async {
            assert_send_response_updates_level_to(
                element_b_current,
                &element_b_status,
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
        });
        executor.run_singlethreaded(async {
            let current = assert_set_level_required_eq_and_return_responder(
                element_a_runner_next,
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            assert_send_response_updates_level_to(
                current,
                &element_a_status,
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
        });
        assert!(executor.run_until_stalled(&mut element_b_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_c_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut element_d_runner_next).is_pending());

        Ok(())
    }

    /// Verifies a shared parent/grandparent dependency hierarchy with multiple child elements
    /// requiring different levels, ensuring intermediate and root elements scale to the maximum
    /// required level and step down appropriately as leases are dropped.
    #[fuchsia::test]
    fn test_shared() -> Result<()> {
        // Create a topology of two child elements (C1 & C2) with a shared
        // parent (P) and grandparent (GP)
        // C1 \
        //     > P -> GP
        // C2 /
        // Child 1 requires Parent at 50 to support its own level of 5.
        // Parent requires Grandparent at 200 to support its own level of 50.
        // C1 -> P -> GP
        //  5 -> 50 -> 200
        // Child 2 requires Parent at 30 to support its own level of 3.
        // Parent requires Grandparent at 90 to support its own level of 30.
        // C2 -> P -> GP
        //  3 -> 30 -> 90
        // Grandparent has a default minimum level of 10.
        // All other elements have a default of 0.
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
        let grandparent_token = zx::Event::create();
        let (grandparent_runner_client, grandparent_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut grandparent_runner = grandparent_runner_server.into_stream();
        let (grandparent_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("GP".into()),
                        initial_current_level: Some(10),
                        valid_levels: Some(vec![10, 90, 200]),
                        element_control: Some(element_control_server),
                        element_runner: Some(grandparent_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
            assert!(
                grandparent_element_control
                    .register_dependency_token(
                        grandparent_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    )
                    .await
                    .is_ok()
            );
        });
        let grandparent_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            grandparent_element_control.open_status_channel(server)?;
            client.into_proxy()
        };
        let parent_token = zx::Event::create();
        let (parent_runner_client, parent_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut parent_runner = parent_runner_server.into_stream();
        let (parent_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("P".into()),
                        initial_current_level: Some(0),
                        valid_levels: Some(vec![0, 30, 50]),
                        dependencies: Some(vec![
                            LevelDependency {
                                dependent_level: Some(50),
                                requires_token: Some(
                                    grandparent_token
                                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                        .expect("dup failed")
                                ),
                                requires_level_by_preference: Some(vec![200]),
                                ..Default::default()
                            },
                            LevelDependency {
                                dependent_level: Some(30),
                                requires_token: Some(
                                    grandparent_token
                                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                        .expect("dup failed")
                                ),
                                requires_level_by_preference: Some(vec![90]),
                                ..Default::default()
                            },
                        ]),
                        element_control: Some(element_control_server),
                        element_runner: Some(parent_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
            assert!(
                parent_element_control
                    .register_dependency_token(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    )
                    .await
                    .is_ok()
            );
        });
        let parent_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            parent_element_control.open_status_channel(server)?;
            client.into_proxy()
        };
        let (child1_runner_client, child1_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut child1_runner = child1_runner_server.into_stream();
        let (child1_lessor, lessor_server) = create_proxy::<LessorMarker>();
        let (child1_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("C1".into()),
                        initial_current_level: Some(0),
                        valid_levels: Some(vec![0, 5]),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(5),
                            requires_token: Some(
                                parent_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![50]),
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_server),
                        element_control: Some(element_control_server),
                        element_runner: Some(child1_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let child1_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            child1_element_control.open_status_channel(server)?;
            client.into_proxy()
        };
        let (child2_runner_client, child2_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut child2_runner = child2_runner_server.into_stream();
        let (child2_lessor, lessor_server) = create_proxy::<LessorMarker>();
        let (child2_element_control, element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("C2".into()),
                        initial_current_level: Some(0),
                        valid_levels: Some(vec![0, 3]),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(3),
                            requires_token: Some(
                                parent_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![30]),
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_server),
                        element_control: Some(element_control_server),
                        element_runner: Some(child2_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let child2_status: fpb::StatusProxy = {
            let (client, server) = create_endpoints::<StatusMarker>();
            child2_element_control.open_status_channel(server)?;
            client.into_proxy()
        };

        // GP should have a initial required level of 10
        // P, C1 and C2 should have initial required levels of 0.
        executor.run_singlethreaded(async {
            let grandparent_current = assert_set_level_required_eq_and_return_responder(
                grandparent_runner.try_next(),
                10,
            )
            .await;
            grandparent_current.send().expect("set_level resp failed");
            assert_eq!(grandparent_status.watch_power_level().await.unwrap(), Ok(10));

            let parent_current =
                assert_set_level_required_eq_and_return_responder(parent_runner.try_next(), 0)
                    .await;
            parent_current.send().expect("set_level resp failed");
            assert_eq!(parent_status.watch_power_level().await.unwrap(), Ok(0));

            let child1_current =
                assert_set_level_required_eq_and_return_responder(child1_runner.try_next(), 0)
                    .await;
            child1_current.send().expect("set_level resp failed");
            assert_eq!(child1_status.watch_power_level().await.unwrap(), Ok(0));

            let child2_current =
                assert_set_level_required_eq_and_return_responder(child2_runner.try_next(), 0)
                    .await;
            child2_current.send().expect("set_level resp failed");
            assert_eq!(child2_status.watch_power_level().await.unwrap(), Ok(0));
        });
        let grandparent_runner_next = grandparent_runner.try_next();
        let mut parent_runner_next = parent_runner.try_next();
        let mut child1_runner_next = child1_runner.try_next();
        let mut child2_runner_next = child2_runner.try_next();

        // Acquire lease for C1 @ 5.
        // GP's required level should become 200 because C1 @ 5 has a
        // dependency on P @ 50 and P @ 50 has a dependency on GP @ 200.
        // GP @ 200 has no dependencies so its level should be raised first.
        // P's required level should remain 0 because GP is not yet at 200.
        // C1's required level should remain 0 because P is not yet at 50.
        // C2's required level should remain 0.
        let lease_child_1 = executor.run_singlethreaded(async {
            child1_lessor.lease(5).await.unwrap().expect("Lease response not ok").into_proxy()
        });
        let grandparent_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(grandparent_runner_next, 200).await
        });
        let mut grandparent_runner_next = grandparent_runner.try_next();
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Pending
            );
        });

        // Raise GP's current level to 200.
        // GP's required level should remain 200.
        // P's required level should become 50 because GP is now at 200.
        // C1's required level should remain 0 because P is not yet at 50.
        // C2's required level should remain 0.
        executor.run_singlethreaded(async {
            grandparent_current.send().expect("set_level resp failed");
            assert_eq!(grandparent_status.watch_power_level().await.unwrap(), Ok(200));
        });
        let parent_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(parent_runner_next, 50).await
        });
        let mut parent_runner_next = parent_runner.try_next();
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Pending
            );
        });

        // Update P's current level to 50.
        // GP's required level should remain 200.
        // P's required level should remain 50.
        // C1's required level should become 5 because P is now at 50.
        // C2's required level should remain 0.
        executor.run_singlethreaded(async {
            parent_current.send().expect("set_level resp failed");
            assert_eq!(parent_status.watch_power_level().await.unwrap(), Ok(50));
        });
        let child1_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(child1_runner_next, 5).await
        });
        let mut child1_runner_next = child1_runner.try_next();
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());

        // Update C1's current level to 5.
        // GP's required level should remain 200.
        // P's required level should remain 50.
        // C1's required level should remain 5.
        // C2's required level should remain 0.
        // C1's lease @ 5 is now satisfied.
        executor.run_singlethreaded(async {
            child1_current.send().expect("set_level resp failed");
            assert_eq!(child1_status.watch_power_level().await.unwrap(), Ok(5));
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_1.watch_status(LeaseStatus::Pending).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Acquire lease for C2 @ 3.
        // Though C2 @ 3 has nominal requirements of P @ 30 and GP @ 90,
        // they are superseded by C1's requirements of 50 and 200.
        // GP's required level should remain 200.
        // P's required level should remain 50.
        // C1's required level should remain 5.
        // C2's required level should become 3 because its dependencies are already satisfied.
        // C1's lease @ 5 is still satisfied.
        let lease_child_2 = executor.run_singlethreaded(async {
            child2_lessor.lease(3).await.unwrap().expect("Lease response not ok").into_proxy()
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        let child2_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(child2_runner_next, 3).await
        });
        let mut child2_runner_next = child2_runner.try_next();
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Update C2's current level to 3.
        // GP's required level should remain 200.
        // P's required level should remain 50.
        // C1's required level should remain 5.
        // C2's required level should remain 0.
        // C1's lease @ 5 is still satisfied.
        // C2's lease @ 3 is now satisfied.
        executor.run_singlethreaded(async {
            child2_current.send().expect("set_level resp failed");
            assert_eq!(child2_status.watch_power_level().await.unwrap(), Ok(3));
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
            assert_eq!(
                lease_child_2.watch_status(LeaseStatus::Pending).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });
        // Drop lease for C1.
        // GP's required level should remain 200.
        // P's required level should remain 50.
        // C1's required level should become 0 because its lease has been dropped.
        // C2's required level should remain 3.
        // C2's lease @ 3 is still satisfied.
        let child1_current = executor.run_singlethreaded(async {
            drop(lease_child_1);
            assert_set_level_required_eq_and_return_responder(child1_runner_next, 0).await
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        let mut child1_runner_next = child1_runner.try_next();
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_2.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Lower C1's current level to 0.
        // GP's required level should remain 200.
        // P's required level should become 30.
        // C1's required level should remain 0.
        // C2's required level should remain 3.
        // C2's lease @ 3 is still satisfied.
        let parent_current = executor.run_singlethreaded(async {
            child1_current.send().expect("set_level resp failed");
            assert_eq!(child1_status.watch_power_level().await.unwrap(), Ok(0));
            assert_set_level_required_eq_and_return_responder(parent_runner_next, 30).await
        });
        let mut parent_runner_next = parent_runner.try_next();
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_2.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Lower Parent's current level to 30.
        // GP's required level should become 90 because P has dropped to 30.
        // P's required level should remain 30.
        // C1's required level should remain 0.
        // C2's required level should remain 3.
        // C2's lease @ 3 is still satisfied.
        let grandparent_current = executor.run_singlethreaded(async {
            parent_current.send().expect("set_level resp failed");
            assert_eq!(parent_status.watch_power_level().await.unwrap(), Ok(30));
            assert_set_level_required_eq_and_return_responder(grandparent_runner_next, 90).await
        });
        let mut grandparent_runner_next = grandparent_runner.try_next();
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_child_2.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Drop lease for Child 2.
        // GP's required level should remain 90 because P is still at 30.
        // P's required level should remain 30.
        // C1's required level should remain 0.
        // C2's required level should become 0 because its lease has been dropped.
        let child2_current = executor.run_singlethreaded(async {
            drop(lease_child_2);
            assert_set_level_required_eq_and_return_responder(child2_runner_next, 0).await
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        let mut child2_runner_next = child2_runner.try_next();

        // Lower GP's current level to 90.
        // GP's required level should remain 90 because P is still at 30.
        // P's required level should remain 30.
        // C1's required level should remain 0.
        // C2's required level should remain 0.
        executor.run_singlethreaded(async {
            grandparent_current.send().expect("set_level resp failed");
            assert_eq!(grandparent_status.watch_power_level().await.unwrap(), Ok(90));
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());

        // Lower C2's current level to 0.
        // GP's required level should remain 90 because P is still at 30.
        // P's required level should become 0.
        // C1's required level should remain 0.
        // C2's required level should remain 0.
        let parent_current = executor.run_singlethreaded(async {
            child2_current.send().expect("set_level resp failed");
            assert_eq!(child2_status.watch_power_level().await.unwrap(), Ok(0));
            assert_set_level_required_eq_and_return_responder(parent_runner_next, 0).await
        });
        let mut parent_runner_next = parent_runner.try_next();
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());

        // Lower Parent's current level to 0.
        // GP's required level should become its minimum level of 10 because P is now at 0.
        // P's required level should remain 0.
        // C1's required level should remain 0.
        // C2's required level should remain 0.
        let grandparent_current = executor.run_singlethreaded(async {
            parent_current.send().expect("set_level resp failed");
            assert_eq!(parent_status.watch_power_level().await.unwrap(), Ok(0));
            assert_set_level_required_eq_and_return_responder(grandparent_runner_next, 10).await
        });
        let mut grandparent_runner_next = grandparent_runner.try_next();
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());

        // Lower GP's current level to 10.
        // GP's required level should remain 10.
        // P's required level should remain 0.
        // C1's required level should remain 0.
        // C2's required level should remain 0.
        executor.run_singlethreaded(async {
            grandparent_current.send().expect("set_level resp failed");
            assert_eq!(grandparent_status.watch_power_level().await.unwrap(), Ok(10));
        });
        assert!(executor.run_until_stalled(&mut grandparent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut parent_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child1_runner_next).is_pending());
        assert!(executor.run_until_stalled(&mut child2_runner_next).is_pending());

        Ok(())
    }

    /// Verifies validation and error handling when adding elements, including unauthorized
    /// dependency tokens and invalid required power levels.
    #[fuchsia::test]
    async fn test_add_element_errors() -> Result<(), Error> {
        let realm = build_power_broker_realm().await?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // Create a root element
        let earth_token = zx::Event::create();
        let (element_runner_client, _) = create_endpoints::<ElementRunnerMarker>();
        let (element_control, element_control_server) = create_proxy::<ElementControlMarker>();
        assert!(
            topology
                .add_element(ElementSchema {
                    element_name: Some("Earth".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    element_control: Some(element_control_server),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await
                .is_ok()
        );
        assert!(
            element_control
                .register_dependency_token(
                    earth_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                )
                .await
                .is_ok()
        );

        // Using an unauthorized dependency token yields AddElementError::NotAuthorized.
        let (element_runner_client, _) = create_endpoints::<ElementRunnerMarker>();
        assert_matches!(
            topology
                .add_element(ElementSchema {
                    element_name: Some("Water".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                        requires_token: Some(zx::Event::create()),
                        requires_level_by_preference: Some(vec![
                            BinaryPowerLevel::On.into_primitive()
                        ]),
                        ..Default::default()
                    }]),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await,
            Ok(Err(fpb::AddElementError::NotAuthorized))
        );

        // Using an invalid level in a dependency yields AddElementError::Invalid.
        let (element_runner_client, _) = create_endpoints::<ElementRunnerMarker>();
        assert_matches!(
            topology
                .add_element(ElementSchema {
                    element_name: Some("Air".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                        requires_token: Some(
                            earth_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed")
                        ),
                        requires_level_by_preference: Some(vec![2]),
                        ..Default::default()
                    }]),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await,
            Ok(Err(fpb::AddElementError::Invalid))
        );

        // Using the correct dependency token succeeds.
        let (element_runner_client, _) = create_endpoints::<ElementRunnerMarker>();
        assert!(
            topology
                .add_element(ElementSchema {
                    element_name: Some("Fire".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                        requires_token: Some(
                            earth_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed")
                        ),
                        requires_level_by_preference: Some(vec![
                            BinaryPowerLevel::On.into_primitive()
                        ]),
                        ..Default::default()
                    }]),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await
                .is_ok()
        );

        Ok(())
    }

    /// Verifies that dropping an element's `ElementControl` channel removes the element and closes
    /// any associated `Status` channels.
    #[fuchsia::test]
    async fn test_closing_element_control_closes_status() -> Result<(), Error> {
        let realm = build_power_broker_realm().await?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        let (element_runner_client, _) = create_endpoints::<ElementRunnerMarker>();
        let (element_control, element_control_server) = create_proxy::<ElementControlMarker>();
        assert!(
            topology
                .add_element(ElementSchema {
                    element_name: Some("Fire".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    element_control: Some(element_control_server),
                    element_runner: Some(element_runner_client),
                    ..Default::default()
                })
                .await
                .is_ok()
        );

        // Confirm the element has been removed and Status channels have been
        // closed.
        let status_proxy = {
            let (client, server) = create_proxy::<StatusMarker>();
            element_control.open_status_channel(server)?;
            client
        };
        drop(element_control);
        status_proxy.as_channel().on_closed().await?;

        Ok(())
    }

    /// Verifies that `Status.WatchPowerLevel` accurately observes power level changes as leases
    /// are acquired and dropped.
    #[fuchsia::test]
    fn test_status_watch_power_level() -> Result<(), Error> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;

        // Create a topology with only one element:
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
        let (element_runner_client, element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_runner = element_runner_server.into_stream();
        let (lessor, lessor_server) = create_proxy::<LessorMarker>();
        let (element_control, element_control_server) = create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("E".into()),
                        initial_current_level: Some(0),
                        valid_levels: Some(vec![0, 1, 2]),
                        lessor_channel: Some(lessor_server),
                        element_control: Some(element_control_server),
                        element_runner: Some(element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let status = {
            let (client, server) = create_proxy::<StatusMarker>();
            element_control.open_status_channel(server)?;
            client
        };

        executor.run_singlethreaded(async {
            // Initial power level should be 0.
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 0)
                    .await;
            responder.send().unwrap();
            assert_eq!(status.watch_power_level().await.unwrap(), Ok(0));

            // Acquire a lease to change the required level to 1.
            let lease1 = lessor.lease(1).await.unwrap().unwrap().into_proxy();
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 1)
                    .await;
            responder.send().unwrap();
            assert_eq!(status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(
                lease1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );

            // Acquire another lease to change the required level to 2.
            let lease2 = lessor.lease(2).await.unwrap().unwrap().into_proxy();
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 2)
                    .await;
            responder.send().unwrap();
            // DON'T call watch_power_level here--we want to make sure it skips to the most recent
            assert_eq!(
                lease2.watch_status(LeaseStatus::Pending).await.unwrap(),
                LeaseStatus::Satisfied
            );
            // Drop lease2, level should go back to 1.
            drop(lease2);
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 1)
                    .await;
            responder.send().unwrap();
            // Sleep here because otherwise we might call this before PB has updated from 2 to 1
            thread::sleep(Duration::from_millis(100));
            assert_eq!(status.watch_power_level().await.unwrap(), Ok(1));

            // Drop lease1, level should go back to 0.
            drop(lease1);
            let responder =
                assert_set_level_required_eq_and_return_responder(element_runner.try_next(), 0)
                    .await;
            responder.send().unwrap();
            assert_eq!(status.watch_power_level().await.unwrap(), Ok(0));
        });

        // Ensure there are no more current levels in the queue.
        assert!(executor.run_until_stalled(&mut status.watch_power_level()).is_pending());

        Ok(())
    }

    /// Verifies that an element added with an `initial_lease_token` acquires a pending initial
    /// lease that becomes satisfied once its dependencies and own power level are satisfied.
    #[fuchsia::test]
    fn test_add_element_initial_lease() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // Create a topology with only two elements and a single dependency:
        // A -> B
        // Setup Dependency "B"
        let element_b_token = zx::Event::create();
        let (element_b_runner_client, element_b_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut element_b_runner = element_b_runner_server.into_stream();
        let (element_b_control, element_b_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("B".into()),
                    initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                    valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                    element_control: Some(element_b_control_server),
                    element_runner: Some(element_b_runner_client),
                    ..Default::default()
                })
                .await
                .expect("AddElement B failed")
                .expect("AddElement B error");

            element_b_control
                .register_dependency_token(
                    element_b_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                )
                .await
                .expect("register_dependency_token failed")
                .expect("register_dependency_token error");
        });

        // Initialize B to OFF
        executor.run_singlethreaded(async {
            let responder = assert_set_level_required_eq_and_return_responder(
                element_b_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            responder.send().expect("send failed");
        });

        // Setup Dependent "A" with initial_lease_token
        let (element_a_runner_client, element_a_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let (_element_a_control, element_a_control_server) = create_proxy::<ElementControlMarker>();

        let (lease_token_client, lease_token_server) = zx::EventPair::create();

        // We run AddElement for A. B is currently OFF, so it will need to be turned ON.
        let add_a_fut = Box::pin(topology.add_element(ElementSchema {
            element_name: Some("A".into()),
            initial_current_level: Some(BinaryPowerLevel::On.into_primitive()), // Initialized to ON
            valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
            dependencies: Some(vec![LevelDependency {
                dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                requires_token: Some(
                    element_b_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                ),
                requires_level_by_preference: Some(vec![BinaryPowerLevel::On.into_primitive()]),
                ..Default::default()
            }]),
            element_control: Some(element_a_control_server),
            element_runner: Some(element_a_runner_client),
            initial_lease_token: Some(lease_token_server), // Request Lease at Initial Level (ON)
            ..Default::default()
        }));

        // AddElement should return immediately with the lease granted (but not yet satisfied).
        executor.run_singlethreaded(async {
            add_a_fut.await.unwrap().unwrap();
        });

        // B should receive SetLevel(ON).
        let element_b_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                element_b_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });

        // Lease is not yet satisfied, because B is not ON.
        assert_matches!(
            lease_token_client.wait_one(
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
                zx::MonotonicInstant::INFINITE_PAST,
            ),
            zx::WaitResult::TimedOut(_)
        );

        // 6. Turn B ON
        executor.run_singlethreaded(async {
            element_b_current.send().expect("send failed");
        });

        // Lease is still not satisfied, because A is not ON.
        assert_matches!(
            lease_token_client.wait_one(
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
                zx::MonotonicInstant::INFINITE_PAST,
            ),
            zx::WaitResult::TimedOut(_)
        );

        // A should receive SetLevel(ON).
        executor.run_singlethreaded(async {
            let mut stream = element_a_runner_server.into_stream();
            if let Ok(Some(fpb::ElementRunnerRequest::SetLevel { level, responder })) =
                stream.try_next().await
            {
                assert_eq!(
                    level,
                    BinaryPowerLevel::On.into_primitive(),
                    "First SetLevel must be ON"
                );
                responder.send().expect("responder failed");
            }
        });

        // The lease should now be SATISFIED.
        executor.run_singlethreaded(async {
            fasync::OnSignals::new(
                &lease_token_client,
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
            )
            .await
            .unwrap();
        });

        Ok(())
    }

    /// Verifies `remove_with_required_element` dependency behavior: when a parent element is
    /// removed, removable dependent leases remain satisfied while unremovable dependent leases
    /// transition to pending.
    #[fuchsia::test]
    fn test_dependency_removal() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;

        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // Parent P:
        let parent_token = zx::Event::create();
        let (parent_element_runner_client, parent_element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut parent_element_runner = parent_element_runner_server.into_stream();
        let (parent_element_control, parent_element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("P".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        element_control: Some(parent_element_control_server),
                        element_runner: Some(parent_element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
            assert!(
                parent_element_control
                    .register_dependency_token(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    )
                    .await
                    .is_ok()
            );
        });
        let parent_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            parent_element_control.open_status_channel(server)?;
            client
        };

        // Child Removable (removable: remove_with_required_element = true):
        let (child_removable_element_runner_client, child_removable_element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut child_removable_element_runner =
            child_removable_element_runner_server.into_stream();
        let (child_removable_lessor, lessor_removable_server) = create_proxy::<LessorMarker>();
        let (child_removable_element_control, child_removable_element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("child_removable".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                            requires_token: Some(
                                parent_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![
                                BinaryPowerLevel::On.into_primitive()
                            ]),
                            remove_with_required_element: Some(true),
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_removable_server),
                        element_control: Some(child_removable_element_control_server),
                        element_runner: Some(child_removable_element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let child_removable_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            child_removable_element_control.open_status_channel(server)?;
            client
        };

        // Child Unremovable (non-removable: remove_with_required_element = false):
        let (child_unremovable_element_runner_client, child_unremovable_element_runner_server) =
            create_endpoints::<ElementRunnerMarker>();
        let mut child_unremovable_element_runner =
            child_unremovable_element_runner_server.into_stream();
        let (child_unremovable_lessor, lessor_unremovable_server) = create_proxy::<LessorMarker>();
        let (child_unremovable_element_control, child_unremovable_element_control_server) =
            create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("child_unremovable".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                            requires_token: Some(
                                parent_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![
                                BinaryPowerLevel::On.into_primitive()
                            ]),
                            remove_with_required_element: Some(false),
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_unremovable_server),
                        element_control: Some(child_unremovable_element_control_server),
                        element_runner: Some(child_unremovable_element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let child_unremovable_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            child_unremovable_element_control.open_status_channel(server)?;
            client
        };

        // Child Default Unremovable (non-removable by default: remove_with_required_element = None):
        let (
            child_default_unremovable_element_runner_client,
            child_default_unremovable_element_runner_server,
        ) = create_endpoints::<ElementRunnerMarker>();
        let mut child_default_unremovable_element_runner =
            child_default_unremovable_element_runner_server.into_stream();
        let (child_default_unremovable_lessor, lessor_default_unremovable_server) =
            create_proxy::<LessorMarker>();
        let (
            child_default_unremovable_element_control,
            child_default_unremovable_element_control_server,
        ) = create_proxy::<ElementControlMarker>();
        executor.run_singlethreaded(async {
            assert!(
                topology
                    .add_element(ElementSchema {
                        element_name: Some("child_default_unremovable".into()),
                        initial_current_level: Some(BinaryPowerLevel::Off.into_primitive()),
                        valid_levels: Some(BINARY_POWER_LEVELS.to_vec()),
                        dependencies: Some(vec![LevelDependency {
                            dependent_level: Some(BinaryPowerLevel::On.into_primitive()),
                            requires_token: Some(
                                parent_token
                                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                    .expect("dup failed")
                            ),
                            requires_level_by_preference: Some(vec![
                                BinaryPowerLevel::On.into_primitive()
                            ]),
                            remove_with_required_element: None,
                            ..Default::default()
                        }]),
                        lessor_channel: Some(lessor_default_unremovable_server),
                        element_control: Some(child_default_unremovable_element_control_server),
                        element_runner: Some(child_default_unremovable_element_runner_client),
                        ..Default::default()
                    })
                    .await
                    .is_ok()
            );
        });
        let child_default_unremovable_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            child_default_unremovable_element_control.open_status_channel(server)?;
            client
        };

        // Set initial levels to OFF
        executor.run_singlethreaded(async {
            let parent_current = assert_set_level_required_eq_and_return_responder(
                parent_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            let child_removable_current = assert_set_level_required_eq_and_return_responder(
                child_removable_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            let child_unremovable_current = assert_set_level_required_eq_and_return_responder(
                child_unremovable_element_runner.try_next(),
                BinaryPowerLevel::Off.into_primitive(),
            )
            .await;
            let child_default_unremovable_current =
                assert_set_level_required_eq_and_return_responder(
                    child_default_unremovable_element_runner.try_next(),
                    BinaryPowerLevel::Off.into_primitive(),
                )
                .await;
            parent_current.send().expect("set_level resp failed");
            child_removable_current.send().expect("set_level resp failed");
            child_unremovable_current.send().expect("set_level resp failed");
            child_default_unremovable_current.send().expect("set_level resp failed");
            assert_eq!(
                parent_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
            assert_eq!(
                child_removable_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
            assert_eq!(
                child_unremovable_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
            assert_eq!(
                child_default_unremovable_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::Off.into_primitive())
            );
        });

        // Lease children. Leases should be pending because P is OFF.
        let lease_removable = executor.run_singlethreaded(async {
            child_removable_lessor
                .lease(BinaryPowerLevel::On.into_primitive())
                .await
                .unwrap()
                .expect("Lease response not ok")
                .into_proxy()
        });
        let lease_unremovable = executor.run_singlethreaded(async {
            child_unremovable_lessor
                .lease(BinaryPowerLevel::On.into_primitive())
                .await
                .unwrap()
                .expect("Lease response not ok")
                .into_proxy()
        });
        let lease_default_unremovable = executor.run_singlethreaded(async {
            child_default_unremovable_lessor
                .lease(BinaryPowerLevel::On.into_primitive())
                .await
                .unwrap()
                .expect("Lease response not ok")
                .into_proxy()
        });

        // P should receive SetLevel(ON).
        let parent_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                parent_element_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });

        // Children should not receive SetLevel because P is still OFF.
        let mut child_removable_element_runner_next = child_removable_element_runner.try_next();
        let mut child_unremovable_element_runner_next = child_unremovable_element_runner.try_next();
        let mut child_default_unremovable_element_runner_next =
            child_default_unremovable_element_runner.try_next();
        assert!(executor.run_until_stalled(&mut child_removable_element_runner_next).is_pending());
        assert!(
            executor.run_until_stalled(&mut child_unremovable_element_runner_next).is_pending()
        );
        assert!(
            executor
                .run_until_stalled(&mut child_default_unremovable_element_runner_next)
                .is_pending()
        );

        // Transition P to ON.
        executor.run_singlethreaded(async {
            parent_current.send().expect("set_level resp failed");
            assert_eq!(
                parent_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::On.into_primitive())
            );
        });

        // Children should now receive SetLevel(ON).
        let child_removable_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                child_removable_element_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });
        let child_unremovable_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                child_unremovable_element_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });
        let child_default_unremovable_current = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(
                child_default_unremovable_element_runner.try_next(),
                BinaryPowerLevel::On.into_primitive(),
            )
            .await
        });

        // Transition Children to ON. Leases should become Satisfied.
        executor.run_singlethreaded(async {
            child_removable_current.send().expect("set_level resp failed");
            child_unremovable_current.send().expect("set_level resp failed");
            child_default_unremovable_current.send().expect("set_level resp failed");
            assert_eq!(
                child_removable_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::On.into_primitive())
            );
            assert_eq!(
                child_unremovable_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::On.into_primitive())
            );
            assert_eq!(
                child_default_unremovable_status.watch_power_level().await.unwrap(),
                Ok(BinaryPowerLevel::On.into_primitive())
            );
            assert_eq!(
                lease_removable.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
            assert_eq!(
                lease_unremovable.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
            assert_eq!(
                lease_default_unremovable.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Drop parent element control to remove P.
        drop(parent_element_control);

        // Verify parent status channel is closed.
        executor.run_singlethreaded(async {
            parent_status.as_channel().on_closed().await.unwrap();
        });

        // Since child_removable has removable: true, its lease stays Satisfied.
        // child_unremovable (removable: false) and child_default_unremovable (removable: default/false) should transition to Pending.
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_removable.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
            assert_eq!(
                lease_unremovable.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Pending
            );
            assert_eq!(
                lease_default_unremovable.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Pending
            );
        });

        Ok(())
    }

    /// Verifies that higher levels implicitly requiring multiple lower-level dependencies
    /// correctly activate all of them.
    #[fuchsia::test]
    fn test_cumulative_implicit_dependencies() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // Element B: valid levels [0, 1]
        let element_b_token = zx::Event::create();
        let (b_runner_client, b_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut b_runner = b_runner_server.into_stream();
        let (b_control, b_control_server) = create_proxy::<ElementControlMarker>();

        // Element C: valid levels [0, 1]
        let element_c_token = zx::Event::create();
        let (c_runner_client, c_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut c_runner = c_runner_server.into_stream();
        let (c_control, c_control_server) = create_proxy::<ElementControlMarker>();

        // Element A: valid levels [0, 1, 2], A(1) -> B(1), A(2) -> C(1)
        let (a_runner_client, a_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut a_runner = a_runner_server.into_stream();
        let (a_lessor, a_lessor_server) = create_proxy::<LessorMarker>();
        let (a_control, a_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("B".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(b_control_server),
                    element_runner: Some(b_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            b_control
                .register_dependency_token(
                    element_b_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("C".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(c_control_server),
                    element_runner: Some(c_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            c_control
                .register_dependency_token(
                    element_c_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("A".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1, 2]),
                    dependencies: Some(vec![
                        LevelDependency {
                            dependent_level: Some(1),
                            requires_token: Some(
                                element_b_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                            ),
                            requires_level_by_preference: Some(vec![1]),
                            ..Default::default()
                        },
                        LevelDependency {
                            dependent_level: Some(2),
                            requires_token: Some(
                                element_c_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                            ),
                            requires_level_by_preference: Some(vec![1]),
                            ..Default::default()
                        },
                    ]),
                    lessor_channel: Some(a_lessor_server),
                    element_control: Some(a_control_server),
                    element_runner: Some(a_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        let b_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            b_control.open_status_channel(server).unwrap();
            client
        };
        let c_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            c_control.open_status_channel(server).unwrap();
            client
        };
        let a_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            a_control.open_status_channel(server).unwrap();
            client
        };

        // Initialize levels to 0.
        executor.run_singlethreaded(async {
            let b_init =
                assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 0).await;
            let c_init =
                assert_set_level_required_eq_and_return_responder(c_runner.try_next(), 0).await;
            let a_init =
                assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 0).await;
            b_init.send().unwrap();
            c_init.send().unwrap();
            a_init.send().unwrap();
            assert_eq!(b_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(c_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // Acquire lease for A at level 2.
        // A@2 implies A@1, so BOTH B@1 and C@1 must be requested.
        let lease_a2 = executor
            .run_singlethreaded(async { a_lessor.lease(2).await.unwrap().unwrap().into_proxy() });

        let b_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 1).await
        });
        let c_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c_runner.try_next(), 1).await
        });
        let mut a_runner_next = a_runner.try_next();
        assert!(executor.run_until_stalled(&mut a_runner_next).is_pending());

        // Update B and C to 1.
        executor.run_singlethreaded(async {
            b_resp.send().unwrap();
            c_resp.send().unwrap();
            assert_eq!(b_status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(c_status.watch_power_level().await.unwrap(), Ok(1));
        });

        // Now A should receive SetLevel(2).
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner_next, 2).await
        });
        executor.run_singlethreaded(async {
            a_resp.send().unwrap();
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(2));
            assert_eq!(
                lease_a2.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Acquire lease for A at level 1, and drop lease for level 2.
        let lease_a1 = executor
            .run_singlethreaded(async { a_lessor.lease(1).await.unwrap().unwrap().into_proxy() });
        drop(lease_a2);

        // A should receive SetLevel(1), C should receive SetLevel(0) (once A is at 1),
        // but B must remain at 1!
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            a_resp.send().unwrap();
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(
                lease_a1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Now C should step down to 0 because A is at 1 (which only requires B).
        let c_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            c_resp.send().unwrap();
            assert_eq!(c_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // B should still be at 1.
        let mut b_runner_next = b_runner.try_next();
        assert!(executor.run_until_stalled(&mut b_runner_next).is_pending());

        // Finally, drop lease_a1.
        drop(lease_a1);
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            a_resp.send().unwrap();
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // B should step down to 0 now that A is at 0.
        let b_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            b_resp.send().unwrap();
            assert_eq!(b_status.watch_power_level().await.unwrap(), Ok(0));
        });

        Ok(())
    }

    /// Verifies that dropping an intermediate element in a dependency chain correctly transitions
    /// dependent leases to pending and steps down active elements.
    #[fuchsia::test]
    fn test_transitive_intermediate_element_removal() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // A <- B <- C
        let element_a_token = zx::Event::create();
        let (a_runner_client, a_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut a_runner = a_runner_server.into_stream();
        let (a_control, a_control_server) = create_proxy::<ElementControlMarker>();

        let element_b_token = zx::Event::create();
        let (b_runner_client, b_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut b_runner = b_runner_server.into_stream();
        let (b_control, b_control_server) = create_proxy::<ElementControlMarker>();

        let (c_runner_client, c_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut c_runner = c_runner_server.into_stream();
        let (c_lessor, c_lessor_server) = create_proxy::<LessorMarker>();
        let (c_control, c_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("A".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(a_control_server),
                    element_runner: Some(a_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            a_control
                .register_dependency_token(
                    element_a_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("B".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(1),
                        requires_token: Some(
                            element_a_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                        ),
                        requires_level_by_preference: Some(vec![1]),
                        ..Default::default()
                    }]),
                    element_control: Some(b_control_server),
                    element_runner: Some(b_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            b_control
                .register_dependency_token(
                    element_b_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("C".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(1),
                        requires_token: Some(
                            element_b_token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                        ),
                        requires_level_by_preference: Some(vec![1]),
                        ..Default::default()
                    }]),
                    lessor_channel: Some(c_lessor_server),
                    element_control: Some(c_control_server),
                    element_runner: Some(c_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        let a_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            a_control.open_status_channel(server).unwrap();
            client
        };
        let b_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            b_control.open_status_channel(server).unwrap();
            client
        };
        let c_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            c_control.open_status_channel(server).unwrap();
            client
        };

        // Initialize levels to 0.
        executor.run_singlethreaded(async {
            let a_init =
                assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 0).await;
            let b_init =
                assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 0).await;
            let c_init =
                assert_set_level_required_eq_and_return_responder(c_runner.try_next(), 0).await;
            a_init.send().unwrap();
            b_init.send().unwrap();
            c_init.send().unwrap();
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(b_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(c_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // Lease C at 1.
        let lease = executor
            .run_singlethreaded(async { c_lessor.lease(1).await.unwrap().unwrap().into_proxy() });

        // Power up A -> B -> C.
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            a_resp.send().unwrap();
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(1));
        });

        let b_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            b_resp.send().unwrap();
            assert_eq!(b_status.watch_power_level().await.unwrap(), Ok(1));
        });

        let c_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            c_resp.send().unwrap();
            assert_eq!(c_status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(
                lease.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Now remove intermediate element B by dropping b_control.
        drop(b_control);

        // B's status channel closes.
        executor.run_singlethreaded(async {
            b_status.as_channel().on_closed().await.unwrap();
        });

        // C's lease should become Pending because B is gone.
        executor.run_singlethreaded(async {
            assert_eq!(
                lease.watch_status(LeaseStatus::Satisfied).await.unwrap(),
                LeaseStatus::Pending
            );
        });

        // C receives SetLevel(0) and steps down.
        let c_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            c_resp.send().unwrap();
            assert_eq!(c_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // A receives SetLevel(0) and steps down because B is no longer requiring A.
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            a_resp.send().unwrap();
            assert_eq!(a_status.watch_power_level().await.unwrap(), Ok(0));
        });

        Ok(())
    }

    /// Verifies that a lease requiring multiple elements is only satisfied when all requirements
    /// are met.
    #[fuchsia::test]
    fn test_direct_multi_element_lease() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        let token_a = zx::Event::create();
        let (a_runner_client, a_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut a_runner = a_runner_server.into_stream();
        let (a_control, a_control_server) = create_proxy::<ElementControlMarker>();

        let token_b = zx::Event::create();
        let (b_runner_client, b_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut b_runner = b_runner_server.into_stream();
        let (b_control, b_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("A".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(a_control_server),
                    element_runner: Some(a_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            a_control
                .register_dependency_token(
                    token_a.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("B".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    element_control: Some(b_control_server),
                    element_runner: Some(b_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            b_control
                .register_dependency_token(
                    token_b.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
        });

        // Initialize levels to 0.
        executor.run_singlethreaded(async {
            let a_init =
                assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 0).await;
            let b_init =
                assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 0).await;
            a_init.send().unwrap();
            b_init.send().unwrap();
        });

        let (lease_token_client, lease_token_server) = zx::EventPair::create();

        // Acquire direct lease with multiple requirements: A@1 and B@1.
        executor.run_singlethreaded(async {
            topology
                .lease(fpb::LeaseSchema {
                    lease_token: Some(lease_token_server),
                    lease_name: Some("multi_lease".into()),
                    dependencies: Some(vec![
                        fpb::LeaseDependency {
                            requires_token: Some(token_a),
                            requires_level: Some(1),
                            ..Default::default()
                        },
                        fpb::LeaseDependency {
                            requires_token: Some(token_b),
                            requires_level: Some(1),
                            ..Default::default()
                        },
                    ]),
                    should_return_pending_lease: Some(true),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        // Lease is pending (not yet satisfied).
        assert_matches!(
            lease_token_client.wait_one(
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
                zx::MonotonicInstant::INFINITE_PAST,
            ),
            zx::WaitResult::TimedOut(_)
        );

        // A receives SetLevel(1). Update A to 1.
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 1).await
        });
        a_resp.send().unwrap();

        // Lease is still NOT satisfied because B is not yet 1.
        assert_matches!(
            lease_token_client.wait_one(
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
                zx::MonotonicInstant::INFINITE_PAST,
            ),
            zx::WaitResult::TimedOut(_)
        );

        // B receives SetLevel(1). Update B to 1.
        let b_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 1).await
        });
        b_resp.send().unwrap();

        // Now the lease should be satisfied!
        executor.run_singlethreaded(async {
            fasync::OnSignals::new(
                &lease_token_client,
                zx::Signals::from_bits_truncate(fpb::LEASE_SIGNAL_SATISFIED),
            )
            .await
            .unwrap();
        });

        // Close the lease token to drop the lease.
        drop(lease_token_client);

        // Both A and B should step down to 0.
        let a_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(a_runner.try_next(), 0).await
        });
        let b_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(b_runner.try_next(), 0).await
        });
        a_resp.send().unwrap();
        b_resp.send().unwrap();

        Ok(())
    }

    /// Verifies that a shared dependency scales to the maximum required level of its active
    /// dependents and scales down correctly.
    #[fuchsia::test]
    fn test_shared_element_different_required_levels() -> Result<()> {
        let mut executor = fasync::TestExecutor::new();
        let realm = executor.run_singlethreaded(async { build_power_broker_realm().await })?;
        let topology: TopologyProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        // Shared element S with levels [0, 1, 2]
        let token_s = zx::Event::create();
        let (s_runner_client, s_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut s_runner = s_runner_server.into_stream();
        let (s_control, s_control_server) = create_proxy::<ElementControlMarker>();

        // Child C1 with levels [0, 1], C1(1) requires S(1)
        let (c1_runner_client, c1_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut c1_runner = c1_runner_server.into_stream();
        let (c1_lessor, c1_lessor_server) = create_proxy::<LessorMarker>();
        let (c1_control, c1_control_server) = create_proxy::<ElementControlMarker>();

        // Child C2 with levels [0, 1], C2(1) requires S(2)
        let (c2_runner_client, c2_runner_server) = create_endpoints::<ElementRunnerMarker>();
        let mut c2_runner = c2_runner_server.into_stream();
        let (c2_lessor, c2_lessor_server) = create_proxy::<LessorMarker>();
        let (c2_control, c2_control_server) = create_proxy::<ElementControlMarker>();

        executor.run_singlethreaded(async {
            topology
                .add_element(ElementSchema {
                    element_name: Some("S".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1, 2]),
                    element_control: Some(s_control_server),
                    element_runner: Some(s_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            s_control
                .register_dependency_token(
                    token_s.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("C1".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(1),
                        requires_token: Some(
                            token_s.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                        ),
                        requires_level_by_preference: Some(vec![1]),
                        ..Default::default()
                    }]),
                    lessor_channel: Some(c1_lessor_server),
                    element_control: Some(c1_control_server),
                    element_runner: Some(c1_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();

            topology
                .add_element(ElementSchema {
                    element_name: Some("C2".into()),
                    initial_current_level: Some(0),
                    valid_levels: Some(vec![0, 1]),
                    dependencies: Some(vec![LevelDependency {
                        dependent_level: Some(1),
                        requires_token: Some(
                            token_s.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                        ),
                        requires_level_by_preference: Some(vec![2]),
                        ..Default::default()
                    }]),
                    lessor_channel: Some(c2_lessor_server),
                    element_control: Some(c2_control_server),
                    element_runner: Some(c2_runner_client),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
        });

        let s_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            s_control.open_status_channel(server).unwrap();
            client
        };
        let c1_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            c1_control.open_status_channel(server).unwrap();
            client
        };
        let c2_status = {
            let (client, server) = create_proxy::<StatusMarker>();
            c2_control.open_status_channel(server).unwrap();
            client
        };

        // Initialize levels to 0.
        executor.run_singlethreaded(async {
            let s_init =
                assert_set_level_required_eq_and_return_responder(s_runner.try_next(), 0).await;
            let c1_init =
                assert_set_level_required_eq_and_return_responder(c1_runner.try_next(), 0).await;
            let c2_init =
                assert_set_level_required_eq_and_return_responder(c2_runner.try_next(), 0).await;
            s_init.send().unwrap();
            c1_init.send().unwrap();
            c2_init.send().unwrap();
            assert_eq!(s_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(c1_status.watch_power_level().await.unwrap(), Ok(0));
            assert_eq!(c2_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // Lease C1 at 1. S should receive SetLevel(1).
        let lease_c1 = executor
            .run_singlethreaded(async { c1_lessor.lease(1).await.unwrap().unwrap().into_proxy() });
        let s_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(s_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            s_resp.send().unwrap();
            assert_eq!(s_status.watch_power_level().await.unwrap(), Ok(1));
        });

        // C1 receives SetLevel(1).
        let c1_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c1_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            c1_resp.send().unwrap();
            assert_eq!(c1_status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(
                lease_c1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Now lease C2 at 1. Since C2(1) requires S(2), S should receive SetLevel(2).
        let lease_c2 = executor
            .run_singlethreaded(async { c2_lessor.lease(1).await.unwrap().unwrap().into_proxy() });
        let s_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(s_runner.try_next(), 2).await
        });
        executor.run_singlethreaded(async {
            s_resp.send().unwrap();
            assert_eq!(s_status.watch_power_level().await.unwrap(), Ok(2));
        });

        // C2 receives SetLevel(1).
        let c2_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c2_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            c2_resp.send().unwrap();
            assert_eq!(c2_status.watch_power_level().await.unwrap(), Ok(1));
            assert_eq!(
                lease_c2.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Drop Lease C2.
        drop(lease_c2);

        // C2 steps down to 0.
        let c2_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c2_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            c2_resp.send().unwrap();
            assert_eq!(c2_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // S should step down from 2 to 1 (not to 0, because C1 is still leased).
        let s_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(s_runner.try_next(), 1).await
        });
        executor.run_singlethreaded(async {
            s_resp.send().unwrap();
            assert_eq!(s_status.watch_power_level().await.unwrap(), Ok(1));
        });

        // Lease C1 is still Satisfied.
        executor.run_singlethreaded(async {
            assert_eq!(
                lease_c1.watch_status(LeaseStatus::Unknown).await.unwrap(),
                LeaseStatus::Satisfied
            );
        });

        // Drop Lease C1.
        drop(lease_c1);

        // C1 steps down to 0.
        let c1_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(c1_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            c1_resp.send().unwrap();
            assert_eq!(c1_status.watch_power_level().await.unwrap(), Ok(0));
        });

        // S steps down from 1 to 0.
        let s_resp = executor.run_singlethreaded(async {
            assert_set_level_required_eq_and_return_responder(s_runner.try_next(), 0).await
        });
        executor.run_singlethreaded(async {
            s_resp.send().unwrap();
            assert_eq!(s_status.watch_power_level().await.unwrap(), Ok(0));
        });

        Ok(())
    }
}
