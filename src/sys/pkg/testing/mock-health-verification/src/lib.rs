#![allow(unused_crate_dependencies)]
// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_update_verify as fidl;
use fuchsia_async::Task;
use futures::{FutureExt as _, StreamExt as _, future};
use std::sync::Arc;

pub trait Hook: Send + Sync {
    fn query_health_checks(&self) -> future::BoxFuture<'static, Result<(), zx::Status>>;
}

impl<F> Hook for F
where
    F: Fn() -> Result<(), zx::Status> + Send + Sync,
{
    fn query_health_checks(&self) -> future::BoxFuture<'static, Result<(), zx::Status>> {
        future::ready(self()).boxed()
    }
}

pub struct MockHealthVerificationService {
    call_hook: Box<dyn Hook>,
}

impl MockHealthVerificationService {
    /// Creates a new MockHealthVerificationService with a given callback to run per call to the service.
    pub fn new(hook: impl Hook + 'static) -> Self {
        Self { call_hook: Box::new(hook) }
    }

    pub fn spawn_health_verification_service(
        self: Arc<Self>,
    ) -> (fidl::HealthVerificationProxy, Task<()>) {
        let (proxy, stream) =
            ::fidl::endpoints::create_proxy_and_stream::<fidl::HealthVerificationMarker>();

        let task = Task::spawn(self.run_health_verification_service(stream));

        (proxy, task)
    }

    /// Serves fuchsia.update.verify/HealthVerification.QueryHealthChecks
    pub async fn run_health_verification_service(
        self: Arc<Self>,
        stream: fidl::HealthVerificationRequestStream,
    ) {
        let Self { call_hook } = &*self;
        stream
            .for_each(|request| match request.expect("received verifier request") {
                fidl::HealthVerificationRequest::QueryHealthChecks { responder } => {
                    call_hook.query_health_checks().map(|res| {
                        responder
                            .send(zx::Status::result_into_raw(res))
                            .expect("sent verifier response")
                    })
                }
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_mock_verifier() {
        let mock = Arc::new(MockHealthVerificationService::new(|| Ok(())));
        let (proxy, _server) = mock.spawn_health_verification_service();

        let verify_result = proxy.query_health_checks().await.expect("made fidl call");

        assert_eq!(verify_result, zx::sys::ZX_OK);
    }
}
