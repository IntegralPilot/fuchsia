// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_boot as fuchsia_boot;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use std::sync::Arc;
use zx::Channel;

pub struct SvcStashCapability {
    channel: Mutex<Option<fidl::endpoints::ServerEnd<fuchsia_boot::SvcStashMarker>>>,
}

impl SvcStashCapability {
    pub fn new(channel: Channel) -> Arc<Self> {
        Arc::new(Self { channel: Mutex::new(Some(fidl::endpoints::ServerEnd::new(channel))) })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fuchsia_boot::SvcStashProviderRequestStream,
    ) -> Result<(), Error> {
        while let Some(fuchsia_boot::SvcStashProviderRequest::Get { responder }) =
            stream.try_next().await?
        {
            // If the channel is valid return it, if not ZX_ERR_NO_UNAVAILABLE.
            let channel = self.channel.lock().take();
            match channel {
                Some(channel) => responder.send(Ok(channel))?,
                None => responder.send(Err(zx::Status::UNAVAILABLE.into_raw()))?,
            }
        }
        Ok(())
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use zx::{AsHandleRef, sys};

    // Just need a channel to stash.
    async fn get_svc_stash_handle() -> Result<Channel, Error> {
        let (_p1, p2) = Channel::create();
        Ok(p2)
    }

    #[fuchsia::test]
    async fn second_call_fails() -> Result<(), Error> {
        let svc_stash_provider = SvcStashCapability::new(get_svc_stash_handle().await?);
        let (proxy, stream) = create_proxy_and_stream::<fuchsia_boot::SvcStashProviderMarker>();
        let _task = fasync::Task::spawn(svc_stash_provider.serve(stream));
        let svc_stash = proxy.get().await?;
        assert_ne!(svc_stash.unwrap().as_handle_ref().raw_handle(), sys::ZX_HANDLE_INVALID);

        // Second call must fail.
        let svc_stash_2 = proxy.get().await?;
        assert!(svc_stash_2.is_err());
        Ok(())
    }
}
