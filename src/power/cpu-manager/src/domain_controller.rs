// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common_utils::result_debug_panic::ResultDebugPanic;
use crate::message::{Message, MessageReturn};
use crate::node::Node;
use crate::ok_or_default_err;
use anyhow::Error;
use async_trait::async_trait;
use fuchsia_component::server::{ServiceFs, ServiceFsDir, ServiceObjLocal};
use futures::{TryFutureExt, TryStreamExt};
use log::{error, info, warn};
use serde_derive::Deserialize;
use std::collections::HashMap;
use std::rc::Rc;
use {fidl_fuchsia_power_cpu as fcpu, fuchsia_async as fasync, serde_json as json};

/// Node: DomainController
///
/// Summary: Serves a FIDL interface that exposes power domain info and actions
///          via calls to CpuManagerMain.
///
/// Sends Messages:
///     - GetDomainInfos
///     - GetMaxFrequency
///     - SetMaxFrequency
///
/// FIDL dependencies: No direct dependencies
///
/// FIDL implementations:
///     - fuchsia.power.cpu.DomainController: the node implements this service to expose CPU domain
///       control to clients.

/// A builder for constructing the DomainController node.
#[derive(Default)]
pub struct DomainControllerBuilder<'a, 'b> {
    cpu_manager_main: Option<Rc<dyn Node>>,
    outgoing_svc_dir: Option<ServiceFsDir<'a, ServiceObjLocal<'b, ()>>>,
}

impl<'a, 'b> DomainControllerBuilder<'a, 'b> {
    #[cfg(test)]
    fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn with_cpu_manager_main(mut self, cpu_manager_main: Rc<dyn Node>) -> Self {
        self.cpu_manager_main = Some(cpu_manager_main);
        self
    }

    #[cfg(test)]
    pub fn with_outgoing_svc_dir(
        mut self,
        outgoing_svc_dir: ServiceFsDir<'a, ServiceObjLocal<'b, ()>>,
    ) -> Self {
        self.outgoing_svc_dir = Some(outgoing_svc_dir);
        self
    }

    pub fn new_from_json(
        json_data: json::Value,
        nodes: &HashMap<String, Rc<dyn Node>>,
        service_fs: &'a mut ServiceFs<ServiceObjLocal<'b, ()>>,
    ) -> Self {
        #[derive(Deserialize)]
        struct Dependencies {
            cpu_manager_main_node: String,
        }

        #[derive(Deserialize)]
        struct JsonData {
            dependencies: Dependencies,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        Self {
            cpu_manager_main: nodes.get(&data.dependencies.cpu_manager_main_node).cloned(),
            outgoing_svc_dir: Some(service_fs.dir("svc")),
        }
    }

    pub fn build(self) -> Result<Rc<DomainController>, Error> {
        let dc = Rc::new(DomainController {
            cpu_manager_main: self
                .cpu_manager_main
                .ok_or_else(|| anyhow::anyhow!("cpu_manager_main must be set"))?,
            domain_info_map: Rc::new(async_lock::OnceCell::new()),
            scope: Default::default(),
        });

        dc.publish_service(&mut ok_or_default_err!(self.outgoing_svc_dir).or_debug_panic()?);
        Ok(dc)
    }
}

pub struct DomainController {
    cpu_manager_main: Rc<dyn Node>,
    domain_info_map: Rc<async_lock::OnceCell<HashMap<u64, fcpu::DomainInfo>>>,
    scope: fasync::Scope,
}

impl DomainController {
    /// Publishes a service to expose sensor manager control of the Power Manager.
    fn publish_service<'a, 'b>(
        &self,
        outgoing_svc_dir: &mut ServiceFsDir<'a, ServiceObjLocal<'b, ()>>,
    ) {
        info!("Starting domain_controller service");
        let domain_info_map = self.domain_info_map.clone();
        let cpu_manager_main = self.cpu_manager_main.clone();
        let scope = self.scope.to_handle();

        outgoing_svc_dir.add_fidl_service(move |stream: fcpu::DomainControllerRequestStream| {
            let domain_info_map = domain_info_map.clone();
            let cpu_manager_main = cpu_manager_main.clone();
            scope.spawn_local(
                Self::handle_domain_controller_stream(stream, domain_info_map, cpu_manager_main)
                    .unwrap_or_else(|e: anyhow::Error| error!("{:?}", e)),
            );
        });
    }

