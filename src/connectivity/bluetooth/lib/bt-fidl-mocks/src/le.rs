// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::expect::{expect_call, Status};
use anyhow::Error;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_bluetooth::PeerId;
use log::info;
use zx::MonotonicDuration;
use {fidl_fuchsia_bluetooth_gatt2 as fidl_gatt2, fidl_fuchsia_bluetooth_le as fidl_le};

/// Provides a simple mock implementation of a `fuchsia.bluetooth.le/Central`
pub struct CentralMock {
    stream: fidl_le::CentralRequestStream,
    timeout: zx::MonotonicDuration,
}

impl CentralMock {
    pub fn new(timeout: MonotonicDuration) -> (fidl_le::CentralProxy, Self) {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fidl_le::CentralMarker>();
        (proxy, Self { stream, timeout })
    }

    pub fn from_stream(stream: fidl_le::CentralRequestStream, timeout: MonotonicDuration) -> Self {
        Self { stream, timeout }
    }

    pub async fn expect_connect(
        &mut self,
        expected_peer_id: Option<PeerId>,
    ) -> Result<(fidl_le::ConnectionOptions, ServerEnd<fidl_le::ConnectionMarker>), Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            fidl_le::CentralRequest::Connect { id, options, handle, .. } => {
                if let Some(match_id) = expected_peer_id {
                    assert_eq!(match_id, id);
                };
                Ok(Status::Satisfied((options, handle)))
            }
            x => {
                info!("Received unexpected Central Request {x:?}");
                Ok(Status::Pending)
            }
        })
        .await
    }
}

/// Provides a simple mock implementation of a `fuchsia.bluetooth.le/Connection`
pub struct ConnectionMock {
    stream: fidl_le::ConnectionRequestStream,
    timeout: zx::MonotonicDuration,
}

impl ConnectionMock {
    pub fn new(timeout: MonotonicDuration) -> (fidl_le::ConnectionProxy, Self) {
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_le::ConnectionMarker>();
        (proxy, Self { stream, timeout })
    }

    pub fn from_stream(
        stream: fidl_le::ConnectionRequestStream,
        timeout: MonotonicDuration,
    ) -> Self {
        Self { stream, timeout }
    }

    pub async fn expect_request_gatt_client(
        &mut self,
    ) -> Result<ServerEnd<fidl_gatt2::ClientMarker>, Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            fidl_le::ConnectionRequest::RequestGattClient { client, .. } => {
                Ok(Status::Satisfied(client))
            }
            x => {
                info!("Received unexpected Connection request: {x:?}");
                Ok(Status::Pending)
            }
        })
        .await
    }
}
