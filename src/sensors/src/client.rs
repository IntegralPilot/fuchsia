// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_sensors::ManagerControlHandle;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;

static CLIENT_ID_COUNT: AtomicUsize = AtomicUsize::new(0);

// TODO(375043170): Remove this when sensors have a better IPC mechanism.
//
// There is no way to compare control handles, so instead this helper struct will assign a unique
// id to each new instance. This will only be used until sensors no longer need to use
// send_on_sensor_event.
#[derive(Debug, Clone)]
pub struct Client {
    id: usize,
    pub(crate) control_handle: ManagerControlHandle,
}

impl Client {
    pub fn new(control_handle: ManagerControlHandle) -> Self {
        Self { id: CLIENT_ID_COUNT.fetch_add(1, SeqCst), control_handle }
    }
}

impl PartialEq for Client {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Client {}

impl Hash for Client {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::*;
    use fidl_fuchsia_sensors::*;

    #[fuchsia::test]
    async fn test_unique_client_ids() {
        let (_, stream) = create_proxy_and_stream::<ManagerMarker>();
        let client1 = Client::new(stream.control_handle().clone());
        let client2 = Client::new(stream.control_handle().clone());
        assert_ne!(client1.id, client2.id);
        // IDs should start at 0 and monotonically increase.
        assert_eq!(client1.id, 0);
        assert_eq!(client2.id, 1);
    }

    #[fuchsia::test]
    async fn test_client_partial_eq() {
        let (_, stream) = create_proxy_and_stream::<ManagerMarker>();
        let client1 = Client::new(stream.control_handle().clone());
        let client2 = Client::new(stream.control_handle().clone());
        assert_ne!(client1, client2);
        assert_eq!(client1, client1.clone());
    }
}
