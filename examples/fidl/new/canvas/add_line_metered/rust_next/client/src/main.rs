// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use config::Config;
use fidl_next_examples_canvas_addlinemetered::{Instance, InstanceClientHandler, Point};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::StreamExt;

#[derive(Debug)]
enum InstanceEvent {
    OnDrawn { top_left: Point, bottom_right: Point },
    Unknown(u64),
}

struct CanvasClient {
    sender: UnboundedSender<InstanceEvent>,
}

impl InstanceClientHandler for CanvasClient {
    async fn on_drawn(
        &mut self,
        event: fidl_next::Request<fidl_next_examples_canvas_addlinemetered::instance::OnDrawn>,
    ) {
        let payload = event.payload();
        let event = InstanceEvent::OnDrawn {
            top_left: payload.top_left,
            bottom_right: payload.bottom_right,
        };
        self.sender.unbounded_send(event).context("Error sending event to channel").unwrap();
    }

    async fn on_unknown_interaction(&mut self, ordinal: u64) {
        self.sender
            .unbounded_send(InstanceEvent::Unknown(ordinal))
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
    let instance = connect_to_protocol::<Instance>()
        .expect("Error connecting to Instance protocol")
        .spawn_handler(client_impl);
    println!("Outgoing connection enabled");

    for action in config.script.into_iter() {
        // If the next action in the script is to "WAIT", block until an OnDrawn event is received
        // from the server.
        if action == "WAIT" {
            loop {
                match receiver.next().await.ok_or_else(|| format_err!("Proxy sent no events"))? {
                    InstanceEvent::OnDrawn { top_left, bottom_right } => {
                        println!(
                            "OnDrawn event received: top_left: {:?}, bottom_right: {:?}",
                            top_left, bottom_right
                        );
                        break;
                    }
                    InstanceEvent::Unknown(ordinal) => {
                        println!("Received an unknown event with ordinal {ordinal}");
                    }
                }
            }
            continue;
        }

        // If the action is not a "WAIT", we need to draw a line instead. Parse the string input,
        // making two points out of it.
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
        let line = [from, to];

        // Draw a line to the canvas by calling the server, using the two points we just parsed
        // above as arguments.
        println!("AddLine request sent: {:?}", line);

        // By awaiting on the reply, we prevent the client from sending another request before the
        // server is ready to handle, thereby syncing the flow rate between the two parties over
        // this method.
        instance.add_line(&line).await.context("Error sending request")?;
        println!("AddLine response received");
    }

    Ok(())
}
