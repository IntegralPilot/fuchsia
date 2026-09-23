// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Result, anyhow};
use args::DebugCommand;
use flex_fuchsia_power_manager_debug as fdebug;

pub async fn debugcmd(cmd: DebugCommand, proxy: fdebug::DebugProxy) -> Result<()> {
    proxy
        .message(&cmd.node_name, &cmd.command, &cmd.args)
        .await
        .map_err(|err| anyhow!("Failed to call Debug/Message: {}", err))?
        .map_err(|e| match e {
            fdebug::MessageError::Generic => anyhow!("Generic error occurred"),
            fdebug::MessageError::InvalidNodeName => {
                anyhow!("Invalid node name '{}'", cmd.node_name)
            }
            fdebug::MessageError::UnsupportedCommand => {
                anyhow!("Unsupported command '{}' for node '{}'", cmd.command, cmd.node_name)
            }
            fdebug::MessageError::InvalidCommandArgs => {
                anyhow!("Invalid arguments for command '{}'", cmd.command)
            }
            e => anyhow!("Unknown error: {:?}", e),
        })?;
    Ok(())
}

#[cfg(feature = "fdomain")]
#[cfg(test)]
mod tests {
    use target_holders::fake_proxy;

    use super::*;

    #[fuchsia::test]
    async fn test_debugcmd() {
        let client = fdomain_local::local_client_empty();
        let command_request = DebugCommand {
            node_name: "test_node_name".to_string(),
            command: "test_command".to_string(),
            args: vec!["test_arg_1".to_string(), "test_arg_2".to_string()],
        };

        let debug_proxy = fake_proxy(std::sync::Arc::clone(&client), move |req| match req {
            fdebug::DebugRequest::Message { node_name, command, args, responder, .. } => {
                assert_eq!(node_name, "test_node_name");
                assert_eq!(command, "test_command");
                assert_eq!(args, vec!["test_arg_1", "test_arg_2"]);
                let _ = responder.send(Ok(()));
            }
        });

        debugcmd(command_request, debug_proxy).await.unwrap();
    }
}
