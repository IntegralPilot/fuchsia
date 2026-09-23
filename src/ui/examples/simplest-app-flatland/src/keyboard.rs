// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::internal_message::InternalMessage;
use fidl_fuchsia_ui_input3::{
    KeyEventStatus, KeyEventType, KeyboardListenerRequest, KeyboardListenerRequestStream,
};
use fuchsia_async as fasync;
use fuchsia_trace as trace;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use log::warn;

pub fn spawn_keyboard_listener(
    mut stream: KeyboardListenerRequestStream,
    sender: UnboundedSender<InternalMessage>,
) {
    fasync::Task::local(async move {
        while let Some(request) = stream.next().await {
            match request {
                Ok(KeyboardListenerRequest::OnKeyEvent { event, responder, .. }) => {
                    trace::duration!("input", "simplest_app::OnKeyEvent");
                    let trace_id = trace::Id::new();
                    trace::flow_begin!("input", "keyboard_in_simplest_app", trace_id);

                    // We don't want to double change color when the user presses down and then releases.
                    let change_color = event.type_ == Some(KeyEventType::Pressed);

                    if sender
                        .unbounded_send(InternalMessage::KeyboardEvent { trace_id, change_color })
                        .is_err()
                    {
                        warn!("Failed to send KeyboardEvent to main loop");
                    }
                    if let Err(e) = responder.send(KeyEventStatus::Handled) {
                        warn!("Failed to respond to OnKeyEvent: {:?}", e);
                    }
                }
                Err(e) => {
                    warn!("Keyboard listener error: {:?}", e);
                }
            }
        }
    })
    .detach();
}
