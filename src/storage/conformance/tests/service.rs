// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl::endpoints::{DiscoverableProtocolMarker as _, create_proxy};
use fidl_test_placeholders::EchoMarker;

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use futures::TryFutureExt;
use io_conformance_util::DirectoryProxyExt as _;
use io_conformance_util::test_harness::TestHarness;
use zx::Status;

const TEST_STRING: &'static str = "Hello, world!";

/// Opening a service node without any flags or with [`fio::Flags::PROTOCOL_SERVICE`] should connect
/// the channel to the service.
// TODO(https://fxbug.dev/293947862): fuchsia.io/Flags documents that connecting to a service
// requires PROTOCOL_SERVICE to be specified explicitly. However, there are a significant amount of
// existing callers that depend on the legacy io1 behavior allowing no flags to be specified for
// this case. We should either relax this restriction, or work to migrate callers, after which we
// can update this test accordingly.
#[fuchsia::test]
async fn open_service() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_services {
        return;
    }
    let svc_dir = harness.open_service_directory().await;

    for flags in [fio::Flags::empty(), fio::Flags::PROTOCOL_SERVICE] {
        let (echo_proxy, echo_server) = create_proxy::<EchoMarker>();
        svc_dir
            .open(
                EchoMarker::PROTOCOL_NAME,
                flags,
                &Default::default(),
                echo_server.into_channel().into(),
            )
            .unwrap();
        let echo_response = echo_proxy.echo_string(Some(TEST_STRING)).await.unwrap();
        assert_eq!(echo_response.unwrap(), TEST_STRING);
    }
}

// Opening a service node with [`fio::Flags::PROTOCOL_SERVICE`] on a directory connection without
// [`fio::Rights::CONNECT`] must fail with `ZX_ERR_ACCESS_DENIED`.
#[fuchsia::test]
async fn open_service_without_connect_rights_fails() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_services {
        return;
    }
    let svc_dir = harness.open_service_directory().await;

    // Open a restricted directory connection to "." without CONNECT rights.
    let (restricted_dir, server) = create_proxy::<fio::DirectoryMarker>();
    svc_dir
        .open(
            ".",
            fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::PERM_TRAVERSE
                | fio::Flags::PERM_GET_ATTRIBUTES,
            &Default::default(),
            server.into_channel(),
        )
        .expect("open directory failed");

    let (echo_proxy, echo_server) = create_proxy::<EchoMarker>();
    restricted_dir
        .open(
            EchoMarker::PROTOCOL_NAME,
            fio::Flags::PROTOCOL_SERVICE,
            &Default::default(),
            echo_server.into_channel(),
        )
        .expect("open service failed");

    let echo_response_status = echo_proxy
        .echo_string(Some(TEST_STRING))
        .map_err(|e| {
            if let fidl::Error::ClientChannelClosed { epitaph, .. } = e {
                match epitaph.into() {
                    Err(s) => s,
                    Ok(()) => zx::Status::PEER_CLOSED,
                }
            } else {
                panic!("Unhandled FIDL error: {:?}", e);
            }
        })
        .await
        .expect_err("echo_string succeeded");
    assert_eq!(echo_response_status, Status::ACCESS_DENIED);
}

/// Opening a service node with [`fio::Flags::PROTOCOL_NODE`] should open the underlying node.
#[fuchsia::test]
async fn open_service_as_node() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_services {
        return;
    }
    let svc_dir = harness.open_service_directory().await;

    for flags in [
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::PROTOCOL_NODE | fio::Flags::PROTOCOL_SERVICE,
        // As long as PROTOCOL_SERVICE is specified with PROTOCOL_NODE, the request should succeed.
        fio::Flags::PROTOCOL_NODE | fio::Flags::PROTOCOL_SERVICE | fio::Flags::PROTOCOL_FILE,
    ] {
        let (proxy, representation) = svc_dir
            .open_node_repr::<fio::NodeMarker>(
                EchoMarker::PROTOCOL_NAME,
                flags | fio::Flags::PERM_GET_ATTRIBUTES | fio::Flags::FLAG_SEND_REPRESENTATION,
                None,
            )
            .await
            .unwrap();
        assert_matches!(representation, fio::Representation::Node(_));
        let (_, attrs) = proxy
            .get_attributes(fio::NodeAttributesQuery::PROTOCOLS)
            .await
            .expect("transport error")
            .expect("get attributes");
        assert_eq!(attrs.protocols.unwrap(), fio::NodeProtocolKinds::CONNECTOR);
    }
}

/// Opening a service node with the wrong protocol should yield the expected error.
#[fuchsia::test]
async fn open_service_with_wrong_protocol() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_services {
        return;
    }
    let svc_dir = harness.open_service_directory().await;

    for (flags, expected_error) in [
        (fio::Flags::PROTOCOL_DIRECTORY, Status::NOT_DIR),
        // When both PROTOCOL_DIRECTORY and PROTOCOL_FILE are specified, directory takes precedence.
        (fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PROTOCOL_FILE, Status::NOT_DIR),
        (fio::Flags::PROTOCOL_FILE, Status::NOT_FILE),
        (fio::Flags::PROTOCOL_SYMLINK, Status::WRONG_TYPE),
    ] {
        let status = svc_dir
            .open_node::<fio::NodeMarker>(
                EchoMarker::PROTOCOL_NAME,
                flags | fio::Flags::PERM_GET_ATTRIBUTES,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(status, expected_error);
    }
}

#[fuchsia::test]
async fn open_service_with_other_flags_should_fail() {
    let harness = TestHarness::new().await;
    if !harness.config.supports_services {
        return;
    }
    let svc_dir = harness.open_service_directory().await;

    // No need to test for all flag possibilities; just test some flags.
    for (flags, expected_error) in [
        (fio::Flags::PERM_READ_BYTES, Status::INVALID_ARGS),
        (fio::Flags::PERM_TRAVERSE, Status::INVALID_ARGS),
        (fio::Flags::FLAG_SEND_REPRESENTATION, Status::INVALID_ARGS),
        (fio::Flags::PERM_GET_ATTRIBUTES, Status::INVALID_ARGS),
    ] {
        let (proxy, server) = create_proxy::<EchoMarker>();
        svc_dir
            .open(
                EchoMarker::PROTOCOL_NAME,
                fio::Flags::PROTOCOL_SERVICE | flags,
                &fio::Options::default(),
                server.into_channel(),
            )
            .expect("Failed wire call open3");

        let echo_response_status = proxy
            .echo_string(Some(TEST_STRING))
            .map_err(|e| {
                if let fidl::Error::ClientChannelClosed { epitaph, .. } = e {
                    match epitaph.into() {
                        Err(s) => s,
                        Ok(()) => zx::Status::PEER_CLOSED,
                    }
                } else {
                    panic!("Unhandled FIDL error: {:?}", e);
                }
            })
            .await
            .unwrap_err();
        assert_eq!(echo_response_status, expected_error);
    }
}
