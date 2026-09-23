// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;

use crate::header::{Header, HeaderSet};
use crate::operation::ResponseCode;

/// An operation can be rejected with a `ResponseCode` and optional headers describing the
/// reason for rejection.w
pub type ObexOperationError = (ResponseCode, HeaderSet);

/// Helper function for creating an [ObexOperationError] with the specified [ResponseCode]
/// and a [Header::Description] with the reason.
pub fn new_operation_error(code: ResponseCode, reason: &str) -> ObexOperationError {
    (code, HeaderSet::from_header(Header::Description(reason.into())))
}

pub type ObexResult<T> = Result<T, ObexOperationError>;

/// An interface that implements the OBEX Server role.
/// This interface roughly corresponds to the operations defined in OBEX v1.5.
#[async_trait]
pub trait ObexServerHandler {
    /// A request to initiate the CONNECT operation.
    /// `headers` are the informational headers provided by the remote OBEX client.
    ///
    /// Directed OBEX connections are automatically supported by the `ObexServer` and needn't be
    /// handled in this response. Specifically, the `ConnectionIdentifier` is automatically
    /// added and shouldn't be included here.
    ///
    /// Returns `Ok` with any response headers if the CONNECT request is accepted.
    /// Returns `Err` with a rejection code and headers if the CONNECT request is rejected.
    async fn connect(&mut self, headers: HeaderSet) -> ObexResult<HeaderSet>;

    /// A request to disconnect the OBEX connection.
    /// `headers` are the informational headers provided by the remote OBEX client.
    /// Returns informational headers in response to the request.
    async fn disconnect(&mut self, headers: HeaderSet) -> HeaderSet;

    /// A request to set the current working folder on the device.
    /// `headers` are the informational headers provided by the remote OBEX client.
    /// If `backup` is `true`, then the remote requests to backup one directory before setting the
    /// path.
    /// If `create` is `true`, then the remote requests to create the path if it does not exist.
    /// If `create` is `false` and the path doesn't exist, `Err` should be returned.
    /// Returns `Ok` with any response headers if the SET_PATH request is accepted.
    /// Returns `Err` with a rejection code and headers if the SET_PATH request is rejected.
    async fn set_path(
        &mut self,
        headers: HeaderSet,
        backup: bool,
        create: bool,
    ) -> ObexResult<HeaderSet>;

    /// A request to get information about a data payload from the local OBEX server.
    /// `headers` are the headers provided by the remote OBEX client that specify the desired
    /// information.
    /// Returns `Ok` with headers if accepted.
    /// Returns `Err` with a rejection code and optional headers if rejected.
    async fn get_info(&mut self, headers: HeaderSet) -> ObexResult<HeaderSet>;

    /// A request to get data from the local OBEX server.
    /// `headers` are the informational headers provided by the remote OBEX client that identify
    /// the payload to be retrieved.
    /// Returns `Ok` with the payload and optional informational headers if accepted.
    /// Returns `Err` with a rejection code and optional headers if rejected.
    async fn get_data(&mut self, headers: HeaderSet) -> ObexResult<(Vec<u8>, HeaderSet)>;

    /// A request to put data in the local OBEX server.
    /// `data` is the payload to be written.
    /// `headers` are the informational headers provided by the remote OBEX client that describe
    /// the payload.
    /// Returns `Ok` if accepted.
    /// Returns `Err` with a rejection code and optional headers if rejected.
    async fn put(&mut self, data: Vec<u8>, headers: HeaderSet) -> ObexResult<()>;

    /// A request to delete data in the local OBEX server.
    /// `headers` are the informational headers provided by the remote OBEX client that describe
    /// the delete request.
    /// Returns `Ok` if accepted.
    /// Returns `Err` with a rejection code and optional headers if rejected.
    async fn delete(&mut self, headers: HeaderSet) -> ObexResult<()>;
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;

    use fuchsia_sync::Mutex;
    use std::sync::Arc;

    #[derive(Default)]
    struct TestApplicationProfileInner {
        generic_response: Option<ObexResult<HeaderSet>>,
        get_response: Option<Result<(Vec<u8>, HeaderSet), ObexOperationError>>,
        put_response: Option<Result<(), ObexOperationError>>,
        received_put_data: Option<(Vec<u8>, HeaderSet)>,
    }

    #[derive(Clone)]
    pub(crate) struct TestApplicationProfile {
        inner: Arc<Mutex<TestApplicationProfileInner>>,
    }

    impl TestApplicationProfile {
        pub fn new() -> Self {
            Self { inner: Arc::new(Mutex::new(Default::default())) }
        }

        pub fn set_response(&self, response: ObexResult<HeaderSet>) {
            (*self.inner.lock()).generic_response = Some(response);
        }

        pub fn set_get_response(&self, response: (Vec<u8>, HeaderSet)) {
            (*self.inner.lock()).get_response = Some(Ok(response));
        }

        pub fn set_put_response(&self, response: ObexResult<()>) {
            (*self.inner.lock()).put_response = Some(response);
        }

        pub fn put_data(&self) -> (Vec<u8>, HeaderSet) {
            (*self.inner.lock()).received_put_data.take().expect("expect data")
        }
    }

    #[async_trait]
    impl ObexServerHandler for TestApplicationProfile {
        async fn connect(&mut self, _headers: HeaderSet) -> ObexResult<HeaderSet> {
            // Defaults to rejecting with `MethodNotAllowed`.
            self.inner
                .lock()
                .generic_response
                .take()
                .unwrap_or_else(|| Err((ResponseCode::MethodNotAllowed, HeaderSet::new())))
        }

        async fn disconnect(&mut self, _headers: HeaderSet) -> HeaderSet {
            // Disconnect cannot be rejected so just take the response headers if they exist or
            // default to returning an empty HeaderSet.
            match self.inner.lock().generic_response.take() {
                Some(Ok(headers)) => headers,
                _ => HeaderSet::new(),
            }
        }

        async fn set_path(
            &mut self,
            _headers: HeaderSet,
            _backup: bool,
            _create: bool,
        ) -> ObexResult<HeaderSet> {
            // Defaults to rejecting with `Forbidden`.
            self.inner
                .lock()
                .generic_response
                .take()
                .unwrap_or_else(|| Err((ResponseCode::Forbidden, HeaderSet::new())))
        }

        async fn get_info(&mut self, _headers: HeaderSet) -> ObexResult<HeaderSet> {
            self.inner
                .lock()
                .generic_response
                .take()
                .unwrap_or_else(|| Err((ResponseCode::NotFound, HeaderSet::new())))
        }

        async fn get_data(&mut self, _headers: HeaderSet) -> ObexResult<(Vec<u8>, HeaderSet)> {
            self.inner
                .lock()
                .get_response
                .take()
                .unwrap_or_else(|| Err((ResponseCode::NotImplemented, HeaderSet::new())))
        }

        async fn put(&mut self, data: Vec<u8>, headers: HeaderSet) -> ObexResult<()> {
            let mut inner = self.inner.lock();
            inner.received_put_data = Some((data, headers));
            inner
                .put_response
                .take()
                .unwrap_or_else(|| Err((ResponseCode::NotImplemented, HeaderSet::new())))
        }

        async fn delete(&mut self, _headers: HeaderSet) -> ObexResult<()> {
            self.inner
                .lock()
                .put_response
                .take()
                .unwrap_or_else(|| Err((ResponseCode::NotImplemented, HeaderSet::new())))
        }
    }
}
