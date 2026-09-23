// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use fidl_fuchsia_pkg_garbagecollector::{
    GcError, ManagerMarker, ManagerProxy, ManagerRequest, ManagerRequestStream,
};
use fuchsia_async as fasync;
use futures::prelude::*;
use std::sync::Arc;

type CallHook = Box<dyn Fn() -> Result<(), GcError> + Send + Sync>;

pub struct MockSpaceService {
    call_hook: CallHook,
}

impl MockSpaceService {
    pub fn new(call_hook: CallHook) -> Self {
        Self { call_hook }
    }

    pub fn spawn_space_service(self: &Arc<Self>) -> ManagerProxy {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<ManagerMarker>();

        fasync::Task::spawn(
            Arc::clone(self)
                .run_space_service(stream)
                .unwrap_or_else(|e| panic!("error running space service: {:#}", anyhow!(e))),
        )
        .detach();

        proxy
    }

    pub async fn run_space_service(
        self: Arc<Self>,
        mut stream: ManagerRequestStream,
    ) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await.expect("received request") {
            let ManagerRequest::Gc { responder } = event else {
                return Err(anyhow!("Unknown method called on garbage collector."));
            };
            responder.send((self.call_hook)())?;
        }

        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[fuchsia::test]
    async fn test_mock_space() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = Arc::clone(&called);
        let mock = Arc::new(MockSpaceService::new(Box::new(move || {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })));
        let proxy = mock.spawn_space_service();

        assert_eq!(called.load(Ordering::SeqCst), 0);

        let gc_result = proxy.gc().await.expect("made fidl call");
        assert_eq!(gc_result, Ok(()));
        assert_eq!(called.load(Ordering::SeqCst), 1);

        let gc_result = proxy.gc().await.expect("made fidl call");
        assert_eq!(gc_result, Ok(()));

        let gc_result = proxy.gc().await.expect("made fidl call");
        assert_eq!(gc_result, Ok(()));

        assert_eq!(called.load(Ordering::SeqCst), 3);
    }

    #[fuchsia::test]
    async fn test_mock_error() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = Arc::clone(&called);
        let mock = Arc::new(MockSpaceService::new(Box::new(move || {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Err(GcError::Internal)
        })));
        let proxy = mock.spawn_space_service();

        let gc_result = proxy.gc().await.expect("made fidl call");
        assert_eq!(gc_result, Err(GcError::Internal));

        assert_eq!(called.load(Ordering::SeqCst), 1);
    }
}
