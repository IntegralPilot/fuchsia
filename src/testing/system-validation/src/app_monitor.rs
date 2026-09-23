// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fuchsia_async as fasync;
use std::sync::{Arc, Mutex};

// Helper for getting information about the child component using event_stream.
pub struct AppMonitor {
    pub moniker: String,
    stopped: Arc<Mutex<bool>>,
}

impl AppMonitor {
    pub fn new(moniker: String) -> Self {
        Self { moniker: moniker, stopped: Arc::new(Mutex::new(false)) }
    }

    // Non-blocking. Starts a separate process that waits for a Stopped event from the moniker.
    // Note: this is a best effort check, the stopped event is only being observed while the
    // system validation test is running.
    pub fn add_monitor_for_stop_event(&self) {
        *self.stopped.lock().unwrap() = false;
        let stopped_clone = self.stopped.clone();
        let moniker_clone = self.moniker.clone();
        fasync::Task::spawn(async move {
            let mut event_stream = EventStream::open().await.unwrap();
            EventMatcher::ok()
                .moniker(&moniker_clone)
                .wait::<Stopped>(&mut event_stream)
                .await
                .unwrap_or_else(|e| {
                    panic!("failed to observe {} stop event: {:?}", &moniker_clone, e)
                });
            *stopped_clone.lock().unwrap() = true;
        })
        .detach();
    }

    pub fn has_seen_stop_event(&self) -> bool {
        *self.stopped.lock().unwrap()
    }

    // Blocking. Waits for event matcher to report that app is running.
    pub async fn wait_for_start_event(&self) {
        let mut event_stream = EventStream::open().await.unwrap();
        let moniker = self.moniker.clone();

        EventMatcher::ok()
            .moniker(&moniker)
            .wait::<Started>(&mut event_stream)
            .await
            .unwrap_or_else(|e| panic!("failed to observe {} start event: {:?}", &moniker, e));
    }
}
