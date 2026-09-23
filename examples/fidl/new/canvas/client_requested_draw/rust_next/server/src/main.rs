// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_next::{Request, Responder, Server};
use fidl_next_examples_canvas_clientrequesteddraw::{
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
#[derive(Debug)]
struct CanvasState {
    // Tracks whether there has been a change since the last send, to prevent redundant updates.
    changed: bool,
    // Tracks whether or not the client has declared itself ready to receive more updated.
    ready: bool,
    bounding_box: BoundingBox,
}

struct CanvasServer {
    state: Arc<Mutex<CanvasState>>,
    // We don't strictly need sender here if we don't send events from the handler,
    // but it's good practice or might be needed for some patterns.
    // In this example, events are sent from a separate task.
}

impl InstanceServerHandler for CanvasServer {
    async fn add_lines(&mut self, request: Request<instance::AddLines>) {
        let lines = &request.payload().lines;
        println!("AddLines request received");

        let mut state = self.state.lock();

        // Update the bounding box to account for the new lines we've just "added" to the canvas.
        let bounds = &mut state.bounding_box;
        for line in lines {
            println!("AddLines printing line: {:?}", line);
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
        }

        // Mark the state as "dirty", so that an update is sent back to the client on the next tick.
        state.changed = true;
    }

    async fn ready(&mut self, responder: Responder<instance::Ready>) {
        println!("Ready request received");
        // The client must only call `Ready() -> ();` after receiving an `-> OnDrawn();`
        // event; if two "consecutive" `Ready() -> ();` calls are received, this
        // interaction has entered an invalid state, and should be aborted immediately.
        {
            let mut state = self.state.lock();
            if state.ready {
                // Invalid back-to-back Ready requests.
                println!("Invalid back-to-back `Ready` requests received");
                return;
            }

            state.ready = true;
        }
        responder.respond(()).await.unwrap();
    }
}

/// A separate watcher task periodically "draws" the canvas, and notifies the client of the new
/// state.
async fn run_updater(state: Arc<Mutex<CanvasState>>, sender: Server<Instance>) {
    loop {
        // Our server sends one update per second, but only if the client has declared that it
        // is ready to receive one.
        Timer::new(MonotonicInstant::after(zx::Duration::from_seconds(1))).await;

        let bounds = {
            let mut state = state.lock();
            if !state.changed || !state.ready {
                continue;
            }

            // Reset the change and ready trackers.
            state.ready = false;
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
            ready: true,
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
