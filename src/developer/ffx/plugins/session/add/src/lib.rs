// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_element::{
    self as felement, Annotation, AnnotationKey, AnnotationValue, ControllerMarker,
    ControllerProxy, ManagerProxy, Spec,
};
use ffx_session_add_args::SessionAddCommand;
use ffx_session_common::CommandStatus;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, Result, bug, user_error};
use futures::FutureExt;
use futures::channel::oneshot;
use signal_hook::consts::signal::*;
use signal_hook::iterator::Signals;
use std::future::Future;
use std::io::Write;
use target_holders::moniker;

#[derive(FfxTool)]
pub struct AddTool {
    #[command]
    cmd: SessionAddCommand,
    #[with(moniker("/core/session-manager"))]
    manager_proxy: ManagerProxy,
}

fho::embedded_plugin!(AddTool);

#[async_trait(?Send)]
impl FfxMain for AddTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        add_impl(self.manager_proxy, self.cmd, spawn_ctrl_c_listener(), &mut writer).await?;
        Ok(())
    }
}

pub async fn add_impl(
    manager_proxy: ManagerProxy,
    cmd: SessionAddCommand,
    ctrl_c_signal: impl Future<Output = ()>,
    writer: &mut VerifiedMachineWriter<CommandStatus>,
) -> Result<()> {
    if !writer.is_machine() {
        writeln!(writer, "Add {} to the current session", cmd.url)?;
    }
    let (_controller_client, controller_server) = if cmd.interactive {
        let (client, server) = manager_proxy.domain().create_endpoints::<ControllerMarker>();
        let client: ControllerProxy = client.into_proxy();
        (Some(client), Some(server))
    } else {
        (None, None)
    };

    let mut annotations = Vec::new();

    if cmd.persist {
        annotations.push(Annotation {
            key: AnnotationKey {
                namespace: felement::MANAGER_NAMESPACE.to_string(),
                value: felement::ANNOTATION_KEY_PERSIST_ELEMENT.to_string(),
            },
            value: AnnotationValue::Text(String::new()),
        });
    }

    if let Some(name) = cmd.name {
        annotations.push(Annotation {
            key: AnnotationKey {
                namespace: felement::MANAGER_NAMESPACE.to_string(),
                value: felement::ANNOTATION_KEY_NAME.to_string(),
            },
            value: AnnotationValue::Text(name),
        });
    }

    manager_proxy
        .propose_element(
            Spec {
                component_url: Some(cmd.url.to_string()),
                annotations: if annotations.is_empty() { None } else { Some(annotations) },
                ..Default::default()
            },
            controller_server,
        )
        .await
        .map_err(|err| bug!("Transport error proposing element: {err:?}"))?
        .map_err(|err| user_error!("Failed to propose element: {err:?}"))?;

    writer.machine(&CommandStatus::Ok { message: None })?;

    if cmd.interactive {
        // TODO(https://fxbug.dev/42058904) wait for either ctrl+c or the controller to close
        if !writer.is_machine() {
            writeln!(writer, "Waiting for Ctrl+C before terminating element...")?;
        }
        ctrl_c_signal.await;
    }

    Ok(())
}

