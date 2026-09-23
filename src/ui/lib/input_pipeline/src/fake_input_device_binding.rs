// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![cfg(test)]

use crate::{input_device, keyboard_binding};
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;

/// A fake [`InputDeviceBinding`] for testing.
pub struct FakeInputDeviceBinding {
    /// The channel to stream InputEvents to.
    event_sender: UnboundedSender<Vec<input_device::InputEvent>>,
}

impl FakeInputDeviceBinding {
    pub fn new(input_event_sender: UnboundedSender<Vec<input_device::InputEvent>>) -> Self {
        FakeInputDeviceBinding { event_sender: input_event_sender }
    }
}

#[async_trait]
impl input_device::InputDeviceBinding for FakeInputDeviceBinding {
    fn get_device_descriptor(&self) -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::Keyboard(keyboard_binding::KeyboardDeviceDescriptor {
            keys: vec![],
            device_information: fidl_fuchsia_input_report::DeviceInformation {
                vendor_id: Some(42),
                product_id: Some(43),
                version: Some(44),
                polling_rate: Some(1000),
                ..Default::default()
            },
            // Random fake identifier.
            device_id: 442,
        })
    }

    fn input_event_sender(&self) -> UnboundedSender<Vec<input_device::InputEvent>> {
        self.event_sender.clone()
    }
}
