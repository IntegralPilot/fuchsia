// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::socket;
use flex_client::fidl::Responder as _;
use flex_fuchsia_developer_ffx_speedtest as fspeedtest;
use fuchsia_async as fasync;
use log::{error, warn};

pub struct Server {
    scope: fasync::Scope,
}

impl Default for Server {
    fn default() -> Self {
        Self::new(fasync::Scope::new_with_name("speedtest-server"))
    }
}

impl Server {
    pub fn new(scope: fasync::Scope) -> Self {
        Self { scope }
    }

    pub fn handle_request(&self, req: fspeedtest::SpeedtestRequest) -> Result<(), fidl::Error> {
        match req {
            fspeedtest::SpeedtestRequest::Ping { responder } => responder.send()?,
            fspeedtest::SpeedtestRequest::SocketDown { socket, params, responder } => {
                match params.try_into() {
                    Ok(params) => {
                        let _: fasync::JoinHandle<()> = self.scope.spawn(async move {
                            let report = socket::Transfer { socket, params }.receive().await;
                            match report {
                                Ok(report) => responder.send(&report.into()).unwrap_or_else(|e| {
                                    if !e.is_closed() {
                                        error!("error responding to transfer {e:?}")
                                    }
                                }),
                                Err(e) => {
                                    error!("SocketDown failed with {e:?}");
                                    responder
                                        .control_handle()
                                        .shutdown_with_epitaph(zx_status::Status::INTERNAL);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("error initiating transfer: {e:?}");
                        responder
                            .control_handle()
                            .shutdown_with_epitaph(zx_status::Status::INVALID_ARGS);
                    }
                }
            }
            fspeedtest::SpeedtestRequest::SocketUp { socket, params, responder } => {
                match params.try_into() {
                    Ok(params) => {
                        let _: fasync::JoinHandle<()> = self.scope.spawn(async move {
                            let report = socket::Transfer { socket, params }.send().await;
                            match report {
                                Ok(report) => responder.send(&report.into()).unwrap_or_else(|e| {
                                    if !e.is_closed() {
                                        error!("error responding to transfer {e:?}")
                                    }
                                }),
                                Err(e) => {
                                    error!("SocketUp failed with {e:?}");
                                    responder
                                        .control_handle()
                                        .shutdown_with_epitaph(zx_status::Status::INTERNAL)
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("error initiating transfer: {e:?}");
                        responder
                            .control_handle()
                            .shutdown_with_epitaph(zx_status::Status::INVALID_ARGS);
                    }
                }
            }
            fspeedtest::SpeedtestRequest::_UnknownMethod { ordinal, .. } => {
                warn!("ignoring unknown method {ordinal}");
            }
        }
        Ok(())
    }
}
