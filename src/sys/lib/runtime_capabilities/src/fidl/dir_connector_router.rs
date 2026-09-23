// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fidl::router;
use crate::{DirConnector, Router, WeakInstanceToken};
use fidl::AsHandleRef;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_component_sandbox as fsandbox;
use futures::TryStreamExt;
use std::sync::Arc;

impl crate::fidl::IntoFsandboxCapability for Arc<Router<DirConnector>> {
    fn into_fsandbox_capability(self, token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        fsandbox::Capability::DirConnectorRouter(self.into_fsandbox_router(token))
    }
}

impl Router<DirConnector> {
    fn into_fsandbox_router(
        self: Arc<Self>,
        token: Arc<WeakInstanceToken>,
    ) -> ClientEnd<fsandbox::DirConnectorRouterMarker> {
        let (client_end, sender_stream) =
            fidl::endpoints::create_request_stream::<fsandbox::DirConnectorRouterMarker>();
        self.serve_and_register(sender_stream, client_end.as_handle_ref().koid().unwrap(), token);
        client_end
    }

    async fn serve_router(
        self: Arc<Self>,
        mut stream: fsandbox::DirConnectorRouterRequestStream,
        token: Arc<WeakInstanceToken>,
    ) -> Result<(), fidl::Error> {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fsandbox::DirConnectorRouterRequest::Route { payload, responder } => {
                    let resp = match router::route_from_fidl(&self, payload, token.clone()).await {
                        Ok(Some(c)) => {
                            let dir_connector = c.to_fsandbox();
                            Ok(fsandbox::DirConnectorRouterRouteResponse::DirConnector(
                                dir_connector,
                            ))
                        }
                        Ok(None) => Ok(fsandbox::DirConnectorRouterRouteResponse::Unavailable(
                            fsandbox::Unit {},
                        )),
                        Err(e) => Err(e),
                    };
                    responder.send(resp)?;
                }
                fsandbox::DirConnectorRouterRequest::_UnknownMethod { ordinal, .. } => {
                    log::warn!(
                        ordinal:%;
                        "Received unknown DirConnectorRouter request"
                    );
                }
            }
        }
        Ok(())
    }

    /// Serves the `fuchsia.sandbox.Router` protocol and moves ourself into the registry.
    pub fn serve_and_register(
        self: Arc<Self>,
        stream: fsandbox::DirConnectorRouterRequestStream,
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
