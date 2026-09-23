// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_async as fasync;
use futures::channel::mpsc::{SendError, channel};
use futures::{Sink, SinkExt, Stream, StreamExt};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{CompletionType, Config, EditMode, Editor};
use std::thread;

use super::command_handler::CommandHandler;
use super::commands::CommandHelper;

const PROMPT: &str = "hf> ";

pub struct Runner {
    command_handler: CommandHandler,
}

impl Runner {
    pub fn new(command_handler: CommandHandler) -> Self {
        Self { command_handler }
    }

    // Runs the REPL.
    pub async fn run(mut self) -> Result<(), Error> {
        let (mut commands, mut acks) = Self::cmd_stream();

        while let Some(command) = commands.next().await {
            if self.handle_command(command).await.map_err(|e| {
                println!("Error: {e}");
                e
            })? {
                break;
            }

            // Allow the rustyline thread to continue
            acks.send(()).await?;
        }

        Ok(())
    }

    /// Generates a rustyline `Editor` in a separate thread to manage user input. This input is returned
    /// as a `Stream` of lines entered by the user. Since rustyline::Editor::readline blocks the thread,
    /// it needs to be run in its own separate thread from the executor running the FIDL operations.
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
                let c = CommandHelper::new();
                let mut rl: Editor<CommandHelper, DefaultHistory> = Editor::with_config(config)?;
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
                            println!("Error: {e:?}");
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

    async fn handle_command(&mut self, line: String) -> Result<bool, Error> {
        let mut components = line.trim().split_whitespace();
        let command = components.next().map(|c| c.parse());
        let args: Vec<&str> = components.collect();

        match { command } {
            Some(Ok(command)) => self.command_handler.handle_command(command, args).await,
            Some(Err(err)) => {
                println!("Unknown command: {err:?}");
                Ok(false)
            }
            None => Ok(false),
        }
    }
}
