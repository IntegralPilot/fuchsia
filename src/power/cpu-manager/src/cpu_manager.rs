// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::message::{Message, MessageReturn};
use crate::node::Node;
use anyhow::{Context, Error, format_err};
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter as _; // for `set_starting_up()`, etc.
use futures::future::{LocalBoxFuture, join_all};
use futures::stream::{FuturesUnordered, StreamExt, TryStreamExt};
use log::*;
use serde_json as json;
use std::collections::HashMap;
use std::rc::Rc;
use zx;

// nodes
use crate::{
    cpu_control_handler, cpu_device_handler, cpu_manager_main, cpu_stats_handler,
    cpu_stats_recorder, domain_controller, syscall_handler, thermal_watcher, trippoint_watcher,
};

pub struct CpuManager {
    nodes: HashMap<String, Rc<dyn Node>>,
}

impl CpuManager {
    pub fn new() -> Self {
        Self { nodes: HashMap::new() }
    }

    /// Perform the node initialization and begin running the CpuManager.
    pub async fn run(&mut self) -> Result<(), Error> {
        // Create a new ServiceFs to handle incoming service requests.
        let mut fs = ServiceFs::new_local();

        // Required call to serve the inspect tree
        let inspector = component::inspector();
        let _inspect_server_task =
            inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());
        component::health().set_starting_up();

        let structured_config = cpu_manager_config_lib::Config::take_from_startup_handle();
        structured_config.record_inspect(fuchsia_inspect::component::inspector().root());
        log_config(&structured_config);

        let node_futures = FuturesUnordered::new();
        self.create_nodes_from_config(&structured_config, &node_futures, &mut fs)
            .await
            .context("Failed to create nodes from config")?;

        let handler = self.nodes.get("cpu_manager_main").map(|n| n.clone());
        fs.dir("svc").add_fidl_service(move |stream: fcpumanager::BoostRequestStream| {
            let handler = handler.clone();
            fasync::Task::local(async move {
                if let Err(e) = Self::handle_boost_requests(
                    stream,
                    handler,
                    structured_config.cpu_boost_enabled,
                )
                .await
                {
                    log::error!("Error handling Manager requests: {}", e);
                }
            })
            .detach();
        });

        // Begin serving FIDL requests. It's important to do this after creating nodes but before
        // initializing them, since some nodes depend on incoming FIDL requests for their `init()`
        // process.
        fs.take_and_serve_directory_handle()?;

        let node_futures_task = fasync::Task::local(node_futures.collect::<()>());
        let service_fs_task = fasync::Task::local(fs.collect::<()>());

        match self.init_nodes().await {
            Ok(()) => component::health().set_ok(),
            Err(e) => {
                component::health().set_unhealthy(&format!("{e:?}"));
                return Err(e);
            }
        };

        info!("Setup complete");

        // Run the ServiceFs and node futures. This future never completes.
        futures::join!(service_fs_task, node_futures_task);

