// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
use crate::{
    input_device,
    input_handler::{self, Handler},
};
#[cfg(test)]
use async_trait::async_trait;
#[cfg(test)]
use futures::channel::mpsc::Sender;
#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;

/// A fake [`InputHandler`] used for testing.
/// A [`ObserveFakeEventsInputHandler`] does not consume InputEvents.
#[cfg(test)]
pub struct ObserveFakeEventsInputHandler {
    /// Events received by [`handle_input_event()`] are sent to this channel.
    event_sender: RefCell<Sender<input_device::InputEvent>>,
}

#[cfg(test)]
impl ObserveFakeEventsInputHandler {
    pub fn new(event_sender: Sender<input_device::InputEvent>) -> Rc<Self> {
        Rc::new(ObserveFakeEventsInputHandler { event_sender: RefCell::new(event_sender) })
    }
}

#[cfg(test)]
impl Handler for ObserveFakeEventsInputHandler {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        // No inspect data on ObserveFakeEventsInputHandler. Do nothing.
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, _msg: &str) {
        // No inspect data on ObserveFakeEventsInputHandler. Do nothing.
    }

    fn get_name(&self) -> &'static str {
        "ObserveFakeEventsInputHandler"
    }

    fn interest(&self) -> Vec<input_device::InputEventType> {
        vec![
            input_device::InputEventType::Keyboard,
            input_device::InputEventType::LightSensor,
            input_device::InputEventType::ConsumerControls,
            input_device::InputEventType::Mouse,
            input_device::InputEventType::TouchScreen,
            input_device::InputEventType::Touchpad,
            input_device::InputEventType::Fake,
        ]
    }
}

#[async_trait(?Send)]
#[cfg(test)]
impl input_handler::InputHandler for ObserveFakeEventsInputHandler {
    async fn handle_input_event(
        self: Rc<Self>,
        input_event: input_device::InputEvent,
    ) -> Vec<input_device::InputEvent> {
        match self.event_sender.borrow_mut().try_send(input_event.clone()) {
            Err(_) => assert!(false),
            _ => {}
        };

        vec![input_event]
    }
}
