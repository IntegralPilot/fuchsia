// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_elf_test as fet;
use fuchsia_component::client;

#[fuchsia::test]
async fn test_puppet_has_environ_set() {
    let proxy = client::connect_to_protocol::<fet::ContextMarker>()
        .expect("couldn't connect to context service");
    let environ = proxy.get_environ().await.expect("failed to make fidl call");
    // Test the presence of environment variables from the cml manifest.
    // Additional environment variables may come from the component manager config.
    assert!(environ.contains(&"ENVIRONMENT=testing".to_string()));
    assert!(environ.contains(&"threadcount=8".to_string()));
}
