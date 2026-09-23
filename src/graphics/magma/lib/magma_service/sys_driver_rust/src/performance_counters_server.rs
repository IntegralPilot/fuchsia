// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl_fuchsia_gpu_magma as fidl_magma;
use futures::TryStreamExt;

pub struct PerformanceCountersServer {
    event: zx::Event,
}

impl PerformanceCountersServer {
    pub fn new() -> Result<Self, i32> {
        let event = zx::Event::create();
        Ok(PerformanceCountersServer { event })
    }

    pub fn get_event_koid(&self) -> Result<u64, i32> {
        self.event.koid().map(|koid| koid.raw_koid()).map_err(|status| status.into_raw())
    }

    pub async fn run(
        &self,
        mut stream: fidl_magma::PerformanceCounterAccessRequestStream,
    ) -> anyhow::Result<()> {
        while let Some(request) = stream.try_next().await.context("Stream error")? {
            match request {
                fidl_magma::PerformanceCounterAccessRequest::GetPerformanceCountToken {
                    responder,
                } => {
                    let duplicate_event = self
                        .event
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .context("Duplicate handle failed")?;
                    responder.send(duplicate_event).context("Send failed")?;
                }
            }
        }
        Ok(())
    }
}
