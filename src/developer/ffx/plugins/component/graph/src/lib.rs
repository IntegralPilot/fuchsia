// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug::cli::{GraphResult, graph_cmd};
use component_debug_fdomain as component_debug;
use errors::ffx_error;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_graph_args::ComponentGraphCommand;
use ffx_writer::MachineWriter;
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct GraphTool {
    #[command]
    cmd: ComponentGraphCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(GraphTool);

#[async_trait(?Send)]
impl FfxMain for GraphTool {
    type Writer = MachineWriter<GraphResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;

        // All errors from component_debug library are user-visible.
        let result = graph_cmd(self.cmd.filter, self.cmd.orientation, realm_query)
            .await
            .map_err(|e| ffx_error!(e))?;

        writer.machine_or(&result, &result)?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_client::fidl::ServerEnd;
    use fdomain_fuchsia_sys2 as fsys_f;
    use ffx_writer::TestBuffers;
    use futures::TryStreamExt;

    #[fuchsia::test]
    async fn test_graph() -> anyhow::Result<()> {
        let client = fdomain_local::local_client_empty();

        let mut capability_handlers = std::collections::HashMap::new();

        let client_clone = client.clone();
        capability_handlers.insert(
            "svc/fuchsia.sys2.RealmQuery.root".to_string(),
            Box::new(move |server_channel| {
                let client = client_clone.clone();
                let mut rq_stream =
                    ServerEnd::<fsys_f::RealmQueryMarker>::new(server_channel).into_stream();
                fuchsia_async::Task::local(async move {
                    while let Ok(Some(rq_req)) = rq_stream.try_next().await {
                        match rq_req {
                            fsys_f::RealmQueryRequest::GetAllInstances { responder } => {
                                let (client_end, mut iterator_stream) =
                                    client
                                        .create_request_stream::<fsys_f::InstanceIteratorMarker>();
                                fuchsia_async::Task::local(async move {
                                    if let Ok(Some(fsys_f::InstanceIteratorRequest::Next {
                                        responder,
                                    })) = iterator_stream.try_next().await
                                    {
                                        responder
                                            .send(&[
                                                fsys_f::Instance {
                                                    moniker: Some(".".to_string()),
                                                    url: Some("fuchsia-boot:///#meta/root.cm".to_string()),
                                                    resolved_info: Some(fsys_f::ResolvedInfo {
                                                        resolved_url: Some("fuchsia-boot:///#meta/root.cm".to_string()),
                                                        ..Default::default()
                                                    }),
                                                    ..Default::default()
                                                },
                                                fsys_f::Instance {
                                                    moniker: Some("core/test".to_string()),
                                                    url: Some("fuchsia-pkg://fuchsia.com/test#meta/test.cm".to_string()),
                                                    resolved_info: Some(fsys_f::ResolvedInfo {
                                                        resolved_url: Some("fuchsia-pkg://fuchsia.com/test#meta/test.cm".to_string()),
                                                        ..Default::default()
                                                    }),
                                                    ..Default::default()
                                                }
                                            ])
                                            .unwrap();
                                    }
                                    if let Ok(Some(fsys_f::InstanceIteratorRequest::Next {
                                        responder,
                                    })) = iterator_stream.try_next().await
                                    {
                                        responder.send(&[]).unwrap();
                                    }
                                })
                                .detach();
                                responder.send(Ok(client_end)).unwrap();
                            }
                            _ => {}
                        }
                    }
                })
                .detach();
            }) as Box<dyn Fn(fdomain_client::Channel) + 'static>,
        );

        let config = testing_lib::FakeRcsConfig {
            components: vec![],
            identify_host_handler: None,
            identify_host_response: None,
            capability_handlers,
        };

        let rcs = testing_lib::setup_fake_rcs(client.clone(), config);

        // Test non-machine mode (text dot representation)
        {
            let tool = GraphTool {
                cmd: ComponentGraphCommand {
                    filter: None,
                    orientation: component_debug::cli::GraphOrientation::TopToBottom,
                },
                rcs: rcs.clone().into(),
            };
            let buffers = TestBuffers::default();
            let writer = MachineWriter::new_test(None, &buffers);
            tool.main(writer).await.expect("tool failed");
            let output = buffers.into_stdout_str();
            assert!(output.contains("digraph {"));
            assert!(output.contains("\"core/test\""));
        }

        // Test machine mode (JSON format)
        {
            let tool = GraphTool {
                cmd: ComponentGraphCommand {
                    filter: None,
                    orientation: component_debug::cli::GraphOrientation::TopToBottom,
                },
                rcs: rcs.into(),
            };
            let buffers = TestBuffers::default();
            let writer = MachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
            tool.main(writer).await.expect("tool failed");
            let output = buffers.into_stdout_str();
            let parsed: serde_json::Value = serde_json::from_str(&output)?;
            assert!(parsed.is_object());
            assert!(parsed.get("instances").is_some());
            assert_eq!(parsed.get("orientation").and_then(|o| o.as_str()), Some("TopToBottom"));
        }

        Ok(())
    }
}
