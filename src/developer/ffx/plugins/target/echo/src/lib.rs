// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_target_echo_args::EchoCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::Serialize;
use target_connector::Connector;
use target_holders::RemoteControlProxyHolder;
use thiserror::Error;
use traceable_error::TraceableBox;
use traceable_error_derive::TraceableError;

/// Custom error type for the `ffx target echo` plugin.
///
/// We derive `TraceableError` here so that failures occurring within this tool
/// produce structured, deterministic layer codes (e.g., `ffx_target_echo::EchoError::Writer`).
/// This enables distributed diagnostic systems to trace the exact causal chain of failures
/// across multi-layered software boundaries without relying on fragile string parsing.
#[derive(Error, Debug, TraceableError)]
pub enum EchoError {
    #[error("Failed to establish connection to Remote Control Service: {0}")]
    TargetConnectionFailed(#[source] fho::Error),

    #[error("Echo capability failed: {0}")]
    #[trace(opaque)]
    EchoCapabilityFailed(#[from] fidl::Error),

    #[error("FFX Writer error: {0}")]
    #[trace(opaque)]
    Writer(#[from] ffx_writer::Error),
}

/// We implement `From<EchoError> for fho::Error` explicitly using `TraceableBox` rather
/// than relying on standard type-erasing conversions into `anyhow::Error` / `fho::Error`.
///
/// Converting a concrete tool error directly into untyped conduits (`anyhow::Error`) normally
/// creates an opaque tracing boundary, discarding structured layer code history. By wrapping
/// `EchoError` inside a `TraceableBox` before nesting it in `FfxError::Error` (with default exit code 1)
/// and `fho::Error`, the FHO execution framework and error loggers can successfully downcast through
/// the error chain and preserve the complete chronological trajectory of diagnostic layer codes.
impl From<EchoError> for fho::Error {
    fn from(e: EchoError) -> Self {
        fho::Error::from(anyhow::Error::from(errors::FfxError::Error(
            anyhow::Error::from(TraceableBox::from(e)),
            1,
        )))
    }
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EchoMessage {
    /// Message from the target
    Message(String),
    // Waiting on target
    Waiting(String),
    /// Unexpected error with string denoting error message.
    UnexpectedError(String),
}

#[derive(FfxTool)]
#[main_error(EchoError)]
pub struct EchoTool {
    #[command]
    cmd: EchoCommand,
    rcs_proxy: Connector<RemoteControlProxyHolder>,
}

fho::embedded_plugin!(EchoTool, EchoError);

#[async_trait(?Send)]
impl FfxMain for EchoTool {
    type Writer = VerifiedMachineWriter<EchoMessage>;
    type Error = EchoError;

    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        match echo_impl(self.rcs_proxy, self.cmd, &mut writer).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let error_msg = e.to_string();
                let _ = writer.machine(&EchoMessage::UnexpectedError(error_msg));
                Err(e)
            }
        }
    }
}

async fn echo_impl(
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    cmd: EchoCommand,
    writer: &mut VerifiedMachineWriter<EchoMessage>,
) -> Result<(), EchoError> {
    let echo_text = cmd.text.unwrap_or_else(|| "Ffx".to_string());
    // This outer loop retries connecting to the target every time the
    // connection fails. If we only connect once it only runs once.
    loop {
        // Get a connection to the target. If the target isn't there, this will
        // wait for it to appear. The closure is called just before that wait so
        // we can log what's happening before blocking a long time.
        //
        // If the daemon isn't available, we simply start it. If the daemon is
        // disabled from auto-starting with `daemon.autostart = false` then this
        // will still fail and exit the tool. Workflows that need tools to
        // auto-reconnect but still need to manually manage the daemon aren't
        // known to us at this time.
        //
        // Daemonless workflows should behave as though the daemon is always
        // reachable as far as this command is concerned, but daemonless is
        // experimental/unimplemented as of now so this isn't tested.
        let rcs_proxy = rcs_proxy_connector
            .try_connect(|target, connect_err| {
                let err_string = connect_err
                    .as_ref()
                    .map(|e| format!(". Error encountered: {e}"))
                    .unwrap_or_else(|| ".".to_owned());
                let message = if let Some(target) = &target {
                    format!("Waiting for target {target} to return{err_string}")
                } else {
                    format!("Waiting for target to return{err_string}")
                };
                writer
                    .machine_or(&EchoMessage::Waiting(message.clone()), message)
                    .map_err(|e| fho::Error::User(e.into()))?;
                Ok(())
            })
            .await
            .map_err(EchoError::TargetConnectionFailed)?;

        // This inner loop handles the repetition part of the --repeat argument.
        // If that argument wasn't specified then this too only runs once.
        loop {
            match rcs_proxy.echo_string(&echo_text).await {
                Ok(r) => {
                    let user_out = format!("SUCCESS: received {r:?}");
                    writer.machine_or(&EchoMessage::Message(r), user_out)?;
                }
                Err(e) => {
                    if cmd.repeat {
                        let message = format!("ERROR: {e:?}");
                        writer
                            .machine_or(&EchoMessage::UnexpectedError(message.clone()), &message)?;
                        break;
                    } else {
                        return Err(EchoError::EchoCapabilityFailed(e));
                    }
                }
            }

            if cmd.repeat {
                fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
            } else {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;
    use ffx_writer::{Format, TestBuffers};
    use fho::{FhoEnvironment, TryFromEnv};
    use fidl::endpoints::DiscoverableProtocolMarker;
    use serde_json::json;
    use std::sync::Arc;
    use target_behavior::ConnectionBehavior;

    fn setup_fake_client(
        rcs_handler: impl Fn(fidl_fuchsia_developer_remotecontrol::RemoteControlRequest)
        + Send
        + Sync
        + 'static,
    ) -> Arc<fdomain_client::Client> {
        let rcs_handler = Arc::new(rcs_handler);
        fdomain_local::local_client(move || {
            let (client_end, mut stream) =
                fidl::endpoints::create_request_stream::<fidl_fuchsia_io::DirectoryMarker>();
            let rcs_handler = Arc::clone(&rcs_handler);
            fuchsia_async::Task::local(async move {
                use futures::StreamExt;
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        fidl_fuchsia_io::DirectoryRequest::Open { path, object, .. } => {
                            if path == fidl_fuchsia_developer_remotecontrol::RemoteControlMarker::PROTOCOL_NAME {
                                let mut rcs_stream = fidl::endpoints::ServerEnd::<
                                    fidl_fuchsia_developer_remotecontrol::RemoteControlMarker,
                                >::new(object)
                                .into_stream();
                                let rcs_handler = Arc::clone(&rcs_handler);
                                fuchsia_async::Task::local(async move {
                                    use futures::TryStreamExt;
                                    while let Ok(Some(req)) = rcs_stream.try_next().await {
                                        rcs_handler(req);
                                    }
                                }).detach();
                            }
                        }
                        _ => {}
                    }
                }
            }).detach();
            Ok(client_end)
        })
    }

    async fn run_echo_test(cmd: EchoCommand) -> Result<String> {
        let client = setup_fake_client(|req| match req {
            fidl_fuchsia_developer_remotecontrol::RemoteControlRequest::EchoString {
                value,
                responder,
            } => {
                responder.send(&value).unwrap();
            }
            _ => panic!("unexpected request: {:?}", req),
        });

        let env = FhoEnvironment::new_with_args(
            &ffx_config::EnvironmentContext::no_context(
                ffx_config::environment::ExecutableKind::Test,
                Default::default(),
                None,
                true,
            )?,
            &["some", "test"],
        );
        let target_env = target_behavior::target_interface(&env);
        let behavior = ConnectionBehavior::fake_with_fdomain_client(client).await;
        target_env.set_behavior_for_test(behavior);

        let connector = Connector::try_from_env(&env).await.expect("Could not make test connector");
        let tool = EchoTool { cmd, rcs_proxy: connector };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<EchoMessage>::new_test(None, &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());
        Ok(buffers.into_stdout_str())
    }

    #[fuchsia::test]
    async fn test_echo_with_no_text() -> Result<()> {
        let cmd = EchoCommand { text: None, repeat: false };
        let output = run_echo_test(cmd).await?;
        assert_eq!("SUCCESS: received \"Ffx\"\n".to_string(), output);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_echo_with_text() -> Result<()> {
        let cmd = EchoCommand { text: Some("test".to_string()), repeat: false };
        let output = run_echo_test(cmd).await?;
        assert_eq!("SUCCESS: received \"test\"\n".to_string(), output);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_echo_with_machine() -> Result<()> {
        let client = setup_fake_client(|req| match req {
            fidl_fuchsia_developer_remotecontrol::RemoteControlRequest::EchoString {
                value,
                responder,
            } => {
                responder.send(&value).unwrap();
            }
            _ => panic!("unexpected request: {:?}", req),
        });

        let env = FhoEnvironment::new_with_args(
            &ffx_config::EnvironmentContext::no_context(
                ffx_config::environment::ExecutableKind::Test,
                Default::default(),
                None,
                true,
            )?,
            &["some", "test"],
        );
        let target_env = target_behavior::target_interface(&env);
        let behavior = ConnectionBehavior::fake_with_fdomain_client(client).await;
        target_env.set_behavior_for_test(behavior);
        let connector = Connector::try_from_env(&env).await.expect("Could not make test connector");
        let cmd = EchoCommand { text: Some("test".to_string()), repeat: false };
        let tool = EchoTool { cmd, rcs_proxy: connector };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<EchoMessage>::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = buffers.into_stdout_str();

        let err = format!("schema not valid {output}");
        let json = serde_json::from_str(&output).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        VerifiedMachineWriter::<EchoMessage>::verify_schema(&json).expect(&err);

        let want = EchoMessage::Message("test".into());
        assert_eq!(json, json!(want));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_echo_failure() -> Result<()> {
        let cmd = EchoCommand { text: None, repeat: false };
        let client = fdomain_local::local_client_empty();

        let env = FhoEnvironment::new_with_args(
            &ffx_config::EnvironmentContext::no_context(
                ffx_config::environment::ExecutableKind::Test,
                Default::default(),
                None,
                true,
            )?,
            &["some", "test"],
        );
        let target_env = target_behavior::target_interface(&env);
        let behavior = ConnectionBehavior::fake_with_fdomain_client(client).await;
        target_env.set_behavior_for_test(behavior);

        let connector = Connector::try_from_env(&env).await.expect("Could not make test connector");
        let tool = EchoTool { cmd, rcs_proxy: connector };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<EchoMessage>::new_test(None, &buffers);

        let result = tool.main(writer).await;

        match result {
            Err(EchoError::TargetConnectionFailed(_)) => {}
            _ => panic!("Expected Err(EchoError::TargetConnectionFailed), got {:?}", result),
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_echo_capability_failure() -> Result<()> {
        let cmd = EchoCommand { text: Some("test".to_string()), repeat: false };
        let client = setup_fake_client(|req| match req {
            fidl_fuchsia_developer_remotecontrol::RemoteControlRequest::EchoString {
                responder,
                ..
            } => {
                // Intentionally drop the responder to trigger ClientChannelClosed
                drop(responder);
            }
            _ => panic!("unexpected request: {:?}", req),
        });

        let env = FhoEnvironment::new_with_args(
            &ffx_config::EnvironmentContext::no_context(
                ffx_config::environment::ExecutableKind::Test,
                Default::default(),
                None,
                true,
            )?,
            &["some", "test"],
        );
        let target_env = target_behavior::target_interface(&env);
        let behavior = ConnectionBehavior::fake_with_fdomain_client(client).await;
        target_env.set_behavior_for_test(behavior);

        let connector = Connector::try_from_env(&env).await.expect("Could not make test connector");
        let tool = EchoTool { cmd, rcs_proxy: connector };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<EchoMessage>::new_test(None, &buffers);

        let result = tool.main(writer).await;

        match result {
            Err(EchoError::EchoCapabilityFailed(_)) => {}
            _ => panic!("Expected Err(EchoError::EchoCapabilityFailed), got {:?}", result),
        }
        Ok(())
    }
}
