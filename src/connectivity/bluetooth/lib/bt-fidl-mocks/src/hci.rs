// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::expect::{Status, expect_call};
use anyhow::Error;
use fidl_fuchsia_hardware_bluetooth::{
    HciTransportMarker, HciTransportProxy, HciTransportRequest, HciTransportRequestStream,
    SentPacket,
};
use zx::MonotonicDuration;

/// Provides a simple mock implementation of `fuchsia.hardware.bluetooth/HciTransport`.
pub struct HciTransportMock {
    stream: HciTransportRequestStream,
    timeout: MonotonicDuration,
}

impl HciTransportMock {
    pub fn new(timeout: MonotonicDuration) -> Result<(HciTransportProxy, HciTransportMock), Error> {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<HciTransportMarker>();
        Ok((proxy, HciTransportMock { stream, timeout }))
    }

    pub fn from_stream(
        stream: HciTransportRequestStream,
        timeout: MonotonicDuration,
    ) -> HciTransportMock {
        HciTransportMock { stream, timeout }
    }

    pub async fn expect_send(&mut self, packet: SentPacket) -> Result<(), Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            HciTransportRequest::Send_ { payload, responder } => {
                let _ = responder.send()?;
                if payload != packet {
                    return Ok(Status::Pending);
                }
                Ok(Status::Satisfied(()))
            }
            _ => Ok(Status::Pending),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeout_duration;
    use futures::join;

    #[fuchsia::test(allow_stalls = false)]
    async fn test_expect_send() {
        let (proxy, mut mock) =
            HciTransportMock::new(timeout_duration()).expect("failed to create mock");
        let send_fut = proxy.send_(&SentPacket::Command(vec![0x03]));
        let expect_fut = mock.expect_send(SentPacket::Command(vec![0x03]));

        let (send_result, expect_result) = join!(send_fut, expect_fut);
        let _ = send_result.expect("send request failed");
        let _ = expect_result.expect("expectation not satisfied");
    }
}
