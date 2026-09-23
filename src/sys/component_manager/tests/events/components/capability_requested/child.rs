// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fidl_test_components as ftest;
use fuchsia_component::client::connect_to_protocol_at_path;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let client_0 =
        connect_to_protocol_at_path::<ftest::TriggerMarker>("/svc/fuchsia.test.Protocol0").unwrap();
    assert_eq!(client_0.run().await.unwrap(), "0");

    let client_1 =
        connect_to_protocol_at_path::<ftest::TriggerMarker>("/svc/fuchsia.test.Protocol1").unwrap();
    assert_eq!(client_1.run().await.unwrap(), "1");

    Ok(())
}
