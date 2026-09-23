// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::expect::{Status, expect_call};
use anyhow::Error;
use fidl_fuchsia_bluetooth::Uuid as FidlUuid;
use fidl_fuchsia_bluetooth_gatt::{
    self as gatt, ReadByTypeResult, RemoteServiceMarker, RemoteServiceProxy, RemoteServiceRequest,
    RemoteServiceRequestStream,
};
use fuchsia_bluetooth::types::Uuid;
use zx::MonotonicDuration;

/// Provides a simple mock implementation of `fuchsia.bluetooth.gatt.RemoteService`.
pub struct RemoteServiceMock {
    stream: RemoteServiceRequestStream,
    timeout: MonotonicDuration,
}

impl RemoteServiceMock {
    pub fn new(
        timeout: MonotonicDuration,
    ) -> Result<(RemoteServiceProxy, RemoteServiceMock), Error> {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<RemoteServiceMarker>();
        Ok((proxy, RemoteServiceMock { stream, timeout }))
    }

    /// Wait until a Read By Type message is received with the given `uuid`. `result` will be sent
    /// in response to the matching FIDL request.
    pub async fn expect_read_by_type(
        &mut self,
        expected_uuid: Uuid,
        result: Result<&[ReadByTypeResult], gatt::Error>,
    ) -> Result<(), Error> {
        let expected_uuid: FidlUuid = expected_uuid.into();
        expect_call(&mut self.stream, self.timeout, move |req| {
            if let RemoteServiceRequest::ReadByType { uuid, responder } = req {
                if uuid == expected_uuid {
                    responder.send(result)?;
                    Ok(Status::Satisfied(()))
                } else {
                    // Send error to unexpected request.
                    responder.send(Err(fidl_fuchsia_bluetooth_gatt::Error::Failure))?;
                    Ok(Status::Pending)
                }
            } else {
                Ok(Status::Pending)
            }
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
    async fn test_expect_read_by_type() {
        let (proxy, mut mock) =
            RemoteServiceMock::new(timeout_duration()).expect("failed to create mock");
        let uuid = Uuid::new16(0x180d);
        let result = Ok(&[][..]);

        let fidl_uuid: FidlUuid = uuid.clone().into();
        let read_by_type = proxy.read_by_type(&fidl_uuid);
        let expect = mock.expect_read_by_type(uuid, result);

        let (read_by_type_result, expect_result) = join!(read_by_type, expect);
        let _ = read_by_type_result.expect("read by type request failed");
        let _ = expect_result.expect("expectation not satisfied");
    }
}
