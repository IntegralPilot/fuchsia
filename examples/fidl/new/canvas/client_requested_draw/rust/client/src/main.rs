// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use config::Config;
use fidl_examples_canvas_clientrequesteddraw::{InstanceEvent, InstanceMarker, Point};
use fuchsia_component::client::connect_to_protocol;
use futures::TryStreamExt;
use std::{thread, time};

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Load the structured config values passed to this component at startup.
    let config = Config::take_from_startup_handle();

    // Use the Component Framework runtime to connect to the newly spun up server component. We wrap
    // our retained client end in a proxy object that lets us asynchronously send Instance requests
    // across the channel.
    let instance = connect_to_protocol::<InstanceMarker>()?;
    println!("Outgoing connection enabled");

    // [START diff_1]
    let mut batched_lines = Vec::<[Point; 2]>::new();
    for action in config.script.into_iter() {
        // If the next action in the script is to "PUSH", send a batch of lines to the server.
        if action == "PUSH" {
            instance.add_lines(&batched_lines).context("Could not send lines")?;
            println!("AddLines request sent");
            batched_lines.clear();
            continue;
        }
        // [END diff_1]

        // If the next action in the script is to "WAIT", block until an OnDrawn event is received
        // from the server.
        if action == "WAIT" {
            let mut event_stream = instance.take_event_stream();
            loop {
                match event_stream
                    .try_next()
                    .await
                    .context("Error getting event response from proxy")?
                    .ok_or_else(|| format_err!("Proxy sent no events"))?
                {
                    InstanceEvent::OnDrawn { top_left, bottom_right } => {
                        println!(
                            "OnDrawn event received: top_left: {:?}, bottom_right: {:?}",
                            top_left, bottom_right
                        );
                        break;
                    }
                    InstanceEvent::_UnknownEvent { ordinal, .. } => {
                        println!("Received an unknown event with ordinal {ordinal}");
                    }
                }
            }

            // [START diff_2]
            // Now, inform the server that we are ready to receive more updates whenever they are
            // ready for us.
            println!("Ready request sent");
            instance.ready().await.context("Could not send ready call")?;
            println!("Ready success");
            continue;
            // [END diff_2]
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

        // [START diff_3]
        // Assemble a line from the two points.
        let from = points.pop().ok_or_else(|| format_err!("line requires 2 points, but has 0"))?;
        let to = points.pop().ok_or_else(|| format_err!("line requires 2 points, but has 1"))?;
        let line: [Point; 2] = [from, to];

        // Batch a line for drawing to the canvas using the two points provided.
        println!("AddLines batching line: {:?}", line);
        batched_lines.push(line);
        // [END diff_3]
    }

    // TODO(https://fxbug.dev/42156498): We need to sleep here to make sure all logs get drained. Once the
    // referenced bug has been resolved, we can remove the sleep.
    thread::sleep(time::Duration::from_secs(2));
    Ok(())
}
