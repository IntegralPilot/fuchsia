// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fidl::registry;
use crate::{Connector, Receiver, WeakInstanceToken};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_component_sandbox as fsandbox;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use std::sync::Arc;

impl Connector {
    pub(crate) fn new_with_fidl_receiver(
        receiver_client: ClientEnd<fsandbox::ReceiverMarker>,
        scope: &fasync::Scope,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded();
        let receiver = Receiver::new(receiver);
        // Exits when ServerEnd<Receiver> is closed
        scope.spawn(receiver.handle_receiver(receiver_client.into_proxy()));
        Self::new_sendable(sender)
    }

    pub(crate) fn to_fsandbox(self: Arc<Self>) -> fsandbox::Connector {
        fsandbox::Connector { token: registry::insert_token(self.into()) }
    }
}

impl crate::fidl::IntoFsandboxCapability for Arc<Connector> {
    fn into_fsandbox_capability(self, _token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        fsandbox::Capability::Connector(fsandbox::Connector {
            token: registry::insert_token(self.into()),
        })
    }
}
