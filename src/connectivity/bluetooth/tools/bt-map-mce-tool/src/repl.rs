// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fuchsia_async as fasync;
use futures::channel::mpsc::{SendError, channel};
use futures::{Sink, SinkExt, Stream, StreamExt};
use log::{info, warn};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{CompletionType, Config, EditMode, Editor};
use std::thread;

use crate::accessor::*;
use crate::commands::*;

const PROMPT: &str = "MESSAGE_ACCESSOR> ";

// Starts the accessor REPL. This first requests a list of remote services and resolves the
// returned future with an error if no services are found.
pub async fn start_accessor_loop<'a>(client: AccessorClient) -> Result<(), Error> {
    let (mut commands, mut acks) = cmd_stream();
    while let Some(cmd) = commands.next().await {
        handle_cmd(cmd, client.clone()).await?;
        acks.send(()).await?;
    }

    Ok(())
}

// Generates a rustyline `Editor` in a separate thread to manage user input. This input is returned
/// as a `Stream` of lines entered by the user.
///
/// The thread exits and the `Stream` is exhausted when an error occurs on stdin or the user
/// sends a ctrl-c or ctrl-d sequence.
///
/// Because rustyline shares control over output to the screen with other parts of the system, a
/// `Sink` is passed to the caller to send acknowledgements that a command has been processed and
/// that rustyline should handle the next line of input.
fn cmd_stream() -> (impl Stream<Item = String>, impl Sink<(), Error = SendError>) {
    // Editor thread and command processing thread must be synchronized so that output
    // is printed in the correct order.
    let (mut cmd_sender, cmd_receiver) = channel(512);
    let (ack_sender, mut ack_receiver) = channel(512);

    let _ = thread::spawn(move || -> Result<(), Error> {
        let mut exec = fasync::LocalExecutorBuilder::new().build();

        let fut = async {
            let config = Config::builder()
                .auto_add_history(true)
                .history_ignore_space(true)
                .completion_type(CompletionType::List)
                .edit_mode(EditMode::Emacs)
                .build();
            let c = CmdHelper::new();
            let mut rl: Editor<CmdHelper, DefaultHistory> = Editor::with_config(config)?;
            rl.set_helper(Some(c));
            loop {
                let readline = rl.readline(PROMPT);
                match readline {
                    Ok(line) => {
                        cmd_sender.try_send(line)?;
                    }
                    Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => {
                        return Ok(());
                    }
                    Err(e) => {
                        warn!("Error: {:?}", e);
                        return Err(e.into());
                    }
                }
                // wait until processing thread is finished evaluating the last command
                // before running the next loop in the repl
                if ack_receiver.next().await.is_none() {
                    return Ok(());
                }
            }
        };
        exec.run_singlethreaded(fut)
    });
    (cmd_receiver, ack_sender)
}

// Processes `cmd` and returns its result.
async fn handle_cmd(line: String, client: AccessorClient) -> Result<(), Error> {
    let mut components = line.trim().split_whitespace();
    let cmd = components.next().map(|c| c.parse());
    let args: Vec<&str> = components.collect();

    match cmd {
        Some(Ok(Cmd::Exit)) => {
            return Err(format_err!("Terminating current Accessor FIDL client"));
        }
        Some(Ok(Cmd::Help)) => {
            info!("{}", Cmd::help_msg());
            Ok(())
        }
        Some(Ok(Cmd::ListAllMasInstances)) => list_all_mas_instances(client).await,
        Some(Ok(Cmd::TurnOnNotifications)) => register_for_notifications(&args, client).await,
        Some(Ok(Cmd::TurnOffNotifications)) => unregister_notifications(client).await,
        Some(Err(e)) => {
            info!("Unknown command: {:?}", e);
            Ok(())
        }
        None => Ok(()),
    }
}
