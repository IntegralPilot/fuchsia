// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_update_verify as fupdate;
use futures::StreamExt;
use std::sync::Arc;
use vfs::service;

/// Make a new vfs service node that implements fuchsia.update.ComponentOtaHealthCheck
pub fn ota_health_check_service() -> Arc<service::Service> {
    service::host(move |mut stream: fupdate::ComponentOtaHealthCheckRequestStream| async move {
        while let Some(request) = stream.next().await {
            match request {
                Ok(fupdate::ComponentOtaHealthCheckRequest::GetHealthStatus {
                    responder, ..
                }) => {
                    // TODO(https://fxbug.dev/42077105): Implement by calling out
                    // to Fxfs' blob volume.
                    responder.send(fupdate::HealthStatus::Healthy).unwrap_or_else(|e| {
                        log::error!("failed to send GetHealthStatus response. error: {:?}", e);
                    });
                }
                Err(e) => {
                    log::error!("ComponentOtaHealthCheck server failed: {:?}", e);
                    return;
                }
            }
        }
    })
}
