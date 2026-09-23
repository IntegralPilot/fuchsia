// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_hardware_clock::{self as fclock, ClockServerHandler};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use fuchsia_sync::Mutex;
use std::sync::Arc;

struct FakeClockState {
    enabled: bool,
    rate: u64,
}

pub struct FakeClock {
    state: Arc<Mutex<FakeClockState>>,
}

impl FakeClock {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(FakeClockState { enabled: false, rate: 0 })) }
    }

    pub fn enabled(&self) -> bool {
        self.state.lock().enabled
    }

    pub fn rate(&self) -> u64 {
        self.state.lock().rate
    }

    pub fn set_rate(&self, hz: u64) {
        self.state.lock().rate = hz;
    }

    pub fn serve<O: ServiceObjTrait>(
        &self,
        service_fs: &mut ServiceFs<O>,
        scope: fasync::ScopeHandle,
        instance_name: &str,
    ) -> fdf::Offer {
        fdf_component::ServiceOffer::<fclock::Service>::new_next()
            .add_default_named_next(
                service_fs,
                instance_name,
                FakeClockService { state: self.state.clone(), scope },
            )
            .build_zircon_offer_next()
    }
}

struct FakeClockService {
    state: Arc<Mutex<FakeClockState>>,
    scope: fasync::ScopeHandle,
}

impl fclock::ServiceHandler for FakeClockService {
    fn clock(&self, server_end: fidl_next::ServerEnd<fclock::Clock>) {
        server_end.spawn_on(FakeClockServer { state: self.state.clone() }, &self.scope);
    }
}

struct FakeClockServer {
    state: Arc<Mutex<FakeClockState>>,
}

impl ClockServerHandler for FakeClockServer {
    async fn enable(&mut self, responder: Responder<fclock::clock::Enable>) {
        self.state.lock().enabled = true;
        let _ = responder.respond(()).await;
    }

    async fn disable(&mut self, responder: Responder<fclock::clock::Disable>) {
        self.state.lock().enabled = false;
        let _ = responder.respond(()).await;
    }

    async fn is_enabled(&mut self, responder: Responder<fclock::clock::IsEnabled>) {
        let enabled = self.state.lock().enabled;
        let _ = responder.respond(enabled).await;
    }

    async fn set_rate(
        &mut self,
        request: Request<fclock::clock::SetRate>,
        responder: Responder<fclock::clock::SetRate>,
    ) {
        self.state.lock().rate = request.payload().hz;
        let _ = responder.respond(()).await;
    }

    async fn query_supported_rate(
        &mut self,
        _request: Request<fclock::clock::QuerySupportedRate>,
        responder: Responder<fclock::clock::QuerySupportedRate>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn get_rate(&mut self, responder: Responder<fclock::clock::GetRate>) {
        let rate = self.state.lock().rate;
        let _ = responder.respond(rate).await;
    }

    async fn set_input(
        &mut self,
        _request: Request<fclock::clock::SetInput>,
        responder: Responder<fclock::clock::SetInput>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn get_num_inputs(&mut self, responder: Responder<fclock::clock::GetNumInputs>) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn get_input(&mut self, responder: Responder<fclock::clock::GetInput>) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn get_properties(&mut self, responder: Responder<fclock::clock::GetProperties>) {
        let _ = responder
            .respond(fclock::natural::ClockGetPropertiesResponse {
                id: 0,
                name: "clock".to_string(),
            })
            .await;
    }
}
