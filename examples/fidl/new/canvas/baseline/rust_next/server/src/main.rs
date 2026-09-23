// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Server};
use fidl_next_examples_canvas_baseline::{
    BoundingBox, Instance, InstanceServerHandler, Point, instance,
};
use fuchsia_async::{MonotonicInstant, Scope, Timer};

use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use std::sync::Arc;
use zx;

// A struct that stores the two things we care about for this example: the bounding box the lines
// that have been added thus far, and bit to track whether or not there have been changes since the
// last `OnDrawn` event.
struct CanvasState {
    // Tracks whether there has been a change since the last send, to prevent redundant updates.
    changed: bool,
    bounding_box: BoundingBox,
}

struct CanvasServer {
    state: Arc<Mutex<CanvasState>>,
    sender: Server<Instance>,
}

impl InstanceServerHandler for CanvasServer {
    async fn add_line(&mut self, request: Request<instance::AddLine>) {
        // Get the line from the request payload
        let line = request.payload().line;

        println!("AddLine request received: {:?}", line);

        let mut state = self.state.lock();

        // Update the bounding box to account for the new lines we've just "added" to the canvas.
        let bounds = &mut state.bounding_box;
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
        state.changed = true;
    }
}

/// A separate watcher task periodically "draws" the canvas, and notifies the client of the new
/// state.
async fn run_updater(state: Arc<Mutex<CanvasState>>, sender: Server<Instance>) {
    loop {
        // Our server sends one update per second.
        Timer::new(MonotonicInstant::after(zx::Duration::from_seconds(1))).await;
        let (changed, bounds) = {
            let mut state_ref = state.lock();
            if !state_ref.changed {
                (false, state_ref.bounding_box.clone())
            } else {
                // Reset the change tracker.
                state_ref.changed = false;
                (true, state_ref.bounding_box.clone())
            }
        };

        if !changed {
            continue;
        }

        if sender.on_drawn_with(&bounds).await.is_err() {
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

        let server = CanvasServer { state: state.clone(), sender };

        // Spawn a task to run the updater.
        scope.spawn(run_updater(state, server.sender.clone()));

        server
    });
    fs.take_and_serve_directory_handle()?;
    println!("Listening for incoming connections");

    // Run the service fs.
    fs.collect::<()>().await;

    Ok(())
}
