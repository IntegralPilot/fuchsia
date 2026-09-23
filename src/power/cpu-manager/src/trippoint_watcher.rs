// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log_if_err;
use crate::message::Message;
use crate::node::Node;
use anyhow::{Context, Result};
use async_trait::async_trait;
use fidl_fuchsia_hardware_trippoint as ftrippoint;
use fuchsia_component::client as fclient;
use futures::FutureExt;
use futures::future::LocalBoxFuture;
use futures::stream::FuturesUnordered;
use serde_derive::Deserialize;
use serde_json as json;
use std::collections::HashMap;
use std::rc::Rc;

/// Node: TrippointWatcher
///
/// Summary: Connects to a fuchsia.hardware.trippoint.TripPoint service to monitor trippoint events.
///          When a trippoint is triggered, it restricts the maximum operating point limit of a
///          CPU domain based on the provided configuration.
///
/// Handles Messages: N/A
///
/// Sends Messages:
///     - SetMaximumOperatingPointLimit
///
/// FIDL dependencies:
///     - fuchsia.hardware.trippoint: the node connects to this service to watch for LVL interrupts

pub struct TrippointWatcherBuilder {
    cpu_handler: Rc<dyn Node>,
    index_to_opp_map: HashMap<u32, u32>,
}

impl TrippointWatcherBuilder {
    pub fn new_from_json(json_data: json::Value, nodes: &HashMap<String, Rc<dyn Node>>) -> Self {
        #[derive(Deserialize)]
        struct Config {
            index_to_opp_map: HashMap<String, u32>,
        }

        #[derive(Deserialize)]
        struct Dependencies {
            cpu_handler_node: String,
        }

        #[derive(Deserialize)]
        struct JsonData {
            config: Config,
            dependencies: Dependencies,
        }

        let data: JsonData = json::from_value(json_data).unwrap();

        let index_to_opp_map = data
            .config
            .index_to_opp_map
            .into_iter()
            .map(|(k, v)| (k.parse::<u32>().expect("Invalid trippoint index"), v))
            .collect();

        Self { cpu_handler: nodes[&data.dependencies.cpu_handler_node].clone(), index_to_opp_map }
    }

    pub async fn build(
        self,
        futures_out: &FuturesUnordered<LocalBoxFuture<'_, ()>>,
    ) -> Result<Rc<TrippointWatcher>> {
        let proxy = fclient::Service::open(ftrippoint::ServiceMarker)
            .context("Failed to open service")?
            .watch_for_any()
            .await
            .context("Failed to find instance")?
            .connect_to_trippoint()
            .context("Failed to connect to trippoint protocol")?;

        let node = Rc::new(TrippointWatcher {
            proxy,
            cpu_handler: self.cpu_handler,
            index_to_opp_map: self.index_to_opp_map,
        });

        futures_out.push(node.clone().watch().boxed_local());

        Ok(node)
    }
}

pub struct TrippointWatcher {
    /// Proxy to the trippoint service.
    proxy: ftrippoint::TripPointProxy,

    /// Node to which we send the SetMaximumOperatingPointLimit message.
    cpu_handler: Rc<dyn Node>,

    /// Mapping from trippoint index to max operating point limit.
    index_to_opp_map: HashMap<u32, u32>,
}

impl TrippointWatcher {
    async fn watch(self: Rc<Self>) {
        loop {
            match self.proxy.wait_for_any_trip_point().await {
                Ok(Ok(result)) => {
                    if let Some(&opp) = self.index_to_opp_map.get(&result.index) {
                        log::info!(
                            "Trippoint triggered: index {}, setting max OPP limit to {}",
                            result.index,
                            opp
                        );
                        log_if_err!(
                            self.send_message(
                                &self.cpu_handler,
                                &Message::SetMaximumOperatingPointLimit(opp),
                            )
                            .await,
                            "Failed to send SetMaximumOperatingPointLimit"
                        );
                    } else {
                        log::warn!(
                            "Trippoint triggered for index {} but no mapping found in config",
                            result.index
                        );
                    }
                }
                Ok(Err(e)) => {
                    log::error!("Trippoint wait returned error: {}", zx::Status::err_from_raw(e));
                }
                Err(e) => {
                    log::error!("Error while waiting for trippoint updates: {:?}", e);
                    break;
                }
            }
        }
    }
}