    async fn handle_domain_controller_stream(
        mut stream: fcpu::DomainControllerRequestStream,
        domain_info_map: Rc<async_lock::OnceCell<HashMap<u64, fcpu::DomainInfo>>>,
        cpu_manager_main: Rc<dyn Node>,
    ) -> Result<(), Error> {
        let domain_info_map = domain_info_map.wait().await;
        while let Some(req) = stream.try_next().await? {
            match req {
                fcpu::DomainControllerRequest::ListDomains { responder } => {
                    let values: Vec<fcpu::DomainInfo> = domain_info_map.values().cloned().collect();
                    if let Err(e) = responder.send(&values) {
                        error!("Failed to send ListDomains response to client: {:?}", e);
                        continue;
                    }
                }
                fcpu::DomainControllerRequest::GetMaxFrequency { domain_id, responder } => {
                    if !domain_info_map.contains_key(&domain_id) {
                        let _ = responder.send(Err(fcpu::GetMaxFrequencyError::InvalidArguments));
                        continue;
                    }

                    let max_frequency_index = match cpu_manager_main
                        .handle_message(&Message::GetMaxFrequency(domain_id))
                        .await
                    {
                        Ok(MessageReturn::GetMaxFrequency(max_frequency_index)) => {
                            max_frequency_index
                        }
                        e => {
                            error!("Failed to get max frequency for domain {}: {:?}", domain_id, e);
                            let _ = responder.send(Err(fcpu::GetMaxFrequencyError::Internal));
                            continue;
                        }
                    };

                    if let Err(e) = responder.send(Ok(max_frequency_index as u64)) {
                        error!("Failed to send GetMaxFrequency response to client: {:?}", e);
                        continue;
                    }
                }
                fcpu::DomainControllerRequest::SetMaxFrequency {
                    domain_id,
                    frequency_index,
                    responder,
                } => {
                    if !domain_info_map.contains_key(&domain_id) {
                        let _ = responder.send(Err(fcpu::SetMaxFrequencyError::InvalidArguments));
                        continue;
                    }

                    match cpu_manager_main
                        .handle_message(&Message::SetMaxFrequency(domain_id, Some(frequency_index)))
                        .await
                    {
                        Ok(MessageReturn::SetMaxFrequency) => {
                            // MessageReturn::SetMaxFrequency is the expected message type here.
                            // Any other message type will be treated as an error.
                        }
                        e => {
                            error!("Failed to set max frequency for domain {}: {:?}", domain_id, e);
                            let _ = responder.send(Err(fcpu::SetMaxFrequencyError::Internal));
                            continue;
                        }
                    };

                    if let Err(e) = responder.send(Ok(())) {
                        error!("Failed to send SetMaxFrequency response to client: {:?}", e);
                        continue;
                    }
                }
                fcpu::DomainControllerRequest::ClearMaxFrequency { domain_id, responder } => {
                    if !domain_info_map.contains_key(&domain_id) {
                        let _ = responder.send(Err(fcpu::ClearMaxFrequencyError::InvalidArguments));
                        continue;
                    }

                    match cpu_manager_main
                        .handle_message(&Message::SetMaxFrequency(domain_id, None))
                        .await
                    {
                        Ok(MessageReturn::SetMaxFrequency) => {}
                        e => {
                            error!(
                                "Failed to clear max frequency for domain {}: {:?}",
                                domain_id, e
                            );
                            let _ = responder.send(Err(fcpu::ClearMaxFrequencyError::Internal));
                            continue;
                        }
                    };

                    if let Err(e) = responder.send(Ok(())) {
                        error!("Failed to send ClearMaxFrequency response to client: {:?}", e);
                        continue;
                    }
                }
                fcpu::DomainControllerRequest::_UnknownMethod { ordinal, .. } => {
                    warn!("Unknown DomainControllerRequest call: {}", ordinal);
                }
            }
        }
        Ok(())
    }
}

#[async_trait(?Send)]
impl Node for DomainController {
    fn name(&self) -> String {
        "DomainController".to_string()
    }

