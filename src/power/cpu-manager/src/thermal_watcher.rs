// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log_if_err;
use crate::message::Message;
use crate::node::Node;
use crate::types::ThermalLoad;
use anyhow::Result;
use async_trait::async_trait;
use fuchsia_async::{self as fasync, DurationExt};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_inspect::{self as inspect, Property};
use futures::future::{FutureExt as _, LocalBoxFuture};
use futures::stream::FuturesUnordered;
use serde_derive::Deserialize;
use std::collections::HashMap;
use std::rc::Rc;
use {fidl_fuchsia_thermal as fthermal, serde_json as json};

/// Node: ThermalWatcher
///
/// Summary: Connects to the thermal client service to monitor their current thermal state. Relays
///          the thermal states to the thermal handler node.
///
/// Handles Messages: N/A
///
/// Sends Messages:
///     - UpdateThermalLoad
///
/// FIDL dependencies:
///     - fuchsia.thermal.ClientStateConnector: the node connects to the Power Manager's thermal
///       client service to retrieve their current thermal state using a hanging-get pattern.

pub struct ThermalWatcherBuilder<'a> {
    thermal_handler_node: Rc<dyn Node>,
    thermal_watcher: Option<fthermal::ClientStateWatcherProxy>,
    inspect_root: Option<&'a inspect::Node>,
}

impl<'a> ThermalWatcherBuilder<'a> {
    pub fn new_from_json(json_data: json::Value, nodes: &HashMap<String, Rc<dyn Node>>) -> Self {
        #[derive(Deserialize)]
        struct Dependencies {
            thermal_handler_node: String,
        }

        #[derive(Deserialize)]
        struct JsonData {
            dependencies: Dependencies,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        Self {
            thermal_handler_node: nodes[&data.dependencies.thermal_handler_node].clone(),
            thermal_watcher: None,
            inspect_root: None,
        }
    }

    #[cfg(test)]
    fn new(thermal_handler_node: Rc<dyn Node>) -> Self {
        Self { thermal_handler_node, thermal_watcher: None, inspect_root: None }
    }

    #[cfg(test)]
    fn with_inspect_root(mut self, root: &'a inspect::Node) -> Self {
        self.inspect_root = Some(root);
        self
    }

    #[cfg(test)]
    fn with_proxy(mut self, watcher: fthermal::ClientStateWatcherProxy) -> Self {
        self.thermal_watcher = Some(watcher);
        self
    }

    pub async fn build(
        self,
        futures_out: &FuturesUnordered<LocalBoxFuture<'_, ()>>,
    ) -> Result<Rc<ThermalWatcher>> {
        // Allow test to override
        let inspect_root =
            self.inspect_root.unwrap_or_else(|| inspect::component::inspector().root());
        let inspect = InspectData::new(inspect_root, "ThermalWatcher".to_string());

        // Allow test to override
        let thermal_watcher = if let Some(watcher) = self.thermal_watcher {
            watcher
        } else {
            // TODO(https://fxbug.dev/368440548): Remove delay when PowerManager can start early.
            fasync::Timer::new(zx::MonotonicDuration::from_seconds(15).after_now()).await;

            // This call will always succeed if the protocol is present in the component manifest
            // and the capability routes are correct.
            let connector = connect_to_protocol::<fthermal::ClientStateConnectorMarker>()?;
            let (watcher_proxy, watcher_service) =
                fidl::endpoints::create_proxy::<fthermal::ClientStateWatcherMarker>();
            connector.connect("CPU", watcher_service).expect("Failed to connect thermal client");

            watcher_proxy
        };

        let node = Rc::new(ThermalWatcher {
            thermal_watcher,
            thermal_handler_node: self.thermal_handler_node,
            inspect,
        });

        futures_out.push(node.clone().watch()?);

        Ok(node)
    }
}

pub struct ThermalWatcher {
    /// Watcher proxy to retrieve the current thermal state.
    thermal_watcher: fthermal::ClientStateWatcherProxy,

    /// Node that we send the UpdateThermalLoad message to once we retrieve the thermal states.
    thermal_handler_node: Rc<dyn Node>,

    inspect: InspectData,
}

impl ThermalWatcher {
    /// Watch the `fuchsia.thermal.ClientStateConnector` service to retrieve the current thermal
    /// state using a hanging-get pattern. When states are retrieved, a UpdateThermalLoad message
    /// is sent to `thermal_handler_node`. The method returns a Future that performs these steps in
    /// an infinite loop.
    fn watch<'a>(self: Rc<Self>) -> Result<LocalBoxFuture<'a, ()>> {
        Ok(async move {
            loop {
                match self.thermal_watcher.watch().await {
                    Ok(thermal_state) => {
                        self.inspect.set_thermal_state(thermal_state);
                        log_if_err!(
                            self.send_message(
                                &self.thermal_handler_node,
                                &Message::UpdateThermalLoad(ThermalLoad(thermal_state as u32)),
                            )
                            .await,
                            "Failed to send UpdateThermalLoad"
                        );
                    }
                    Err(e) => {
                        log::error!("Error while waiting for thermal state updates: {:?}", e);
                    }
                }
            }
        }
        .boxed_local())
    }
}

#[async_trait(?Send)]
impl Node for ThermalWatcher {
    fn name(&self) -> String {
        "ThermalWatcher".to_string()
    }
}

struct InspectData {
    thermal_state: inspect::UintProperty,
}

impl InspectData {
    fn new(parent: &inspect::Node, name: String) -> Self {
        let root = parent.create_child(name);
        let thermal_state = root.create_uint("thermal_state", 0);
        parent.record(root);
        Self { thermal_state }
    }

