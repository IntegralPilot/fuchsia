// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use event_listener::Event;
use std::ops::Deref;

/// Same as Event but notifies when dropped.
pub struct DropEvent(Event);

impl Drop for DropEvent {
    fn drop(&mut self) {
        // `Event` doesn't allocate its internal state until a listener is registered or a
        // notification is sent. Since we're in the Drop impl, no new listeners can be
        // registered. If there are no listeners, then we can skip sending the notification,
        // avoiding unnecessary heap allocation and mutex locking in `Event`.
        if self.0.total_listeners() > 0 {
            self.0.notify(usize::MAX);
        }
    }
}

impl Deref for DropEvent {
    type Target = Event;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DropEvent {
    pub fn new() -> Self {
        Self(Event::new())
    }
}