    async fn init(&self) -> Result<(), anyhow::Error> {
        match self.cpu_manager_main.handle_message(&Message::GetDomainInfos).await? {
            MessageReturn::GetDomainInfos(domain_infos) => {
                let domain_info_map = domain_infos
                    .into_iter()
                    .map(|info| match info.id {
                        Some(id) => Ok((id, info)),
                        None => Err(anyhow::anyhow!("Received DomainInfo without an id")),
                    })
                    .collect::<Result<_, _>>()?;
                let _ = self.domain_info_map.set(domain_info_map).await;
                Ok(())
            }
            e => Err(anyhow::anyhow!(
                "Failed to initialize DomainController, cannot get cpu cluster info: {:?}",
                e
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNodeMaker, create_dummy_node};
    use futures::StreamExt;

    /// Tests that well-formed configuration JSON does not panic the `new_from_json` function.
    #[test]
    fn test_new_from_json() {
        let json_data = json::json!({
            "type": "DomainController",
            "name": "domain_controller",
            "dependencies": {
                "cpu_manager_main_node": "cpu_manager_main",
            }
        });

        let mut nodes: HashMap<String, Rc<dyn Node>> = HashMap::new();
        nodes.insert("cpu_manager_main".to_string(), create_dummy_node());

        let _ =
            DomainControllerBuilder::new_from_json(json_data, &nodes, &mut ServiceFs::new_local());
    }

    /// Tests that the fuchsia.thermal.SensorManager server waits for node initialization before
    /// replying to clients.
    #[test]
    fn test_protocol_waits_on_init() {
        let mut mock_node_maker = MockNodeMaker::new();
        let mut executor = fasync::TestExecutor::new();
        let expected_domain_id = 0;
        let expected_max_frequency_index = 0;

        let mock_node = mock_node_maker.make(
            "cpu_manager_main",
            vec![
                (
                    MessageMatcher::Eq(Message::GetDomainInfos),
                    Ok(MessageReturn::GetDomainInfos(vec![fcpu::DomainInfo {
                        id: Some(0),
                        name: Some("cpu".to_string()),
                        core_ids: Some(vec![0]),
                        available_frequencies_hz: Some(vec![1000000000]),
                        ..Default::default()
                    }])),
                ),
                (
                    MessageMatcher::Eq(Message::GetMaxFrequency(expected_domain_id)),
                    Ok(MessageReturn::GetMaxFrequency(expected_max_frequency_index)),
                ),
            ],
        );

        let mut service_fs = ServiceFs::new_local();
        let domain_controller = DomainControllerBuilder::new()
            .with_cpu_manager_main(mock_node)
            .with_outgoing_svc_dir(service_fs.root_dir())
            .build()
            .unwrap();

        let connector = service_fs.create_protocol_connector().unwrap();
        fasync::Task::local(service_fs.collect()).detach();

        let domain_controller_proxy =
            connector.connect_to_protocol::<fcpu::DomainControllerMarker>().unwrap();

        let mut get_max_frequency_fut =
            domain_controller_proxy.get_max_frequency(expected_domain_id);
        assert!(executor.run_until_stalled(&mut get_max_frequency_fut).is_pending());

        executor.run_singlethreaded(domain_controller.init()).unwrap();
        let max_frequency_index =
            executor.run_singlethreaded(get_max_frequency_fut).unwrap().unwrap();
        assert_eq!(expected_max_frequency_index, max_frequency_index);
    }

    #[fuchsia::test]
    async fn test_calls_are_proxied_to_cpu_manager_main() {
        let mut mock_node_maker = MockNodeMaker::new();
        let expected_domain_id = 0;
        let default_max_frequency_index = 0;
        let new_max_frequency_index = 1;

        let expected_domain_infos = vec![fcpu::DomainInfo {
            id: Some(expected_domain_id),
            name: Some(format!("cluster{expected_domain_id}")),
            core_ids: Some(vec![0, 1]),
            available_frequencies_hz: Some(vec![2000000000, 1000000000]),
            ..Default::default()
        }];

        let mock_node = mock_node_maker.make(
            "cpu_manager_main",
            vec![
                (
                    MessageMatcher::Eq(Message::GetDomainInfos),
                    Ok(MessageReturn::GetDomainInfos(expected_domain_infos.clone())),
                ),
                (
                    MessageMatcher::Eq(Message::GetMaxFrequency(expected_domain_id)),
                    Ok(MessageReturn::GetMaxFrequency(default_max_frequency_index)),
                ),
                (
                    MessageMatcher::Eq(Message::SetMaxFrequency(
                        expected_domain_id,
                        Some(new_max_frequency_index),
                    )),
                    Ok(MessageReturn::SetMaxFrequency),
                ),
                (
                    MessageMatcher::Eq(Message::SetMaxFrequency(expected_domain_id, None)),
                    Ok(MessageReturn::SetMaxFrequency),
                ),
            ],
        );

        let mut service_fs = ServiceFs::new_local();
        let domain_controller = DomainControllerBuilder::new()
            .with_cpu_manager_main(mock_node)
            .with_outgoing_svc_dir(service_fs.root_dir())
            .build()
            .unwrap();
        domain_controller.init().await.unwrap();

        let connector = service_fs.create_protocol_connector().unwrap();
        let task = fasync::Task::local(service_fs.collect::<()>());

        // Scope connections to the fuchsia.thermal.SensorManager server so that spawned Tasks
        // complete before awaiting on no tasks.
        {
            let domain_controller_proxy =
                connector.connect_to_protocol::<fcpu::DomainControllerMarker>().unwrap();

            let domains = domain_controller_proxy.list_domains().await.unwrap();
            assert_eq!(expected_domain_infos, domains);

            assert_eq!(
                default_max_frequency_index,
                domain_controller_proxy
                    .get_max_frequency(expected_domain_id)
                    .await
                    .unwrap()
                    .unwrap()
            );
            domain_controller_proxy
                .set_max_frequency(expected_domain_id, new_max_frequency_index)
                .await
                .unwrap()
                .unwrap();
            domain_controller_proxy.clear_max_frequency(expected_domain_id).await.unwrap().unwrap();
        }

        // Tasks and connections must be explicitly aborted and dropped before MockNodeMaker goes
        // out of scope to prevent a panic from cpu_manager_main still being held by Tasks within
        // domain_controller's scope.
        domain_controller.scope.on_no_tasks().await;
        task.abort().await;
    }
}
