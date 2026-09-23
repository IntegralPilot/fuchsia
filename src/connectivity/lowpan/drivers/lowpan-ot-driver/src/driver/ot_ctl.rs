// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use super::*;
use fuchsia_async as fasync;
use futures::task::Poll;
use std::cell::RefCell;
use std::pin::Pin;

#[derive(Debug)]
pub struct OtCtl {
    /// Use this to send command to OpenThread CLI
    pub cli_input_sender: Option<futures::channel::mpsc::UnboundedSender<Option<String>>>,

    /// Task to serve the client socket setup via FIDL
    cli_task: RefCell<Option<fasync::Task<Result<(), anyhow::Error>>>>,
}

impl OtCtl {
    pub fn new() -> Self {
        OtCtl { cli_input_sender: None, cli_task: RefCell::new(None) }
    }

    pub fn replace_client_socket<OT: ot::Cli>(
        &self,
        socket: fuchsia_async::Socket,
        ot_instance: &OT,
    ) {
        let task = self.start_task(socket, ot_instance);
        self.cli_task.replace(Some(task));
    }

    fn start_task<OT: ot::Cli>(
        &self,
        mut socket: fuchsia_async::Socket,
        ot_instance: &OT,
    ) -> fasync::Task<Result<(), anyhow::Error>> {
        let openthread_user_input_sender = self.cli_input_sender.clone();
        let (cli_output_sender, cli_output_receiver) = futures::channel::mpsc::unbounded();
        ot_instance.cli_init(move |c_str| {
            if !cli_output_sender.is_closed() {
                if let Err(e) =
                    cli_output_sender.unbounded_send(c_str.to_string_lossy().into_owned())
                {
                    warn!("OpenThread CLI API callback encountered error: {:?}", e);
                }
            }
        });

        let mut openthread_conole_output_receiver = cli_output_receiver;
        // Internal state for the pull_fn. These buffers may carry data between polls.
        let mut outbound_buffer: Vec<u8> = Vec::new();
        let mut inbound_buffer: Vec<u8> = Vec::new();

        let socket_future = futures::future::poll_fn(
            move |cx| -> Poll<Result<(), anyhow::Error>> {
                const BUFFER_SIZE: usize = 1024;
                let mut buffer = [0u8; BUFFER_SIZE];
                // Getting one line from the socket, read multiple times until the end of line is detected
                // Send out the data to OpenThread CLI API immdeiately if one line is detected
                loop {
                    if socket.is_closed() {
                        // Stop the task once the client side drops the socket
                        return Poll::Ready(Ok(()));
                    }
                    match Pin::new(&mut socket).poll_read(cx, &mut buffer) {
                        Poll::Ready(Ok(len)) => {
                            let mut bytes = buffer[..len].to_vec();
                            inbound_buffer.append(&mut bytes);
                            if let Some(idx) =
                                inbound_buffer.iter().position(|&element| element == 0xa)
                            {
                                // '\n', send the command
                                let cmd_line = inbound_buffer[..idx + 1].to_vec(); // '\n' included
                                let _ = inbound_buffer.drain(..(idx + 1));
                                if let Ok(utf8_str) = String::from_utf8(cmd_line) {
                                    if let Some(sender) = &openthread_user_input_sender {
                                        if let Err(err) = sender.unbounded_send(Some(utf8_str)) {
                                            error!(
                                                "start_task: sender.unbounded_send failed: {:?}",
                                                err
                                            );
                                            return Poll::Ready(Err(err).context(
                                                "openthread_user_input_sender unbounded_send() failed",
                                            ));
                                        }
                                    }
                                } else {
                                    warn!("failed to encode the CLI command string sent by client socket");
                                }
                            }
                        }
                        Poll::Ready(Err(err)) => {
                            error!("poll_read err {:?}", err);
                            return Poll::Ready(Err(err).context("poll_read"));
                        }
                        Poll::Pending => {
                            break;
                        }
                    }
                }

                // Poll the inbound responses from OpenThread CLI API
                if outbound_buffer.is_empty() {
                    loop {
                        match openthread_conole_output_receiver.poll_next_unpin(cx) {
                            Poll::Ready(Some(cli_output_str)) => {
                                info!("CLI output: {}", MarkPii(&cli_output_str));
                                outbound_buffer.append(&mut cli_output_str.as_bytes().to_vec());
                            }
                            Poll::Ready(None) => {
                                return Poll::Ready(Ok(()));
                            }
                            Poll::Pending => {
                                break;
                            }
                        }
                    }
                } else {
                    // Cannot poll, need to wake up shortly to retry
                    cx.waker().wake_by_ref();
                }

                // Forward all inbound responses to the client socket
                while !outbound_buffer.is_empty() {
                    match Pin::new(&mut socket).poll_write(cx, &outbound_buffer) {
                        Poll::Ready(Ok(len)) => {
                            let _ = outbound_buffer.drain(..len);
                        }
                        Poll::Ready(Err(err)) => {
                            error!("writing to socket encountered error {:?}", err);
                            return Poll::Ready(Err(err).context("poll_write"));
                        }
                        Poll::Pending => {
                            break;
                        }
                    }
                }
                Poll::Pending
            },
        );
        fasync::Task::spawn(socket_future.map(|x| {
            if let Err(e) = x {
                warn!("ot_ctl terminated, reason: {:?}", e);
            }
            Ok(())
        }))
    }
}
