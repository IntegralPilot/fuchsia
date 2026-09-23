// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use config::Config;
use fidl_next_examples_canvas_baseline::instance::OnDrawn;
use fidl_next_examples_canvas_baseline::{BoundingBox, Instance, InstanceClientHandler, Point};
use fuchsia_component::client::fidl_next::connect_to_protocol;
use futures::channel::mpsc::UnboundedSender;
use futures::stream::StreamExt;

struct CanvasClient {
    sender: UnboundedSender<BoundingBox>,
}

impl InstanceClientHandler for CanvasClient {
    async fn on_drawn(&mut self, event: fidl_next::Request<OnDrawn>) {
        let bounding_box = event.payload();
        self.sender
            .unbounded_send(bounding_box)
            .context("Error sending bounding box to channel")
            .unwrap();
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    // Load the structured config values passed to this component at startup.
    let config = Config::take_from_startup_handle();

    // Create an MPSC channel to receive events from the InstanceClientHandler, but not process them
    // until we get a WAIT action in the script. Create an async-aware mpsc so that while we're
    // waiting to read from it, we can also pump our executor for other async tasks.
    let (sender, mut receiver) = futures::channel::mpsc::unbounded::<BoundingBox>();
    let client_impl = CanvasClient { sender };

    // Create a client for the Instance protocol, with an event handler to handle the OnDrawn events
    let client = connect_to_protocol::<Instance>()
        .expect("Error connecting to Instance protocol")
        .spawn_handler(client_impl);

    for action in config.script.into_iter() {
        // If the next action in the script is to "WAIT", block until an OnDrawn event is received
        // from the server.
        if action == "WAIT" {
            let BoundingBox { top_left, bottom_right } =
                receiver.next().await.context("Error getting bounding box from channel")?;
            println!(
                "OnDrawn event received: top_left: {:?}, bottom_right: {:?}",
                top_left, bottom_right
            );
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
        client.add_line(&line).await.context("Error sending request")?;
    }

    Ok(())
}
