// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::context::LowpanCtlContext;
use crate::prelude::*;
use futures::channel::mpsc::channel;
use futures::task::Poll;
use rustyline::{DefaultEditor, error::ReadlineError}; //CompletionType, Config, EditMode, Editor};
use std::pin::Pin;

/// Contains the arguments decoded for the `otcli` command.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "otcli")]
pub struct OtCliCommand {
    /// issue one command
    #[argh(positional)]
    cmd: Option<String>,
}

const OT_CLI_CMD_SIZE_MAX: usize = 512;
const UTF8_CONVERTION_ERR_CNT_MAX: usize = 10;

impl OtCliCommand {
    fn cmd_stream() -> impl Stream<Item = String> {
        let (mut cmd_sender, cmd_receiver) = channel(OT_CLI_CMD_SIZE_MAX);

        let _ = std::thread::spawn(move || -> Result<(), Error> {
            let mut exec = fasync::LocalExecutorBuilder::new().build();

            let fut = async {
                let mut rl = DefaultEditor::new()?;
                loop {
                    let readline = rl.readline("> ");
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
                }
            };
            exec.run_singlethreaded(fut)
        });
        cmd_receiver
    }

    pub async fn exec(&self, context: &mut LowpanCtlContext) -> Result<(), Error> {
        let device_factory = context
            .get_default_device_factory()
            .await
            .context("Unable to get device factory instance")?;

        // Send out one command only if that is what user want
        if let Some(cmd) = self.cmd.clone() {
            let result = device_factory
                .send_mfg_command(&cmd)
                .await
                .context("Unable to send command to ot-ctl server")?;
            print!("{}", result);
            return Ok(());
        }

        // Otherwise setup the interactive shell
        let (client_socket_fidl, server_socket_fidl) = fidl::Socket::create_stream();
        device_factory.setup_ot_cli(server_socket_fidl).await.context("Unable to setup ot-ctl")?;
        let mut client_socket = fuchsia_async::Socket::from_socket(client_socket_fidl);
        let mut cmd_receiver_stream = OtCliCommand::cmd_stream();

        let mut outbound_buffer_received_bytes: Vec<u8> = vec![];
        let mut inbound_buffer_received_bytes: Vec<u8> = vec![];
        let mut utf8_encode_err_cnt: usize = 0;
        let socket_future =
            futures::future::poll_fn(move |cx| -> Poll<Result<(), anyhow::Error>> {
                const BUFFER_SIZE: usize = 1000;
                let mut buffer = [0u8; BUFFER_SIZE];
                // Poll the socket to get all the data from lowpan-ot-driver
                loop {
                    match Pin::new(&mut client_socket).poll_read(cx, &mut buffer) {
                        Poll::Ready(Ok(len)) => {
                            inbound_buffer_received_bytes.extend_from_slice(&buffer[..len]);
                        }
                        Poll::Ready(Err(err)) => {
                            println!("poll_read err {:?}", err);
                            return Poll::Ready(Err(err).context("poll_read"));
                        }
                        Poll::Pending => {
                            break;
                        }
                    };
                }

                // Try to print the response after all the available info is received
                if !inbound_buffer_received_bytes.is_empty() {
                    if let Ok(utf8_str) = std::str::from_utf8(&inbound_buffer_received_bytes) {
                        print!("{}", utf8_str);
                        inbound_buffer_received_bytes.clear();
                        utf8_encode_err_cnt = 0;
                    } else {
                        utf8_encode_err_cnt += 1;
                    }
                    if utf8_encode_err_cnt >= UTF8_CONVERTION_ERR_CNT_MAX {
                        println!("invalid response received, cleaning up the inbound buffer");
                        inbound_buffer_received_bytes.clear();
                        utf8_encode_err_cnt = 0;
                    }
                }

                // If the previous command is sent out, try to get a new command string, if there is any
                if outbound_buffer_received_bytes.is_empty() {
                    loop {
                        match cmd_receiver_stream.poll_next_unpin(cx) {
                            Poll::Ready(Some(mut cmd)) => {
                                cmd.push('\n');
                                outbound_buffer_received_bytes.append(&mut cmd.into_bytes());
                            }
                            Poll::Ready(None) => {
                                println!("cmd receiver err: stream ended");
                            }
                            Poll::Pending => {
                                break;
                            }
                        };
                    }
                }

                // If there is a command ready for send out, write it to socket.
                while !outbound_buffer_received_bytes.is_empty() {
                    match Pin::new(&mut client_socket)
                        .poll_write(cx, &outbound_buffer_received_bytes)
                    {
                        Poll::Ready(Ok(bytes_written)) => {
                            let _ = outbound_buffer_received_bytes.drain(..bytes_written);
                        }
                        Poll::Ready(Err(err)) => {
                            println!("poll_write err {:?}", err);
                            return Poll::Ready(Err(err).context("poll_write"));
                        }
                        Poll::Pending => {
                            break;
                        }
                    }
                }
                Poll::Pending
            });

        socket_future.await?;
        Ok(())
    }
}