        Err(format_err!("Tasks completed unexpectedly"))
    }

    /// Create the nodes by reading and parsing the node config JSON file.
    async fn create_nodes_from_config(
        &mut self,
        structured_config: &cpu_manager_config_lib::Config,
        node_futures: &FuturesUnordered<LocalBoxFuture<'_, ()>>,
        service_fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>,
    ) -> Result<(), Error> {
        let node_config_path = &structured_config.node_config_path;
        let contents = std::fs::read_to_string(node_config_path)?;
        let json_data: json::Value = serde_json5::from_str(&contents)
            .context(format!("Failed to parse file {}", node_config_path))?;

        info!("Creating nodes from config file: {}", node_config_path);
        self.create_nodes(json_data, node_futures, service_fs).await
    }

    /// Creates the nodes using the specified JSON object, adding them to the `nodes` HashMap.
    async fn create_nodes(
        &mut self,
        json_data: json::Value,
        node_futures: &FuturesUnordered<LocalBoxFuture<'_, ()>>,
        service_fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>,
    ) -> Result<(), Error> {
        // Iterate through each object in the top-level array, which represents configuration for a
        // single node
        for node_config in json_data.as_array().unwrap().iter() {
            info!("Creating node {}", node_config["name"]);
            let node = self
                .create_node(node_config.clone(), node_futures, service_fs)
                .await
                .with_context(|| format!("Failed creating node {}", node_config["name"]))?;
            self.nodes.insert(node_config["name"].as_str().unwrap().to_string(), node);
        }
        Ok(())
    }

    /// Uses the supplied `json_data` to construct a single node, where `json_data` is the JSON
    /// object corresponding to a single node configuration.
    async fn create_node(
        &mut self,
        json_data: json::Value,
        node_futures: &FuturesUnordered<LocalBoxFuture<'_, ()>>,
        service_fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>,
    ) -> Result<Rc<dyn Node>, Error> {
        let node_name = json_data["name"].clone();
        let _log_warning_task = fasync::Task::local(async move {
            fasync::Timer::new(fasync::MonotonicDuration::from_seconds(30)).await;
            warn!("Creating {} not complete after 30s", node_name);
        });

        Ok(match json_data["type"].as_str().unwrap() {
            "ThermalWatcher" => {
                thermal_watcher::ThermalWatcherBuilder::new_from_json(json_data, &self.nodes)
                    .build(node_futures)
                    .await?
            }
            "TrippointWatcher" => {
                trippoint_watcher::TrippointWatcherBuilder::new_from_json(json_data, &self.nodes)
                    .build(node_futures)
                    .await?
            }
            "CpuControlHandler" => {
                cpu_control_handler::CpuControlHandlerBuilder::new_from_json(json_data, &self.nodes)
                    .build()?
            }
            "CpuDeviceHandler" => {
                cpu_device_handler::CpuDeviceHandlerBuilder::new_from_json(json_data, &self.nodes)
                    .build()?
            }
            "CpuManagerMain" => {
                cpu_manager_main::CpuManagerMainBuilder::new_from_json(json_data, &self.nodes)
                    .build()?
            }

            // TODO(fxbug.dev/42062455): Remove async node creation
            "CpuStatsHandler" => {
                cpu_stats_handler::CpuStatsHandlerBuilder::new_from_json(json_data, &self.nodes)
                    .build()
                    .await?
            }
            "CpuStatsRecorder" => {
                cpu_stats_recorder::CpuStatsRecorderBuilder::new_from_json(json_data, &self.nodes)
                    .build(node_futures)
                    .await?
            }
            "DomainController" => domain_controller::DomainControllerBuilder::new_from_json(
                json_data,
                &self.nodes,
                service_fs,
            )
            .build()?,

            // TODO(fxbug.dev/42062455): Remove async node creation
            "SyscallHandler" => {
                syscall_handler::SyscallHandlerBuilder::new_from_json(json_data).build().await?
            }

            unknown => panic!("Unknown node type: {}", unknown),
        })
    }

    async fn init_nodes(&self) -> Result<(), Error> {
        info!("Initializing nodes");

        join_all(self.nodes.iter().map(|node| async move {
            let node_name = node.0.clone();
            let _log_warning_task = fasync::Task::local(async move {
                fasync::Timer::new(fasync::MonotonicDuration::from_seconds(30)).await;
                warn!("Init {} not complete after 30s", node_name);
            });
            node.1.init().await.context(format!("Failed to init node: {}", node.0))
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map(|_| ())
    }

    async fn handle_boost_requests(
        mut stream: fcpumanager::BoostRequestStream,
        handler: Option<Rc<dyn Node>>,
        boost_supported: bool,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                fcpumanager::BoostRequest::Boost { responder } => {
                    if !boost_supported {
                        log::error!("Boost is not supported");
                        responder.send(Err(fcpumanager::BoostError::NotSupported))?;
                        continue;
                    }

                    let (token, remote_token) = zx::EventPair::create();
                    let koid = token.koid().unwrap().raw_koid();
                    let msg = Message::SetBoost(true, koid);

                    if let Some(handler) = &handler {
                        match handler.handle_message(&msg).await {
                            Ok(MessageReturn::SetBoost) => {
                                // success
                                let handler = handler.clone();
                                fasync::Task::local(async move {
                                    let _ = fasync::OnSignals::new(
                                        &token,
                                        zx::Signals::EVENTPAIR_PEER_CLOSED,
                                    )
                                    .await;
                                    let msg = Message::SetBoost(false, koid);
                                    if let Err(e) = handler.handle_message(&msg).await {
                                        log::error!(
                                            "Failed to disable boost after token drop: {:?}",
                                            e
                                        );
                                    }
                                })
                                .detach();

                                responder.send(Ok(remote_token))?;
                            }
                            res => {
                                log::error!("Failed to set boost: {:?}", res);
                                responder.send(Err(fcpumanager::BoostError::Internal))?;
                            }
                        }
                    } else {
                        log::error!("No handler for the manger fidl");
                        responder.send(Err(fcpumanager::BoostError::Internal))?;
                    }
                }
                fcpumanager::BoostRequest::_UnknownMethod { .. } => {
                    log::info!("Received an unknown method");
                }
            }
        }
        Ok(())
    }
}