/// Spawn a thread to listen for Ctrl+C, resolving the returned future when it's received.
fn spawn_ctrl_c_listener() -> impl Future<Output = ()> {
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).expect("must be able to create signal waiter");
        signals.forever().next().unwrap();
        sender.send(()).unwrap();
    });
    receiver.map(|_| ())
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;
    use assert_matches::assert_matches;
    use fdomain_fuchsia_element::{self as felement, ManagerRequest};
    use ffx_writer::{Format, TestBuffers};
    use futures::poll;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_add_element() {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { spec, responder, .. } => {
                assert_eq!(spec.component_url.unwrap(), TEST_ELEMENT_URL.to_string());
                let _ = responder.send(Ok(()));
            }
            ManagerRequest::RemoveElement { .. } => unreachable!(),
        });

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: false,
            persist: false,
            name: None,
        };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let response = add_impl(proxy, add_cmd, async {}.boxed(), &mut writer).await;
        assert!(response.is_ok());
    }

    #[fuchsia::test]
    async fn test_add_element_args() {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            ManagerRequest::RemoveElement { .. } => unreachable!(),
        });

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: false,
            persist: false,
            name: None,
        };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let response = add_impl(proxy, add_cmd, async {}.boxed(), &mut writer).await;
        assert!(response.is_ok());
    }

    #[fuchsia::test]
    async fn test_add_interactive_element_stop_with_ctrl_c() {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ManagerRequest::ProposeElement { responder, .. } => {
                responder.send(Ok(())).unwrap();
            }
            ManagerRequest::RemoveElement { .. } => unreachable!(),
        });

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: true,
            persist: false,
            name: None,
        };
        let (ctrl_c_sender, ctrl_c_receiver) = oneshot::channel();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let mut add_fut =
            Box::pin(add_impl(proxy, add_cmd, ctrl_c_receiver.map(|_| ()), &mut writer));

        assert!(poll!(&mut add_fut).is_pending(), "add should yield until ctrl+c");

        // Send ctrl+c so add will exit.
        ctrl_c_sender.send(()).unwrap();
        let result = add_fut.await;
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    async fn test_add_element_with_persist_and_name() {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { spec, responder, .. } => {
                assert_eq!(spec.component_url.as_ref().unwrap(), TEST_ELEMENT_URL);
                let mut got_name = false;
                let mut got_persist = false;
                if let Some(annotations) = spec.annotations.as_ref() {
                    for annotation in annotations {
                        assert_eq!(annotation.key.namespace, felement::MANAGER_NAMESPACE);
                        if annotation.key.value == felement::ANNOTATION_KEY_NAME {
                            assert_matches!(annotation.value,
                                            felement::AnnotationValue::Text(ref t) if t == "foo");
                            got_name = true;
                        } else if annotation.key.value == felement::ANNOTATION_KEY_PERSIST_ELEMENT {
                            assert_matches!(annotation.value,
                                            felement::AnnotationValue::Text(ref t) if t == "");
                            got_persist = true;
                        }
                    }
                }
                assert!(got_name && got_persist);
                let _ = responder.send(Ok(()));
            }
            ManagerRequest::RemoveElement { .. } => unreachable!(),
        });

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: false,
            persist: true,
            name: Some("foo".to_string()),
        };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let response = add_impl(proxy, add_cmd, async {}.boxed(), &mut writer).await;
        assert!(response.is_ok());
    }

    #[fuchsia::test]
    async fn test_add_element_machine() -> Result<()> {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { spec, responder, .. } => {
                assert_eq!(spec.component_url.unwrap(), TEST_ELEMENT_URL.to_string());
                let _ = responder.send(Ok(()));
            }
            ManagerRequest::RemoveElement { .. } => unreachable!(),
        });

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: false,
            persist: false,
            name: None,
        };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = add_impl(proxy, add_cmd, async {}.boxed(), &mut writer).await;
        assert!(response.is_ok());
        let output = test_buffers.into_stdout_str();
        let status: CommandStatus = serde_json::from_str(&output)?;
        assert_eq!(status, CommandStatus::Ok { message: None });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_add_element_error() -> Result<()> {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { responder, .. } => {
                let _ = responder.send(Err(felement::ManagerError::NotFound));
            }
            ManagerRequest::RemoveElement { .. } => unreachable!(),
        });

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: false,
            persist: false,
            name: None,
        };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = add_impl(proxy, add_cmd, async {}.boxed(), &mut writer).await;
        assert!(response.is_err());
        assert_eq!(response.unwrap_err().to_string(), "Failed to propose element: NotFound");
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_add_element_transport_error() -> Result<()> {
        const TEST_ELEMENT_URL: &str = "Test Element Url";

        let client = fdomain_local::local_client_empty();
        let (proxy, server) = client.create_proxy_and_stream::<felement::ManagerMarker>();
        drop(server);

        let add_cmd = SessionAddCommand {
            url: TEST_ELEMENT_URL.to_string(),
            interactive: false,
            persist: false,
            name: None,
        };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = add_impl(proxy, add_cmd, async {}.boxed(), &mut writer).await;
        assert!(response.is_err());
        assert!(response.unwrap_err().to_string().contains("Transport error proposing element"));
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
        Ok(())
    }
}