#[async_trait(?Send)]
impl Node for TrippointWatcher {
    fn name(&self) -> String {
        "TrippointWatcher".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNodeMaker};
    use crate::{msg_eq, msg_ok_return};
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use futures::{StreamExt, TryStreamExt};

    // A fake Trippoint service implementation for testing
    struct FakeTrippoint {
        request_stream: ftrippoint::TripPointRequestStream,
    }

    impl FakeTrippoint {
        fn new() -> (ftrippoint::TripPointProxy, Self) {
            let (proxy, request_stream) =
                fidl::endpoints::create_proxy_and_stream::<ftrippoint::TripPointMarker>();

            (proxy, Self { request_stream })
        }

        async fn trigger_trippoint(&mut self, index: u32) {
            match self
                .request_stream
                .try_next()
                .await
                .expect("FakeTrippoint request stream yielded Some(None)")
                .expect("FakeTrippoint request stream yielded Some(Err)")
            {
                ftrippoint::TripPointRequest::WaitForAnyTripPoint { responder } => {
                    responder
                        .send(Ok(&ftrippoint::TripPointResult {
                            measured_temperature_celsius: 0.0,
                            index,
                        }))
                        .expect("failed to send trippoint result");
                }
                _ => panic!("Unexpected request"),
            }
        }
    }

    /// Tests that well-formed configuration JSON does not panic the `new_from_json` function.
    #[fuchsia::test]
    fn test_new_from_json() {
        let mut mock_maker = MockNodeMaker::new();
        let cpu_handler = mock_maker.make("CpuHandler", vec![]);
        let mut nodes = HashMap::new();
        nodes.insert("CpuHandler".to_string(), cpu_handler as Rc<dyn Node>);

        let json_data = json::json!({
            "config": {
                "index_to_opp_map": {
                    "0": 1,
                    "1": 2
                }
            },
            "dependencies": {
                "cpu_handler_node": "CpuHandler"
            }
        });

        let builder = TrippointWatcherBuilder::new_from_json(json_data, &nodes);
        assert_eq!(builder.index_to_opp_map.len(), 2);
        assert_eq!(builder.index_to_opp_map[&0], 1);
        assert_eq!(builder.index_to_opp_map[&1], 2);
    }

    /// Tests that different trippoint index produces expected SetMaximumOperatingPointLimit with
    /// corresponding opp.
    #[fuchsia::test]
    fn test_watch() {
        let mut mock_maker = MockNodeMaker::new();
        let mut exec = fasync::TestExecutor::new();

        let cpu_handler = mock_maker.make(
            "CpuHandler",
            vec![
                (
                    msg_eq!(SetMaximumOperatingPointLimit(1)),
                    msg_ok_return!(SetMaximumOperatingPointLimit),
                ),
                (
                    msg_eq!(SetMaximumOperatingPointLimit(2)),
                    msg_ok_return!(SetMaximumOperatingPointLimit),
                ),
            ],
        );
        let (proxy, mut fake_trippoint) = FakeTrippoint::new();

        let node = Rc::new(TrippointWatcher {
            proxy,
            cpu_handler: cpu_handler as Rc<dyn Node>,
            index_to_opp_map: [(0, 1), (1, 2)].iter().cloned().collect(),
        });

        let futures_out = FuturesUnordered::new();
        futures_out.push(node.clone().watch().boxed_local());
        let mut task = fasync::Task::local(futures_out.collect::<()>());

        // Initial stall - waiting for trippoint
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Trigger trippoint index 0
        assert_matches!(
            exec.run_until_stalled(&mut fake_trippoint.trigger_trippoint(0).boxed_local()),
            Poll::Ready(())
        );
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);

        // Trigger trippoint index 1
        assert_matches!(
            exec.run_until_stalled(&mut fake_trippoint.trigger_trippoint(1).boxed_local()),
            Poll::Ready(())
        );
        assert_matches!(exec.run_until_stalled(&mut task), Poll::Pending);
    }
}
