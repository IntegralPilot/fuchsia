// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use bt_common::Uuid;
use fuchsia_async as fasync;
use futures::channel::mpsc::{SendError, channel};
use futures::{Sink, SinkExt, Stream, StreamExt};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{CompletionType, Config, EditMode, Editor};
use std::thread;

use super::{
    GattClient,
    GattClientPtr,
    commands::{Cmd, CmdHelper},
    do_connect,
    //  do_read_by_type,
    do_disable_notify,
    do_enable_notify,
    do_find_services,
    do_list,
    do_pacs,
    do_read_chr,
    do_read_desc,
    do_read_long_chr,
    do_read_long_desc,
    do_vcs,
    do_write_chr,
    do_write_desc,
    do_write_long_chr,
    do_write_long_desc,
};

const PROMPT: &str = "GATT> ";
// Escape codes:
/// Clear the pty line on which the cursor is located.
/// Used when evented output is intermingled with the REPL prompt.
pub static CLEAR_LINE: &str = "\x1b[2K";
/// Move cursor to column 1
pub static CHA: &str = "\x1b[1G";

// Starts the GATT REPL. This first requests a list of remote services and resolves the
// returned future with an error if no services are found.
pub async fn start_gatt_loop<'a, T: bt_gatt::GattTypes>(
    client: T::Client,
    service: Option<Uuid>,
) -> Result<(), Error>
where
    <T as bt_gatt::GattTypes>::Client: Clone,
{
    let client = GattClient::<T>::new(client);

    if let Some(uuid) = service {
        println!("  discovering service {}...", uuid.recognize());
        client.find_service(uuid).await;
    }

    let (mut commands, mut acks) = cmd_stream();
    while let Some(cmd) = commands.next().await {
        handle_cmd(cmd, &client).await.map_err(|e| {
            println!("Error: {}", e);
            e
        })?;
        acks.send(()).await?;
    }

    Ok(())
}

/// Generates a rustyline `Editor` in a separate thread to manage user input. This input is returned
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
                        println!("Error: {:?}", e);
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
async fn handle_cmd<T: bt_gatt::GattTypes>(
    line: String,
    client: &GattClientPtr<T>,
) -> Result<(), Error> {
    let mut components = line.trim().split_whitespace();
    let cmd = components.next().map(|c| c.parse());
    let args: Vec<&str> = components.collect();

    let args_str: Vec<String> = args.iter().map(|s| String::from(*s)).collect();

    match cmd {
        Some(Ok(Cmd::Exit)) | Some(Ok(Cmd::Quit)) => {
            return Err(format_err!("exited").into());
        }
        Some(Ok(Cmd::Help)) => {
            print!("{}", Cmd::help_msg());
            Ok(())
        }
        Some(Ok(Cmd::List)) => {
            do_list(&args, client);
            Ok(())
        }
        Some(Ok(Cmd::Find)) => {
            do_find_services(&args, client).await;
            Ok(())
        }
        Some(Ok(Cmd::Connect)) => do_connect(&args, client).await,
        Some(Ok(Cmd::ReadChr)) => do_read_chr(&args, client).await,
        Some(Ok(Cmd::ReadLongChr)) => do_read_long_chr(&args, client).await,
        Some(Ok(Cmd::WriteChr)) => do_write_chr(args, client).await,
        Some(Ok(Cmd::WriteLongChr)) => do_write_long_chr(args, client).await,
        Some(Ok(Cmd::ReadDesc)) => do_read_desc(&args, client).await,
        Some(Ok(Cmd::ReadLongDesc)) => do_read_long_desc(&args, client).await,
        Some(Ok(Cmd::WriteDesc)) => do_write_desc(args, client).await,
        Some(Ok(Cmd::WriteLongDesc)) => do_write_long_desc(&args, client).await,
        Some(Ok(Cmd::EnableNotify)) => do_enable_notify(&args, client).await,
        Some(Ok(Cmd::DisableNotify)) => do_disable_notify(&args, client).await,
        Some(Ok(Cmd::Pacs)) => do_pacs(args_str, client.clone()).await,
        Some(Ok(Cmd::Vol)) => do_vcs(args_str, client.clone()).await,
        Some(Err(e)) => {
            eprintln!("Unknown command: {:?}", e);
            Ok(())
        }
        None => Ok(()),
        Some(Ok(unhandled)) => {
            println!("Not implemented: {unhandled:?}");
            Ok(())
        }
    }
}