fn log_config(config: &cpu_manager_config_lib::Config) {
    let cpu_manager_config_lib::Config { node_config_path, cpu_boost_enabled } = config;
    info!(
        "Configuration: node_config_path={}, cpu_boost_enabled={}",
        node_config_path, cpu_boost_enabled
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::format_err;
    use std::collections::HashSet;

    /// Tests that well-formed configuration JSON does not cause an unexpected panic in the
    /// `create_nodes` function. With this test JSON, we expect a panic with the message stated
    /// below indicating a node with the given type doesn't exist. By this point the JSON parsing
    /// will have already been validated.
    #[fuchsia::test]
    #[should_panic(expected = "Unknown node type: test_type")]
    async fn test_create_nodes() {
        let json_data = json::json!([
            {
                "type": "test_type",
                "name": "test_name"
            },
        ]);
        let mut cpu_manager = CpuManager::new();
        let node_futures = FuturesUnordered::new();
        let mut fs = ServiceFs::new_local();
        cpu_manager.create_nodes(json_data, &node_futures, &mut fs).await.unwrap();
    }

    /// Tests that all nodes in a given config file have a unique name.
    #[fuchsia::test]
    fn test_config_file_unique_names() -> Result<(), anyhow::Error> {
        crate::common_utils::test_each_node_config_file(|config_file| {
            let mut set = HashSet::new();
            for node in config_file {
                let node_name = node["name"].as_str().unwrap().to_string();
                if set.contains(&node_name) {
                    return Err(format_err!("Node with name {} already specified", node_name));
                }

                set.insert(node_name);
            }

            Ok(())
        })
    }

    /// Tests each node config file for correct node dependency ordering. The test expects a node's
    /// dependencies to be listed under a "dependencies" object as an array or nested object.
    ///
    /// For each node (`dependent_node`) in the config file, the test ensures that any node
    /// specified within that node config's "dependencies" object (`required_node_name`) occurs in
    /// the node config file at a position before the dependent node.
    #[fuchsia::test]
    fn test_each_node_config_file_dependency_ordering() -> Result<(), anyhow::Error> {
        fn to_string(v: &serde_json::Value) -> String {
            v.as_str().unwrap().to_string()
        }

        // Flattens the provided JSON value to extract all child strings. This is used to extract
        // node dependency names from a node config's "dependencies" object even for nodes with a
        // more complex format.
        fn flatten_node_names(obj: &serde_json::Value) -> Vec<String> {
            use serde_json::Value;
            match obj {
                Value::String(s) => vec![s.to_string()],
                Value::Array(arr) => arr.iter().map(|v| flatten_node_names(v)).flatten().collect(),
                Value::Object(obj) => {
                    obj.values().map(|v| flatten_node_names(v)).flatten().collect()
                }
                e => panic!("Invalid JSON type in dependency object: {:?}", e),
            }
        }

        crate::common_utils::test_each_node_config_file(|config_file| {
            for (dependent_idx, dependent_node) in config_file.iter().enumerate() {
                if let Some(dependencies_obj) = dependent_node.get("dependencies") {
                    for required_node_name in flatten_node_names(dependencies_obj) {
                        let dependent_node_name = to_string(&dependent_node["name"]);
                        let required_node_index = config_file
                            .iter()
                            .position(|n| to_string(&n["name"]) == required_node_name);
                        match required_node_index {
                            Some(found_at_index) if found_at_index > dependent_idx => {
                                return Err(anyhow::format_err!(
                                    "Dependency {} must be specified before node {}",
                                    required_node_name,
                                    dependent_node_name
                                ));
                            }
                            Some(found_at_index) if found_at_index == dependent_idx => {
                                return Err(anyhow::format_err!(
                                    "Invalid to specify self as dependency for node {}",
                                    dependent_node_name
                                ));
                            }
                            None => {
                                return Err(anyhow::format_err!(
                                    "Missing dependency {} for node {}",
                                    required_node_name,
                                    dependent_node_name
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }

            Ok(())
        })
    }

    /// Tests that if any node's init() fails, then the `init_nodes()` function returns an error.
    #[fuchsia::test]
    async fn test_init_failure() {
        use async_trait::async_trait;

        struct InitSuccessNode;
        #[async_trait(?Send)]
        impl Node for InitSuccessNode {
            fn name(&self) -> String {
                "InitSuccessNode".to_string()
            }
        }

        struct InitFailureNode;
        #[async_trait(?Send)]
        impl Node for InitFailureNode {
            fn name(&self) -> String {
                "InitFailureNode".to_string()
            }

            async fn init(&self) -> Result<(), Error> {
                Err(format_err!("Init failure"))
            }
        }

        let success_node = Rc::new(InitSuccessNode {});
        let failure_node = Rc::new(InitFailureNode {});

        let cpu_manager = CpuManager {
            nodes: HashMap::from([
                ("init_success_node".into(), success_node as Rc<dyn Node>),
                ("init_failure_node".into(), failure_node as Rc<dyn Node>),
            ]),
        };

        assert!(cpu_manager.init_nodes().await.is_err());
    }
}
