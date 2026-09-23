// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fidl_fidl_examples_echo as fecho;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use std::sync::Arc;

enum IncomingRequest {
    EchoService(fecho::EchoServiceRequest),
}

async fn handle_echo_stream(mut stream: fecho::EchoRequestStream) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fecho::EchoRequest::EchoString { value, responder } => {
                let _ = responder.send(value.as_deref());
            }
        }
    }
}

struct DtEvalDriver {
    _node: Node,
    _scope: Arc<fasync::Scope>,
}

driver_register!(DtEvalDriver);

impl Driver for DtEvalDriver {
    const NAME: &str = "dt-eval-driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;

        let scope = Arc::new(fasync::Scope::new_with_name("dt-eval-driver"));
        let mut fs = ServiceFs::new();

        fs.dir("svc").add_fidl_service_instance("default", IncomingRequest::EchoService);

        context.serve_outgoing(&mut fs)?;

        scope.spawn_local({
            let scope = Arc::clone(&scope);
            async move {
                fs.for_each_concurrent(None, |request| {
                    let scope = Arc::clone(&scope);
                    async move {
                        match request {
                            IncomingRequest::EchoService(fecho::EchoServiceRequest::Echo(
                                stream,
                            )) => {
                                scope.spawn_local(handle_echo_stream(stream));
                            }
                        }
                    }
                })
                .await;
            }
        });

        Ok(DtEvalDriver { _node: node, _scope: scope })
    }

    async fn stop(&self) {}
}
