// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fidl::router;
use crate::{Data, Router, WeakInstanceToken};
use fidl::AsHandleRef;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_component_sandbox as fsandbox;
use futures::TryStreamExt;
use std::sync::Arc;

impl crate::fidl::IntoFsandboxCapability for Arc<Router<Data>> {
    fn into_fsandbox_capability(self, token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        fsandbox::Capability::DataRouter(self.into_fsandbox_router(token))
    }
}

impl Router<Data> {
    fn into_fsandbox_router(
        self: Arc<Self>,
        token: Arc<WeakInstanceToken>,
    ) -> ClientEnd<fsandbox::DataRouterMarker> {
        let (client_end, sender_stream) =
            fidl::endpoints::create_request_stream::<fsandbox::DataRouterMarker>();
        self.serve_and_register(sender_stream, client_end.as_handle_ref().koid().unwrap(), token);
        client_end
    }

    async fn serve_router(
        self: Arc<Self>,
        mut stream: fsandbox::DataRouterRequestStream,
        token: Arc<WeakInstanceToken>,
    ) -> Result<(), fidl::Error> {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fsandbox::DataRouterRequest::Route { payload, responder } => {
                    let resp = match router::route_from_fidl(&self, payload, token.clone()).await {
                        Ok(Some(c)) => {
                            let data = c.to_fsandbox();
                            Ok(fsandbox::DataRouterRouteResponse::Data(data))
                        }
                        Ok(None) => {
                            Ok(fsandbox::DataRouterRouteResponse::Unavailable(fsandbox::Unit {}))
                        }
                        Err(e) => Err(e),
                    };
                    responder.send(resp)?;
                }
                fsandbox::DataRouterRequest::_UnknownMethod { ordinal, .. } => {
                    log::warn!(ordinal:%; "Received unknown DataRouter request");
                }
            }
        }
        Ok(())
    }

    /// Serves the `fuchsia.sandbox.Router` protocol and moves ourself into the registry.
    pub fn serve_and_register(
        self: Arc<Self>,
        stream: fsandbox::DataRouterRequestStream,
        koid: zx::Koid,
        token: Arc<WeakInstanceToken>,
    ) {
        let router = self.clone();

        // Move this capability into the registry.
        crate::fidl::registry::insert(self.into(), koid, async move {
            router.serve_router(stream, token).await.expect("failed to serve Router");
        });
    }
}
