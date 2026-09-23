// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_hardware_reset::{self as freset, ResetServerHandler};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use fuchsia_sync::Mutex;
use std::sync::Arc;

struct FakeResetState {
    asserted: bool,
    toggled: bool,
}

pub struct FakeReset {
    state: Arc<Mutex<FakeResetState>>,
}

impl FakeReset {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(FakeResetState { asserted: false, toggled: false })) }
    }

    pub fn asserted(&self) -> bool {
        self.state.lock().asserted
    }

    /// Returns and clears the current toggled state.
    pub fn take_toggled(&mut self) -> bool {
        let mut state = self.state.lock();
        let toggled = state.toggled;
        state.toggled = false;
        toggled
    }

    pub fn serve<O: ServiceObjTrait>(
        &self,
        service_fs: &mut ServiceFs<O>,
        scope: fasync::ScopeHandle,
        instance_name: &str,
    ) -> fdf::Offer {
        fdf_component::ServiceOffer::<freset::Service>::new_next()
            .add_default_named_next(
                service_fs,
                instance_name,
                FakeResetService { state: self.state.clone(), scope },
            )
            .build_zircon_offer_next()
    }
}

struct FakeResetService {
    state: Arc<Mutex<FakeResetState>>,
    scope: fasync::ScopeHandle,
}

impl freset::ServiceHandler for FakeResetService {
    fn reset(&self, server_end: fidl_next::ServerEnd<freset::Reset>) {
        server_end.spawn_on(FakeResetServer { state: self.state.clone() }, &self.scope);
    }
}

struct FakeResetServer {
    state: Arc<Mutex<FakeResetState>>,
}

impl ResetServerHandler for FakeResetServer {
    async fn assert(&mut self, responder: Responder<freset::reset::Assert>) {
        self.state.lock().asserted = true;
        let _ = responder.respond(()).await;
    }

    async fn deassert(&mut self, responder: Responder<freset::reset::Deassert>) {
        self.state.lock().asserted = false;
        let _ = responder.respond(()).await;
    }

    async fn toggle(&mut self, responder: Responder<freset::reset::Toggle>) {
        self.state.lock().toggled = true;
        let _ = responder.respond(()).await;
    }

    async fn toggle_with_timeout(
        &mut self,
        _request: Request<freset::reset::ToggleWithTimeout>,
        responder: Responder<freset::reset::ToggleWithTimeout>,
    ) {
        self.state.lock().toggled = true;
        let _ = responder.respond(()).await;
    }

    async fn status(&mut self, responder: Responder<freset::reset::Status>) {
        let asserted = self.state.lock().asserted;
        let _ = responder.respond(asserted).await;
    }
}
