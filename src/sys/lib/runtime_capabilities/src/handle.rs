// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::CapabilityBound;
use fidl::handle;
use fuchsia_sync::Mutex;
use std::sync::Arc;

/// A capability that wraps a single Zircon handle.
#[derive(Debug)]
pub struct Handle(Mutex<Option<handle::NullableHandle>>);

impl Handle {
    /// Creates a new [Handle] containing a Zircon `handle`.
    pub fn new(handle: handle::NullableHandle) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Some(handle))))
    }

    /// Gets a duplicate of the inner handle. Returns None if the handle has been taken or the
    /// duplicate operation fails.
    pub fn duplicate(&self) -> Option<handle::NullableHandle> {
        self.0.lock().as_ref().and_then(|h| h.duplicate_handle(handle::Rights::SAME_RIGHTS).ok())
    }

    /// Removes the inner handle and returns it.
    pub fn take(&self) -> Option<handle::NullableHandle> {
        self.0.lock().take()
    }
}

impl CapabilityBound for Handle {
    fn debug_typename() -> &'static str {
        "Handle"
    }
}

impl Handle {
    pub fn try_clone(&self) -> Result<Arc<Self>, ()> {
        self.duplicate().map(Self::new).ok_or(())
    }
}

#[cfg(target_os = "fuchsia")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fidl::IntoFsandboxCapability;
    use crate::{Capability, WeakInstanceToken};
    use assert_matches::assert_matches;
    use fidl_fuchsia_component_sandbox as fsandbox;

    // Tests converting the Handle to FIDL and back.
    #[fuchsia::test]
    async fn handle_into_fidl() {
        let event = zx::Event::create();
        let expected_koid = event.koid().unwrap();

        let handle = Handle::new(event.into_handle());

        // Convert the OneShotHandle to FIDL and back.
        let fidl_capability: fsandbox::Capability =
            handle.into_fsandbox_capability(WeakInstanceToken::new_invalid());
        assert_matches!(&fidl_capability, fsandbox::Capability::Handle(_));

        let any: Capability = fidl_capability.try_into().unwrap();
        let handle = assert_matches!(any, Capability::Handle(h) => h);

        // Get the handle.
        let handle: zx::NullableHandle = handle.take().unwrap();

        // The handle should be for same Event that was in the original OneShotHandle.
        let got_koid = handle.koid().unwrap();
        assert_eq!(got_koid, expected_koid);
    }

    /// Tests that a Handle can be cloned by duplicating the handle.
    #[fuchsia::test]
    async fn try_clone() {
        let event = zx::Event::create();
        let expected_koid = event.koid().unwrap();

        let handle = Handle::new(event.into_handle());
        let handle = handle.duplicate().unwrap();

        let got_koid = handle.koid().unwrap();
        assert_eq!(got_koid, expected_koid);

        let (ch, _) = zx::Channel::create();
        let handle = Handle::new(ch.into_handle());
        assert_matches!(handle.try_clone(), Err(()));
    }
}
