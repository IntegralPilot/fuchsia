// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl::Error;

use futures::StreamExt;

/// Tests that a regular component cannot use RealmDebug because it is a
/// privileged protocol.
#[fuchsia::test]
async fn use_realm_moniker_disallowed() {
    let client = fuchsia_component::client::connect_to_protocol::<
        fidl_fuchsia_component::IntrospectorMarker,
    >()
    .unwrap();
    let mut events = client.take_event_stream();
    let error = events.next().await.unwrap().unwrap_err();
    assert_matches!(
        error,
        Error::ClientChannelClosed { epitaph, .. }
        if epitaph == zx::Status::ACCESS_DENIED
    );
}
