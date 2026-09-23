// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::fdomain_fuchsia_bluetooth_pandora::{
    GrpcServerControllerProxy, GrpcServerControllerStartRequest, RootcanalClientControllerProxy,
    RootcanalClientControllerStartRequest, ServiceError,
};
use ::ffx_bluetooth_pandora_args::{PandoraCommand, PandoraSubCommand};
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use ffx_writer::{SimpleWriter, ToolIO as _};
use target_holders::toolbox;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct PandoraTool {
    #[command]
    cmd: PandoraCommand,
    #[with(toolbox())]
    grpc_server: GrpcServerControllerProxy,
    #[with(toolbox())]
    rootcanal: RootcanalClientControllerProxy,
}

fho::embedded_plugin!(PandoraTool);
#[async_trait(?Send)]
impl FfxMain for PandoraTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        match &self.cmd.subcommand {
            // ffx bluetooth pandora start
            PandoraSubCommand::Start(cmd) => {
                match self
                    .rootcanal
                    .start(&RootcanalClientControllerStartRequest {
                        ip: Some(cmd.rootcanal_ip.clone()),
                        port: Some(cmd.rootcanal_port),
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("error communicating with bt-rootcanal: {e}"))?
                {
                    Err(ServiceError::AlreadyRunning) => {
                        writer.line("already connected to Rootcanal server")?;
                    }
                    Err(ServiceError::InvalidIp) => {
                        return Err(fho::Error::User(anyhow::anyhow!(
                            "the Rootcanal IP address could not be parsed"
                        )));
                    }
                    Err(ServiceError::ConnectionFailed) => {
                        return Err(fho::Error::User(anyhow::anyhow!(
                            "a connection could not be established to the Rootcanal server"
                        )));
                    }
                    Err(ServiceError::Failed) => {
                        return Err(fho::Error::Unexpected(anyhow::anyhow!(
                            "unexpected error starting bt-rootcanal"
                        )));
                    }
                    _ => (),
                }

                match self
                    .grpc_server
                    .start(&GrpcServerControllerStartRequest {
                        port: Some(cmd.grpc_port),
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("error communicating with gRPC server: {e}"))?
                {
                    Err(ServiceError::AlreadyRunning) => {
                        writer.line("already running gRPC server")?;
                    }
                    Err(ServiceError::Failed) => {
                        return Err(fho::Error::Unexpected(anyhow::anyhow!(
                            "unexpected error starting gRPC server"
                        )));
                    }
                    _ => (),
                }
            }
            // ffx bluetooth pandora stop
            PandoraSubCommand::Stop(_) => {
                self.grpc_server
                    .stop()
                    .await
                    .map_err(|e| anyhow::anyhow!("error stopping gRPC server: {e}"))?;
                self.rootcanal
                    .stop()
                    .await
                    .map_err(|e| anyhow::anyhow!("error stopping bt-rootcanal: {e}"))?;
            }
        }

        Ok(())
    }
}
