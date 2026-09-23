// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod config;

use config::new_builder;
use fidl_fuchsia_hardware_inlineencryption as finline;
use fuchsia_async as _;
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use std::sync::Arc;
use test_vmo_backed_block_server::VmoBackedServer;

#[fuchsia::test]
async fn test_inline_encryption_routing() {
    // Create a VmoBackedServer to simulate the block device.
    let vmo = zx::Vmo::create(1024 * 1024).unwrap(); // 1MB
    let server = Arc::new(VmoBackedServer::from_vmo(512, vmo).unwrap());

    let mut builder = new_builder();
    builder = builder.with_simulated_gpt(server.clone());

    let fixture = builder.build().await;

    // Wait for fshost to expose the service.
    let exposed_dir = fixture.realm.root.get_exposed_dir();

    let inline_encrypt_proxy =
        connect_to_protocol_at_dir_root::<finline::DeviceMarker>(exposed_dir)
            .expect("failed to connect to inline encryption device");

    // Call a method to verify forwarding.
    let test_key = vec![0x12, 0x34, 0x56, 0x78];
    let result = inline_encrypt_proxy.derive_raw_secret(&test_key).await.expect("FIDL call failed");

    assert!(result.is_ok());
    let returned_key = result.unwrap();

    // Mock implementation swaps nibbles: *b = *b >> 4 | *b << 4;
    let expected_key: Vec<u8> = test_key.iter().map(|&b| b >> 4 | b << 4).collect();
    assert_eq!(returned_key, expected_key);

    fixture.tear_down().await;
}
