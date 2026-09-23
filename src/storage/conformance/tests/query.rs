// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests that `fuchsia.unknown/Queryable.Query` returns the correct protocol identifier.
//!
//! All fuchsia.io node protocols compose Queryable. The string returned by this method should match
//! the discoverable name for the protocol that was negotiated for that connection. This is distinct
//! from the underlying object type (see `fuchsia.io/Node.GetAttributes`).

use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn directory_query() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], fio::PERM_READABLE);
    {
        let dir = dir
            .open_node::<fio::DirectoryMarker>(".", fio::Flags::PROTOCOL_DIRECTORY, None)
            .await
            .unwrap();
        let protocol = dir.query().await.expect("query failed");
        assert_eq!(std::str::from_utf8(&protocol), Ok(fio::DirectoryMarker::PROTOCOL_NAME));
    }
    {
        let node =
            dir.open_node::<fio::NodeMarker>(".", fio::Flags::PROTOCOL_NODE, None).await.unwrap();
        let protocol = node.query().await.expect("query failed");
        assert_eq!(std::str::from_utf8(&protocol), Ok(fio::NodeMarker::PROTOCOL_NAME));
    }
}

#[fuchsia::test]
async fn file_query() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, fio::PERM_READABLE);
    {
        let file = dir
            .open_node::<fio::FileMarker>(TEST_FILE, fio::Flags::PROTOCOL_FILE, None)
            .await
            .unwrap();
        let protocol = file.query().await.expect("query failed");
        assert_eq!(std::str::from_utf8(&protocol), Ok(fio::FileMarker::PROTOCOL_NAME));
    }
    {
        let node = dir
            .open_node::<fio::NodeMarker>(TEST_FILE, fio::Flags::PROTOCOL_NODE, None)
            .await
            .unwrap();
        let protocol = node.query().await.expect("query failed");
        assert_eq!(std::str::from_utf8(&protocol), Ok(fio::NodeMarker::PROTOCOL_NAME));
    }
}
