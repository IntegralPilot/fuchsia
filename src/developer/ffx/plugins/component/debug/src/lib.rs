// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use component_debug::query::get_single_instance_from_query;
use component_debug::realm::{Runtime, get_runtime};
use component_debug_fdomain as component_debug;
use errors::ffx_error;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_debug_args::ComponentDebugCommand;
use ffx_config::EnvironmentContext;
use ffx_writer::{MachineWriter, ToolIO};
use ffx_zxdb::Debugger;
use fho::{FfxMain, FfxTool, deferred};
use serde::Serialize;
use target_holders::{RemoteControlProxyHolder, moniker};
use zx_types::zx_koid_t;

#[derive(Serialize)]
pub struct DebugResult {
    pub moniker: String,
    pub job_koid: u64,
}

#[derive(FfxTool)]
pub struct DebugTool {
    #[command]
    cmd: ComponentDebugCommand,

    #[with(deferred(moniker("/core/debugger")))]
    debugger_proxy: fho::Deferred<fdomain_fuchsia_debugger::LauncherProxy>,

    rcs: RemoteControlProxyHolder,

    context: EnvironmentContext,
}

fho::embedded_plugin!(DebugTool);

async fn get_job_koid(runtime: &Runtime) -> Result<zx_koid_t> {
    match runtime {
        Runtime::Elf { job_id, .. } => Ok(*job_id),
        Runtime::Unknown => Err(ffx_error!("Cannot debug non-ELF component.").into()),
    }
}

#[async_trait(?Send)]
impl FfxMain for DebugTool {
    type Writer = MachineWriter<DebugResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;

        let instance = get_single_instance_from_query(&self.cmd.query, &realm_query)
            .await
            .map_err(|e| ffx_error!(e))?;
        let runtime =
            get_runtime(&instance.moniker, &realm_query).await.unwrap_or(Runtime::Unknown);
        let job_koid = get_job_koid(&runtime).await?;

