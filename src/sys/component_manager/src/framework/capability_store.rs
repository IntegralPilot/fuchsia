// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::WeakComponentInstance;
use crate::sandbox_util::take_handle_as_stream;
use fidl_fuchsia_component_sandbox as fsandbox;
use fuchsia_async as fasync;
use futures::FutureExt;
use futures::future::BoxFuture;
use std::sync::LazyLock;

static RECEIVER_SCOPE: LazyLock<fasync::Scope> = LazyLock::new(|| fasync::Scope::new());

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let stream = take_handle_as_stream::<fsandbox::CapabilityStoreMarker>(server_end);
        runtime_capabilities::serve_capability_store(stream, &*RECEIVER_SCOPE, source.into())
            .await
            .map_err(Into::into)
    }
    .boxed()
}
