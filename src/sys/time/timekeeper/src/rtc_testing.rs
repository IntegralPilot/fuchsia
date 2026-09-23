// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Testing-only code for persistent Timekeeper behavior changes around
//! real-time clock (RTC) handling.

use anyhow::Result;
use fidl_fuchsia_time_test as fftt;
use fuchsia_inspect::health;
use fuchsia_inspect::health::Reporter;
use futures::StreamExt;
use log::{debug, error};
use std::cell::RefCell;
use std::rc::Rc;
use time_persistence::{self, State};

#[derive(Debug)]
pub struct Server {
    health_node: Rc<RefCell<health::Node>>,
    persistent_state: Rc<RefCell<State>>,
}

impl Server {
    /// Create a new [Server].
    ///
    /// Args:
    /// - `health_node`: a health node for the persistence subsystem. Errors
    ///   here aren't usually fatal, but the value may be useful for diagnostics.
    /// - `persistent_state`: the state fragment that this server may modify.
    pub fn new(
        health_node: Rc<RefCell<health::Node>>,
        persistent_state: Rc<RefCell<State>>,
    ) -> Self {
        Server { health_node, persistent_state }
    }

    fn set_may_update_rtc<F, E>(&self, value: bool, update_fn: F) -> Result<()>
    where
        // All this to allow using `responder`s of two different types...
        F: FnOnce() -> Result<(), E>,
        E: 'static + std::error::Error + Send + Sync,
    {
        let health = self.health_node.clone();
        self.persistent_state.borrow_mut().set_may_update_rtc(value);
        {
            update_fn()?;
            State::write(&self.persistent_state.borrow())
        }
        .map(|_| health.borrow_mut().set_ok())
        .map_err(|e| {
            health
                .borrow_mut()
                .set_unhealthy(&format!("at runtime: {:?}; use #DEBUG logs for details", e));
            e
        })
    }

    /// Serves `fuchsia.time.test/Rtc`.
    ///
    /// Args:
    /// - `stream`: the request stream from the test fixture.
    pub async fn serve(&self, mut stream: fftt::RtcRequestStream) -> Result<()> {
        debug!("rtc_testing::serve: entering serving loop");
        while let Some(request) = stream.next().await {
            match request {
                Ok(fftt::RtcRequest::PersistentEnable { responder, .. }) => {
                    debug!("received: fuchsia.time.test/Rtc.PersistentEnable");
                    self.set_may_update_rtc(true, || responder.send(Ok(())))?;
                }
                Ok(fftt::RtcRequest::PersistentDisable { responder, .. }) => {
                    debug!("received: fuchsia.time.test/Rtc.PersistentDisable");
                    self.set_may_update_rtc(false, || responder.send(Ok(())))?;
                }
                Err(e) => {
                    error!("FIDL error: {:?}", e);
                }
            };
        }
        debug!("rtc_testing::serve: exited  serving loop");
        Ok(())
    }
}
