// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder, Server};
use fidl_next_examples_canvas_addlinemetered::{
    BoundingBox, Instance, InstanceServerHandler, Point, instance,
};
use fuchsia_async::{MonotonicInstant, Scope, Timer};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use std::sync::{Arc, Mutex};
use zx;

// A struct that stores the two things we care about for this example: the bounding box the lines
// that have been added thus far, and bit to track whether or not there have been changes since the
// last `OnDrawn` event.
#[derive(Debug)]
struct CanvasState {
    // Tracks whether there has been a change since the last send, to prevent redundant updates.
    changed: bool,
    bounding_box: BoundingBox,
}

impl CanvasState {
    /// Handler for the `AddLine` method.
    fn add_line(&mut self, line: [Point; 2]) {
        // Update the bounding box to account for the new lines we've just "added" to the canvas.
        let bounds = &mut self.bounding_box;
        for point in line {
            if point.x < bounds.top_left.x {
                bounds.top_left.x = point.x;
            }
            if point.y > bounds.top_left.y {
                bounds.top_left.y = point.y;
            }
            if point.x > bounds.bottom_right.x {
                bounds.bottom_right.x = point.x;
            }
            if point.y < bounds.bottom_right.y {
                bounds.bottom_right.y = point.y;
            }
        }

        // Mark the state as "dirty", so that an update is sent back to the client on the next tick.
        self.changed = true
    }
}

struct CanvasServer {
    state: Arc<Mutex<CanvasState>>,
}

impl InstanceServerHandler for CanvasServer {
    async fn add_line(
        &mut self,
        request: Request<instance::AddLine>,
        responder: Responder<instance::AddLine>,
    ) {
        let line = &request.payload().line;
        println!("AddLine request received: {:?}", line);

        {
            let mut state = self.state.lock().unwrap();
            state.add_line(line.clone());
        }

        // Because this is now a two-way method, we must use the generated `responder`
        // to send an in this case empty reply back to the client. This is the mechanic
        // which syncs the flow rate between the client and server on this method,
        // thereby preventing the client from "flooding" the server with unacknowledged
        // work.
        responder.respond(()).await.unwrap();
        println!("AddLine response sent");
    }
}

/// A separate watcher task periodically "draws" the canvas, and notifies the client of the new
/// state.
async fn run_updater(state: Arc<Mutex<CanvasState>>, sender: Server<Instance>) {
    loop {
        // Our server sends one update per second.
        Timer::new(MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1))).await;

        let bounds = {
            let mut state = state.lock().unwrap();
            if !state.changed {
                continue;
            }
            state.changed = false;
            state.bounding_box.clone()
        };

        // Send the event.
        // on_drawn(top_left, bottom_right)
        if sender.on_drawn(&bounds.top_left, &bounds.bottom_right).await.is_err() {
            break;
        }

        println!(
            "OnDrawn event sent: top_left: {:?}, bottom_right: {:?}",
            bounds.top_left, bounds.bottom_right
        );
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    println!("Started");

    // Add a discoverable instance of our `Instance` protocol - this will allow the client to see
    // the server and connect to it.
    let scope = Arc::new(Scope::new());
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_next_protocol::<Instance, _>(move |sender| {
        let state = Arc::new(Mutex::new(CanvasState {
            changed: true,
            bounding_box: BoundingBox {
                top_left: Point { x: 0, y: 0 },
                bottom_right: Point { x: 0, y: 0 },
            },
        }));

        let server = CanvasServer { state: state.clone() };

        // Spawn a task to run the updater.
        scope.spawn(run_updater(state, sender));

        server
    });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    // Run the service fs.
    fs.collect::<()>().await;

    Ok(())
}