    fn set_thermal_state(&self, state: u64) {
        self.thermal_state.set(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNodeMaker};
    use crate::{msg_eq, msg_ok_return};
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use futures::{StreamExt, TryStreamExt};

    // A fake ThermalState provider service implementation for testing
    struct FakeThermalStateProvider {
        watcher_stream: fthermal::ClientStateWatcherRequestStream,
    }

    impl FakeThermalStateProvider {
        fn new() -> (fthermal::ClientStateWatcherProxy, Self) {
            let (watcher_proxy, watcher_stream) =
                fidl::endpoints::create_proxy_and_stream::<fthermal::ClientStateWatcherMarker>();

            (watcher_proxy, Self { watcher_stream })
        }

        // Send the thermal state update to the Watcher channel.
        async fn set_thermal_state(&mut self, state: u64) {
            match self
                .watcher_stream
                .try_next()
                .await
                .expect("Provider request stream yielded Some(None) (channel closed)")
                .expect("Provider request stream yielded Some(Err)")
            {
                fthermal::ClientStateWatcherRequest::Watch { responder } => {
                    responder.send(state).expect("failed to send power state");
                }
            }
        }
    }

    /// Tests for the presence and correctness of dynamically-added inspect data
    #[fuchsia::test]
    fn test_inspect_data() {
        let mut mock_maker = MockNodeMaker::new();
        // Create TestExecutor after MockNodeMaker so that all mock nodes are dropped When
        // MockNodeMaker goes out of scope.
        let mut exec = fasync::TestExecutor::new();

        let inspector = inspect::Inspector::default();
        let (watcher_proxy, mut fake_thermal_state_provider) = FakeThermalStateProvider::new();
        let futures_out = FuturesUnordered::new();
        let thermal_state_handler_node = mock_maker.make(
            "CpuManageHandler",
            vec![
                (msg_eq!(UpdateThermalLoad(ThermalLoad(3))), msg_ok_return!(UpdateThermalLoad)),
                (msg_eq!(UpdateThermalLoad(ThermalLoad(2))), msg_ok_return!(UpdateThermalLoad)),
            ],
        );

        let _node = exec.run_until_stalled(
            &mut ThermalWatcherBuilder::new(thermal_state_handler_node)
                .with_inspect_root(inspector.root())
                .with_proxy(watcher_proxy)
                .build(&futures_out)
                .boxed_local(),
        );
        let mut task = fasync::Task::local(futures_out.collect::<()>());
        // Waiting on the Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Send thermal state: 3
        assert_matches!(
            exec.run_until_stalled(
                &mut fake_thermal_state_provider.set_thermal_state(3_u64).boxed_local()
            ),
            Poll::Ready(())
        );
        // Waiting on the next Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        assert_data_tree!(
            @executor exec,
            inspector,
            root: {
                "ThermalWatcher": {
                    "thermal_state": 3_u64
                }
            }
        );

        // Send thermal state: 2
        assert_matches!(
            exec.run_until_stalled(
                &mut fake_thermal_state_provider.set_thermal_state(2_u64).boxed_local()
            ),
            Poll::Ready(())
        );
        // Waiting on the next Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);
        assert_data_tree!(
            @executor exec,
            inspector,
            root: {
                "ThermalWatcher": {
                    "thermal_state": 2_u64
                }
            }
        );
    }

    /// Tests that the ThermalWatcher relays UpdateThermalState messages to the thermal handler node
    /// when it retrieves the thermal state.
    #[fuchsia::test]
    fn test_state_monitor() {
        let mut mock_maker = MockNodeMaker::new();
        // Create TestExecutor after MockNodeMaker so that all mock nodes are dropped When
        // MockNodeMaker goes out of scope.
        let mut exec = fasync::TestExecutor::new();

        let thermal_state_handler_node = mock_maker.make(
            "CpuManageHandler",
            vec![
                (msg_eq!(UpdateThermalLoad(ThermalLoad(1))), msg_ok_return!(UpdateThermalLoad)),
                (msg_eq!(UpdateThermalLoad(ThermalLoad(2))), msg_ok_return!(UpdateThermalLoad)),
                (msg_eq!(UpdateThermalLoad(ThermalLoad(3))), msg_ok_return!(UpdateThermalLoad)),
                (msg_eq!(UpdateThermalLoad(ThermalLoad(2))), msg_ok_return!(UpdateThermalLoad)),
            ],
        );

        // Create the node.
        let (watcher_proxy, mut fake_thermal_state_provider) = FakeThermalStateProvider::new();
        let futures_out = FuturesUnordered::new();
        let _node = exec.run_until_stalled(
            &mut ThermalWatcherBuilder::new(thermal_state_handler_node)
                .with_proxy(watcher_proxy)
                .build(&futures_out)
                .boxed_local(),
        );
        let mut task = fasync::Task::local(futures_out.collect::<()>());
        // Waiting on the Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Send thermal state: 1
        assert_matches!(
            exec.run_until_stalled(
                &mut fake_thermal_state_provider.set_thermal_state(1_u64).boxed_local()
            ),
            Poll::Ready(())
        );
        // Waiting on the next Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Send thermal state: 2
        assert_matches!(
            exec.run_until_stalled(
                &mut fake_thermal_state_provider.set_thermal_state(2_u64).boxed_local()
            ),
            Poll::Ready(())
        );
        // Waiting on the next Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Send thermal state: 3
        assert_matches!(
            exec.run_until_stalled(
                &mut fake_thermal_state_provider.set_thermal_state(3_u64).boxed_local()
            ),
            Poll::Ready(())
        );
        // Waiting on the next Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Send thermal state: 2
        assert_matches!(
            exec.run_until_stalled(
                &mut fake_thermal_state_provider.set_thermal_state(2_u64).boxed_local()
            ),
            Poll::Ready(())
        );
        // Waiting on the next Watch request.
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);
    }
}
