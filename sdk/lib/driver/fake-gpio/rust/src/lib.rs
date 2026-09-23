// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;
use fidl_next::{Request, Responder};
use fidl_next_fuchsia_hardware_gpio::{self as fgpio, GpioServerHandler};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use fuchsia_sync::Mutex;
use std::sync::Arc;

struct FakeGpioState {
    read_value: bool,
    buffer_mode: fgpio::BufferMode,
    interrupt: zx::Interrupt,
    client_has_interrupt: bool,
}

pub struct FakeGpio {
    state: Arc<Mutex<FakeGpioState>>,
}

impl Default for FakeGpio {
    fn default() -> Self {
        Self::new(zx::Interrupt::invalid())
    }
}

impl FakeGpio {
    pub fn new(interrupt: zx::Interrupt) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeGpioState {
                read_value: false,
                buffer_mode: fgpio::BufferMode::Input,
                interrupt,
                client_has_interrupt: false,
            })),
        }
    }

    pub fn set_read_value(&self, read_value: bool) {
        self.state.lock().read_value = read_value;
    }

    pub fn buffer_mode(&self) -> fgpio::BufferMode {
        self.state.lock().buffer_mode
    }

    pub fn set_buffer_mode(&self, buffer_mode: fgpio::BufferMode) {
        self.state.lock().buffer_mode = buffer_mode;
    }

    pub fn serve<O: ServiceObjTrait>(
        &self,
        service_fs: &mut ServiceFs<O>,
        scope: fasync::ScopeHandle,
        instance_name: &str,
    ) -> fdf::Offer {
        fdf_component::ServiceOffer::<fgpio::Service>::new_next()
            .add_default_named_next(
                service_fs,
                instance_name,
                FakeGpioService { state: self.state.clone(), scope },
            )
            .build_zircon_offer_next()
    }
}

struct FakeGpioService {
    state: Arc<Mutex<FakeGpioState>>,
    scope: fasync::ScopeHandle,
}

impl fgpio::ServiceHandler for FakeGpioService {
    fn device(&self, server_end: fidl_next::ServerEnd<fgpio::Gpio>) {
        server_end.spawn_on(FakeGpioServer { state: self.state.clone() }, &self.scope);
    }
}

struct FakeGpioServer {
    state: Arc<Mutex<FakeGpioState>>,
}

impl GpioServerHandler for FakeGpioServer {
    async fn read(&mut self, responder: Responder<fgpio::gpio::Read>) {
        let read_value = self.state.lock().read_value;
        let _ = responder.respond(read_value).await;
    }

    async fn set_buffer_mode(
        &mut self,
        request: Request<fgpio::gpio::SetBufferMode>,
        responder: Responder<fgpio::gpio::SetBufferMode>,
    ) {
        self.state.lock().buffer_mode = request.payload().mode;
        let _ = responder.respond(()).await;
    }

    async fn get_interrupt(
        &mut self,
        _request: Request<fgpio::gpio::GetInterrupt>,
        responder: Responder<fgpio::gpio::GetInterrupt>,
    ) {
        let result: Result<zx::Interrupt, zx::Status> = {
            let mut state = self.state.lock();
            if state.client_has_interrupt {
                Err(zx::Status::ACCESS_DENIED)
            } else if state.interrupt.is_invalid() {
                Err(zx::Status::NOT_SUPPORTED)
            } else {
                state.client_has_interrupt = true;
                state.interrupt.duplicate_handle(zx::Rights::SAME_RIGHTS)
            }
        };
        match result {
            Ok(interrupt) => {
                let _ = responder.respond(interrupt).await;
            }
            Err(e) => {
                let _ = responder.respond_err(e).await;
            }
        }
    }

    async fn configure_interrupt(
        &mut self,
        _request: Request<fgpio::gpio::ConfigureInterrupt>,
        responder: Responder<fgpio::gpio::ConfigureInterrupt>,
    ) {
        let _ = responder.respond(()).await;
    }

    async fn release_interrupt(&mut self, responder: Responder<fgpio::gpio::ReleaseInterrupt>) {
        self.state.lock().client_has_interrupt = false;
        let _ = responder.respond(()).await;
    }
}
