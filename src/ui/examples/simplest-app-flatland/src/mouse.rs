// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::internal_message::*;
use fidl_fuchsia_ui_pointer::{self as fptr};
use fuchsia_async as fasync;
use futures::channel::mpsc::UnboundedSender;
use log::*;

pub fn spawn_mouse_source_watcher(
    mouse_source: fptr::MouseSourceProxy,
    sender: UnboundedSender<InternalMessage>,
) {
    fasync::Task::spawn(async move {
        let mut was_pressed = false;

        loop {
            let events = mouse_source.watch();

            match events.await {
                Ok(events) => {
                    for e in events.iter() {
                        fuchsia_trace::duration!("input", "mouse_source_watcher");

                        let trace_id = e.trace_flow_id.expect("Trace flow id should exist");
                        // need to step dispatch_event_to_client to allow the flow end on scenic.
                        fuchsia_trace::flow_step!(
                            "input",
                            "dispatch_event_to_client",
                            trace_id.into()
                        );

                        // use different trace_id in this application to avoid confuse the trace.
                        let app_trace_id = fuchsia_trace::Id::new();

                        fuchsia_trace::flow_begin!(
                            "input",
                            "mouse_in_simplest_app",
                            app_trace_id.into()
                        );

                        if let Some(pointer_sample) = &e.pointer_sample {
                            let is_pressed = pointer_sample
                                .pressed_buttons
                                .as_ref()
                                .map_or(false, |buttons| !buttons.is_empty());
                            let is_up_event = !is_pressed && was_pressed;
                            was_pressed = is_pressed;

                            // We don't want to double change color when the user presses down and then releases.
                            let change_color = !is_up_event;

                            sender
                                .unbounded_send(InternalMessage::MouseEvent {
                                    trace_id: app_trace_id.into(),
                                    change_color,
                                })
                                .expect("Failed to send internal message");
                        } else {
                            // If there is no pointer_sample, just end the flow that we began above.
                            fuchsia_trace::flow_end!(
                                "input",
                                "mouse_in_simplest_app",
                                app_trace_id.into()
                            );
                        }
                    }
                }
                _ => {
                    error!("MouseSource connection closed");
                    return;
                }
            }
        }
    })
    .detach();
}
