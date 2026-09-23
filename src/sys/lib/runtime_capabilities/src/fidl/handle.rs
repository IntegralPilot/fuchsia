// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Handle, WeakInstanceToken};
use fidl_fuchsia_component_sandbox as fsandbox;
use std::sync::Arc;

impl crate::fidl::IntoFsandboxCapability for Arc<Handle> {
    fn into_fsandbox_capability(self, _token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        // This is safe because this is only called during
        // `fuchsia.component.sandbox.CapabilityStore/Export`, and by design it's not possible to
        // export the same capability twice.
        fsandbox::Capability::Handle(self.take().expect("failed to take handle"))
    }
}
