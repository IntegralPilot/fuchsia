// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_update as fupdate;
use futures::prelude::*;
use std::sync::Arc;

pub struct FuchsiaUpdateFidlServer {
    p_external: zx::EventPair,
    wait_for_status_check: future::Shared<futures::future::BoxFuture<'static, Result<(), String>>>,
    idle_timeout: zx::MonotonicDuration,
}

impl FuchsiaUpdateFidlServer {
    pub fn new(
        p_external: zx::EventPair,
        wait_for_status_check: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
        idle_timeout: zx::MonotonicDuration,
    ) -> Self {
        Self {
            p_external,
            wait_for_status_check: wait_for_status_check.boxed().shared(),
            idle_timeout,
        }
    }

    /// Handle a fuchsia.update/CommitStatusProvider request stream.
    pub async fn handle_commit_status_provider_stream(
        self: Arc<Self>,
        stream: fupdate::CommitStatusProviderRequestStream,
    ) -> Result<(), Error> {
        let (stream, unbind_if_stalled) = detect_stall::until_stalled(stream, self.idle_timeout);
        let mut stream = std::pin::pin!(stream);
        while let Some(request) =
            stream.try_next().await.context("while receiving CommitStatusProvider request")?
        {
            let () = Self::handle_request(self.clone(), request).await?;
        }

        if let Ok(Some(server_end)) = unbind_if_stalled.await {
            fuchsia_component::client::connect_channel_to_protocol_at::<
                fupdate::CommitStatusProviderMarker,
            >(server_end, "/escrow")
            .context("escrowing stream")?;
        }

        Ok(())
    }

    async fn handle_request(
        self: Arc<Self>,
        req: fupdate::CommitStatusProviderRequest,
    ) -> Result<(), Error> {
        // The server should only unblock when either of these conditions are met:
        // * The current configuration was already committed on boot and p_external already has
        //   `USER_0` asserted.
        // * The current configuration was pending on boot (p_external may or may not have `USER_0`
        //   asserted depending on how quickly the system is committed).
        //
        // The common case is that the current configuration is committed on boot, so this
        // arrangement avoids the race condition where a client (e.g. an update checker) queries
        // the commit status right after boot, sees that `USER_0` is not yet asserted (because the
        // system-update-committer itself is still obtaining the status from the paver) and then
        // postpones its work (e.g. an update check), when it would have been able to continue if
        // it had queried the commit status a couple seconds later (many clients check the
        // instantaneous state of `USER_0` instead of waiting for it to be asserted so that they
        // can report failure and retry later instead of blocking indefinitely).
        //
        // If there is an error with `put_metadata_in_happy_state`, the FIDL server will hang here
        // indefinitely. This is acceptable because we'll Soon™ reboot on error.
        let () = self
            .wait_for_status_check
            .clone()
            .await
            .map_err(|e| anyhow::anyhow!("while unblocking fidl server: {e}"))?;
        let fupdate::CommitStatusProviderRequest::IsCurrentSystemCommitted { responder } = req;
        responder
            .send(
                self.p_external
                    .duplicate_handle(zx::Rights::BASIC)
                    .context("while duplicating p_external")?,
            )
            .context("while sending IsCurrentSystemCommitted response")
    }
}
