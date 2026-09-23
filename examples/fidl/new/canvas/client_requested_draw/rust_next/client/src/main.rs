// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use config::Config;
use fidl_next_examples_canvas_clientrequesteddraw::instance::OnDrawn;
use fidl_next_examples_canvas_clientrequesteddraw::{Instance, InstanceClientHandler, Point};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::StreamExt;

#[derive(Debug)]
enum InstanceEvent {
    OnDrawn { top_left: Point, bottom_right: Point },
    Unknown { ordinal: u64 },
}

struct CanvasClient {
    sender: UnboundedSender<InstanceEvent>,
}

impl InstanceClientHandler for CanvasClient {
    async fn on_drawn(&mut self, event: fidl_next::Request<OnDrawn>) {
        let payload = event.payload();
        let event = InstanceEvent::OnDrawn {
            top_left: payload.top_left,
            bottom_right: payload.bottom_right,
        };
        let _ = self.sender.unbounded_send(event);
    }

    async fn on_unknown_interaction(&mut self, ordinal: u64) {
        self.sender
            .unbounded_send(InstanceEvent::Unknown { ordinal })
            .context("Error sending unknown event to channel")
            .unwrap();
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Load the structured config values passed to this component at startup.
    let config = Config::take_from_startup_handle();

    // Create an MPSC channel to receive events from the InstanceClientHandler.
    let (sender, mut receiver) = futures::channel::mpsc::unbounded::<InstanceEvent>();
    let client_impl = CanvasClient { sender };

    // Use the Component Framework runtime to connect to the newly spun up server component.
    let instance = connect_to_protocol::<Instance>()?
        .spawn_handler(client_impl);
    println!("Outgoing connection enabled");

    let mut batched_lines = Vec::<[Point; 2]>::new();
    for action in config.script.into_iter() {
        // If the next action in the script is to "PUSH", send a batch of lines to the server.
        if action == "PUSH" {
            instance.add_lines(&batched_lines).await.context("Could not send lines")?;
            println!("AddLines request sent");
            batched_lines.clear();
            continue;
        }

        // If the next action in the script is to "WAIT", block until an OnDrawn event is received
        // from the server.
        if action == "WAIT" {
            loop {
                match receiver.next().await.context("Proxy sent no events")? {
                    InstanceEvent::OnDrawn { top_left, bottom_right } => {
                        println!(
                            "OnDrawn event received: top_left: {:?}, bottom_right: {:?}",
                            top_left, bottom_right
                        );
                        break;
                    }
                    InstanceEvent::Unknown { ordinal } => {
                        println!("Received an unknown event with ordinal {ordinal}");
                    }
                }
            }

            // Now, inform the server that we are ready to receive more updates whenever they are
            // ready for us.
            println!("Ready request sent");
            instance.ready().await.context("Could not send ready call")?;
            println!("Ready success");
            continue;
        }

        // Add a line to the next batch. Parse the string input, making two points out of it.
        let mut points = action
            .split(":")
            .map(|point| {
                let integers = point
                    .split(",")
                    .map(|integer| integer.parse::<i64>().unwrap())
                    .collect::<Vec<i64>>();
                Point { x: integers[0], y: integers[1] }
            })
            .collect::<Vec<Point>>();

        // Assemble a line from the two points.
        let from = points.pop().ok_or_else(|| format_err!("line requires 2 points, but has 0"))?;
        let to = points.pop().ok_or_else(|| format_err!("line requires 2 points, but has 1"))?;
        let line: [Point; 2] = [from, to];

        // Batch a line for drawing to the canvas using the two points provided.
        println!("AddLines batching line: {:?}", line);
        batched_lines.push(line);
    }

    Ok(())
}
