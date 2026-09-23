// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides a driver unit testing library.

mod dut;
pub mod harness;
pub mod node;

use crate::{Driver, server};
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_logger as flogger;
use fuchsia_component::client::connect_channel_to_protocol;
use std::sync::Arc;

/// Returns a reference to the driver, if it has been started.
///
/// # Safety
///
/// The caller must ensure that the `driver_token` is a valid pointer to the `DriverServer`
/// that was returned by the `initialize` function.
pub(crate) unsafe fn get_driver_from_token<'a, T: Driver>(driver_token: usize) -> Option<&'a T> {
    let driver_server = unsafe { &*(driver_token as *const server::DriverServer<T>) };
    driver_server.testing_get_driver()
}

/// Can be used to forward logsink requests to the test's.
pub(crate) fn logsink_connector(server_end: ServerEnd<flogger::LogSinkMarker>) {
    let (stream, handle) = server_end.into_stream_and_control_handle();

    // Since we are using the already initialized logsink, we need to emulate this
    // notification, otherwise the log init on the driver side hangs.
    handle
        .send_on_init(flogger::LogSinkOnInitRequest {
            buffer: None,
            interest: None,
            ..Default::default()
        })
        .expect("failed to send on init");

    drop(handle);

    let inner = stream.into_inner().0;
    let server_inner = Arc::into_inner(inner).expect("no other refs.");
    let channel = server_inner.into_channel().into_zx_channel();

    // Just forward it to the test's logger.
    if let Err(e) = connect_channel_to_protocol::<flogger::LogSinkMarker>(channel) {
        log::warn!("Failed to connect to LogSink: {:?}", e);
    }
}
