// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf::AsyncDispatcher;
use fidl_next_fuchsia_hardware_serial as serial;
use fidl_next_fuchsia_hardware_serialimpl as serialimpl;
use fuchsia_sync::Mutex;
use std::sync::Arc;

pub const FAKE_SERIAL_PID: u32 = 0x1234;

#[derive(Debug)]
pub struct FakeSerialState {
    pub info_result: Result<serial::SerialPortInfo, zx::Status>,
    pub enable_result: Result<(), zx::Status>,
    pub enabled: bool,
    pub cancel_all_count: usize,
}

impl Default for FakeSerialState {
    fn default() -> Self {
        Self {
            info_result: Ok(serial::SerialPortInfo {
                serial_class: serial::Class::BluetoothHci,
                serial_vid: 0,
                serial_pid: FAKE_SERIAL_PID,
            }),
            enable_result: Ok(()),
            enabled: false,
            cancel_all_count: 0,
        }
    }
}

#[derive(Debug)]
struct FakeSerialServer {
    state: Arc<Mutex<FakeSerialState>>,
}

impl serialimpl::DeviceServerHandler<fdf_fidl::DriverChannel> for FakeSerialServer {
    async fn get_info(
        &mut self,
        responder: fidl_next::Responder<serialimpl::device::GetInfo, fdf_fidl::DriverChannel>,
    ) {
        let info_result = self.state.lock().info_result;
        match info_result {
            Ok(info) => {
                let _ = responder.respond(&info).await;
            }
            Err(status) => {
                let _ = responder.respond_err(status).await;
            }
        }
    }

    async fn config(
        &mut self,
        _request: fidl_next::Request<serialimpl::device::Config, fdf_fidl::DriverChannel>,
        responder: fidl_next::Responder<serialimpl::device::Config, fdf_fidl::DriverChannel>,
    ) {
        let _ = responder.respond(()).await;
    }

    async fn enable(
        &mut self,
        request: fidl_next::Request<serialimpl::device::Enable, fdf_fidl::DriverChannel>,
        responder: fidl_next::Responder<serialimpl::device::Enable, fdf_fidl::DriverChannel>,
    ) {
        let enable_result = {
            let mut state = self.state.lock();
            if state.enable_result.is_ok() {
                state.enabled = request.payload().enable;
            }
            state.enable_result
        };
        match enable_result {
            Ok(()) => {
                let _ = responder.respond(()).await;
            }
            Err(status) => {
                let _ = responder.respond_err(status).await;
            }
        }
    }

    async fn read(
        &mut self,
        responder: fidl_next::Responder<serialimpl::device::Read, fdf_fidl::DriverChannel>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn write(
        &mut self,
        _request: fidl_next::Request<serialimpl::device::Write, fdf_fidl::DriverChannel>,
        responder: fidl_next::Responder<serialimpl::device::Write, fdf_fidl::DriverChannel>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn cancel_all(
        &mut self,
        responder: fidl_next::Responder<serialimpl::device::CancelAll, fdf_fidl::DriverChannel>,
    ) {
        {
            let mut state = self.state.lock();
            state.cancel_all_count += 1;
        }
        let _ = responder.respond(()).await;
    }
}

#[derive(Debug)]
pub struct FakeSerialService {
    dispatcher: AsyncDispatcher,
    state: Arc<Mutex<FakeSerialState>>,
}

impl FakeSerialService {
    pub fn new(dispatcher: AsyncDispatcher, state: Arc<Mutex<FakeSerialState>>) -> Self {
        Self { dispatcher, state }
    }
}

impl serialimpl::ServiceHandler for FakeSerialService {
    fn device(
        &self,
        server_end: fidl_next::ServerEnd<serialimpl::Device, fdf_fidl::DriverChannel>,
    ) {
        let fake_server = FakeSerialServer { state: self.state.clone() };
        server_end.spawn_on(fake_server, &fdf_fidl::FidlExecutor::from(self.dispatcher.clone()));
    }
}
