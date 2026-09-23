// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;
use fidl_next::Responder;
use fidl_next_fuchsia_hardware_powerdomain::{self as fpowerdomain, DomainServerHandler};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use fuchsia_sync::Mutex;
use std::sync::Arc;

struct FakePowerDomainState {
    enabled: bool,
}

pub struct FakePowerDomain {
    state: Arc<Mutex<FakePowerDomainState>>,
}

impl FakePowerDomain {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(FakePowerDomainState { enabled: false })) }
    }

    pub fn enabled(&self) -> bool {
        self.state.lock().enabled
    }

    pub fn serve<O: ServiceObjTrait>(
        &self,
        service_fs: &mut ServiceFs<O>,
        scope: fasync::ScopeHandle,
        instance_name: &str,
    ) -> fdf::Offer {
        fdf_component::ServiceOffer::<fpowerdomain::Service>::new_next()
            .add_default_named_next(
                service_fs,
                instance_name,
                FakePowerDomainService { state: self.state.clone(), scope },
            )
            .build_zircon_offer_next()
    }
}

struct FakePowerDomainService {
    state: Arc<Mutex<FakePowerDomainState>>,
    scope: fasync::ScopeHandle,
}

impl fpowerdomain::ServiceHandler for FakePowerDomainService {
    fn domain(&self, server_end: fidl_next::ServerEnd<fpowerdomain::Domain>) {
        server_end.spawn_on(FakePowerDomainServer { state: self.state.clone() }, &self.scope);
    }
}

struct FakePowerDomainServer {
    state: Arc<Mutex<FakePowerDomainState>>,
}

impl DomainServerHandler for FakePowerDomainServer {
    async fn enable(&mut self, responder: Responder<fpowerdomain::domain::Enable>) {
        self.state.lock().enabled = true;
        let _ = responder.respond(()).await;
    }

    async fn disable(&mut self, responder: Responder<fpowerdomain::domain::Disable>) {
        self.state.lock().enabled = false;
        let _ = responder.respond(()).await;
    }

    async fn is_enabled(&mut self, responder: Responder<fpowerdomain::domain::IsEnabled>) {
        let enabled = self.state.lock().enabled;
        let _ = responder.respond(enabled).await;
    }
}
