// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceState;
use crate::utils;
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use std::cell::RefCell;
use std::sync::Arc;

type DeviceTaskCallback = Box<dyn FnOnce(Arc<RefCell<DeviceState>>) + Send>;

// DeviceTask represents work that can be queued to the device thread for execution.
pub struct DeviceTask {
    callback: DeviceTaskCallback,
}

impl DeviceTask {
    pub fn new(callback: DeviceTaskCallback) -> DeviceTask {
        DeviceTask { callback }
    }

    pub fn handle(self, device: Arc<RefCell<DeviceState>>) {
        (self.callback)(device)
    }
}

pub struct DeviceTaskReceiver {
    receiver: UnboundedReceiver<DeviceTask>,
}

impl DeviceTaskReceiver {
    pub async fn next(&mut self) -> Option<DeviceTask> {
        self.receiver.next().await
    }
}

pub struct DeviceTaskSender {
    sender: UnboundedSender<DeviceTask>,
}

impl DeviceTaskSender {
    pub fn new() -> (Self, DeviceTaskReceiver) {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        (DeviceTaskSender { sender }, DeviceTaskReceiver { receiver })
    }

    pub fn send(&self, request: DeviceTaskCallback) {
        let _ = self.sender.unbounded_send(DeviceTask::new(request));
    }
}

#[derive(Clone)]
pub struct CompletionEvent {
    event: Arc<zx::Event>,
}

impl CompletionEvent {
    pub fn new() -> CompletionEvent {
        CompletionEvent { event: Arc::new(zx::Event::create()) }
    }

    pub fn signal(&self) {
        let result = self.event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED);
        utils::debug_assert_ok!(result);
    }

    pub fn wait(&self) {
        let mut wait_items = [self.event.wait_item(zx::Signals::EVENT_SIGNALED)];
        let result = zx::object_wait_many(&mut wait_items, zx::MonotonicInstant::INFINITE);
        utils::debug_assert_ok!(result);
        debug_assert!(result.unwrap() == false);
    }

    pub async fn async_wait(&self) {
        let result = fasync::OnSignals::new(self.event.as_ref(), zx::Signals::EVENT_SIGNALED).await;
        utils::debug_assert_ok!(result);
    }

    pub fn reset(&self) {
        let result = self.event.signal(zx::Signals::EVENT_SIGNALED, zx::Signals::empty());
        utils::debug_assert_ok!(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_completion_event() {
        let event = CompletionEvent::new();
        event.signal();
        event.wait();
        event.reset();
    }

    #[fuchsia::test]
    async fn test_async_completion_event() {
        let event = CompletionEvent::new();
        event.signal();
        event.async_wait().await;
        event.reset();
    }
}