        // If machine mode is requested, we print the debug metadata (moniker and job KOID)
        // and exit immediately. This allows external tools/scripts to query this information
        // without launching the interactive debugger.
        if writer.is_machine() {
            writer.machine(&DebugResult { moniker: instance.moniker.to_string(), job_koid })?;
        } else {
            let mut debugger = Debugger::launch(&self.context, self.debugger_proxy.await?).await?;
            debugger.command.attach_job_koid(job_koid);
            debugger.run().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_client::fidl::ServerEnd;
    use fdomain_fuchsia_io as fio_f;
    use fdomain_fuchsia_sys2 as fsys_f;
    use ffx_writer::TestBuffers;
    use futures::TryStreamExt;
    use vfs::execution_scope::ExecutionScope;

    #[fuchsia::test]
    async fn test_debug_machine_fails_non_elf() -> anyhow::Result<()> {
        let client = fdomain_local::local_client_empty();
        let scope = fuchsia_async::Scope::new();
        let scope_handle = scope.to_handle();

        let mut capability_handlers = std::collections::HashMap::new();

        let client_clone = client.clone();
        capability_handlers.insert(
            "svc/fuchsia.sys2.RealmQuery.root".to_string(),
            Box::new(move |server_channel| {
                let client = client_clone.clone();
                let scope_handle = scope_handle.clone();
                let mut rq_stream =
                    ServerEnd::<fsys_f::RealmQueryMarker>::new(server_channel).into_stream();
                let scope_handle_for_task = scope_handle.clone();
                scope_handle.spawn_local(async move {
                    let scope_handle = scope_handle_for_task;
                    while let Ok(Some(rq_req)) = rq_stream.try_next().await {
                        match rq_req {
                            fsys_f::RealmQueryRequest::GetAllInstances { responder } => {
                                let (client_end, mut iterator_stream) =
                                    client
                                        .create_request_stream::<fsys_f::InstanceIteratorMarker>();
                                scope_handle.spawn_local(async move {
                                    if let Ok(Some(fsys_f::InstanceIteratorRequest::Next {
                                        responder,
                                    })) = iterator_stream.try_next().await
                                    {
                                        responder
                                            .send(&[fsys_f::Instance {
                                                moniker: Some("core/test".to_string()),
                                                url: Some(
                                                    "fuchsia-pkg://fuchsia.com/test#meta/test.cm"
                                                        .to_string(),
                                                ),
                                                ..Default::default()
                                            }])
                                            .unwrap();
                                    }
                                    if let Ok(Some(fsys_f::InstanceIteratorRequest::Next {
                                        responder,
                                    })) = iterator_stream.try_next().await
                                    {
                                        responder.send(&[]).unwrap();
                                    }
                                });
                                responder.send(Ok(client_end)).unwrap();
                            }
                            fsys_f::RealmQueryRequest::OpenDirectory { responder, .. } => {
                                responder.send(Err(fsys_f::OpenError::NoSuchDir)).unwrap();
                            }
                            _ => {}
                        }
                    }
                });
            }) as Box<dyn Fn(fdomain_client::Channel) + 'static>,
        );

        let config = testing_lib::FakeRcsConfig {
            components: vec![],
            identify_host_handler: None,
            identify_host_response: None,
            capability_handlers,
        };

        let rcs = testing_lib::setup_fake_rcs(client.clone(), config);

        let context = ffx_config::EnvironmentContext::no_context(
            ffx_config::environment::ExecutableKind::Test,
            Default::default(),
            None,
            true,
        )?;

        let tool = DebugTool {
            cmd: ComponentDebugCommand { query: "core/test".to_string() },
            debugger_proxy: fho::Deferred::from_output(Err(fho::Error::User(anyhow::anyhow!(
                "should not be resolved"
            )))),
            rcs: rcs.into(),
            context,
        };
        let buffers = TestBuffers::default();
        let writer = MachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        let res = tool.main(writer).await;
        // Since get_runtime returned Runtime::Unknown, get_job_koid should return Err.
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("Cannot debug non-ELF component."));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_debug_machine_success() -> anyhow::Result<()> {
        let client = fdomain_local::local_client_empty();
        let scope = fuchsia_async::Scope::new();
        let scope_handle = scope.to_handle();

        let mut capability_handlers = std::collections::HashMap::new();

        let client_clone = client.clone();
        let scope_handle_clone = scope_handle.clone();
        capability_handlers.insert(
            "svc/fuchsia.sys2.RealmQuery.root".to_string(),
            Box::new(move |server_channel| {
                let client = client_clone.clone();
                let scope_handle = scope_handle_clone.clone();
                let mut rq_stream =
                    ServerEnd::<fsys_f::RealmQueryMarker>::new(server_channel).into_stream();
                let scope_handle_for_task = scope_handle.clone();
                scope_handle.spawn_local(async move {
                    let scope_handle = scope_handle_for_task;
                    let client = client.clone();
                    while let Ok(Some(rq_req)) = rq_stream.try_next().await {
                        match rq_req {
                            fsys_f::RealmQueryRequest::GetAllInstances { responder } => {
                                let (client_end, mut iterator_stream) =
                                    client
                                        .create_request_stream::<fsys_f::InstanceIteratorMarker>();
                                scope_handle.spawn_local(async move {
                                    if let Ok(Some(fsys_f::InstanceIteratorRequest::Next {
                                        responder,
                                    })) = iterator_stream.try_next().await
                                    {
                                        responder
                                            .send(&[fsys_f::Instance {
                                                moniker: Some("core/test".to_string()),
                                                url: Some(
                                                    "fuchsia-pkg://fuchsia.com/test#meta/test.cm"
                                                        .to_string(),
                                                ),
                                                ..Default::default()
                                            }])
                                            .unwrap();
                                    }
                                    if let Ok(Some(fsys_f::InstanceIteratorRequest::Next {
                                        responder,
                                    })) = iterator_stream.try_next().await
                                    {
                                        responder.send(&[]).unwrap();
                                    }
                                });
                                responder.send(Ok(client_end)).unwrap();
                            }
                            fsys_f::RealmQueryRequest::OpenDirectory {
                                moniker,
                                dir_type,
                                object,
                                responder,
                            } => {
                                assert_eq!(moniker, "core/test");
                                assert_eq!(dir_type, fsys_f::OpenDirType::RuntimeDir);

                                let runtime_dir = vfs::pseudo_directory! {
                                    "elf" => vfs::pseudo_directory! {
                                        "job_id" => vfs::file::read_only("12345"),
                                    },
                                };
                                vfs::directory::serve_on(
                                    runtime_dir,
                                    fio_f::PERM_READABLE,
                                    ExecutionScope::new(client.clone()),
                                    object,
                                );
                                responder.send(Ok(())).unwrap();
                            }
                            _ => {}
                        }
                    }
                });
            }) as Box<dyn Fn(fdomain_client::Channel) + 'static>,
        );

        let config = testing_lib::FakeRcsConfig {
            components: vec![],
            identify_host_handler: None,
            identify_host_response: None,
            capability_handlers,
        };

        let rcs = testing_lib::setup_fake_rcs(client.clone(), config);

        let context = ffx_config::EnvironmentContext::no_context(
            ffx_config::environment::ExecutableKind::Test,
            Default::default(),
            None,
            true,
        )?;

        let tool = DebugTool {
            cmd: ComponentDebugCommand { query: "core/test".to_string() },
            debugger_proxy: fho::Deferred::from_output(Err(fho::Error::User(anyhow::anyhow!(
                "should not be resolved"
            )))),
            rcs: rcs.into(),
            context,
        };
        let buffers = TestBuffers::default();
        let writer = MachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        tool.main(writer).await.expect("tool failed");

        let output = buffers.into_stdout_str();
        let parsed: serde_json::Value = serde_json::from_str(&output)?;
        assert_eq!(parsed.get("moniker").and_then(|m| m.as_str()), Some("core/test"));
        assert_eq!(parsed.get("job_koid").and_then(|k| k.as_u64()), Some(12345));

        Ok(())
    }
}
