// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests that connect to the service node itself.

use super::endpoint;

// Macros are exported into the root of the crate.
use crate::{assert_close, assert_get_attributes};

use crate::directory::serve;
use crate::pseudo_directory;

use assert_matches::assert_matches;
use flex_fuchsia_io as fio;
use futures::StreamExt as _;
use zx_status::Status;

fn connect_at(
    root: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
    #[cfg(feature = "fdomain")] client: &std::sync::Arc<flex_client::Client>,
) -> fio::NodeProxy {
    #[cfg(feature = "fdomain")]
    let (proxy, server_end) = client.create_proxy::<fio::NodeMarker>();
    #[cfg(not(feature = "fdomain"))]
    let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    root.open(path, flags, &fio::Options::default(), server_end.into_channel()).unwrap();
    proxy
}

#[fuchsia::test]
async fn construction() {
    let dir = pseudo_directory! {
        "server" => endpoint(|_scope, _channel| ()),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let proxy = connect_at(
        &root,
        "server",
        fio::Flags::PROTOCOL_NODE,
        #[cfg(feature = "fdomain")]
        &scope.domain(),
    );
    assert_close!(proxy);
}

#[fuchsia::test]
async fn get_attributes() {
    let dir = pseudo_directory! {
        "server" => endpoint(|_scope, _channel| ()),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let proxy = connect_at(
        &root,
        "server",
        fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
        #[cfg(feature = "fdomain")]
        &scope.domain(),
    );
    assert_get_attributes!(
        proxy,
        fio::NodeAttributesQuery::all(),
        immutable_attributes!(
            fio::NodeAttributesQuery::all(),
            Immutable {
                protocols: fio::NodeProtocolKinds::CONNECTOR,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::CONNECT,
            }
        )
    );
    assert_close!(proxy);
}

#[fuchsia::test]
async fn representation() {
    let dir = pseudo_directory! {
        "server" => endpoint(|_scope, _channel| ()),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let proxy = connect_at(
        &root,
        "server",
        fio::Flags::PROTOCOL_NODE | fio::Flags::FLAG_SEND_REPRESENTATION,
        #[cfg(feature = "fdomain")]
        &scope.domain(),
    );
    assert_matches!(
        proxy.take_event_stream().next().await,
        Some(Ok(fio::NodeEvent::OnRepresentation { .. }))
    );
}

#[fuchsia::test]
async fn clone() {
    let dir = pseudo_directory! {
        "server" => endpoint(|_scope, _channel| ()),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let proxy = connect_at(
        &root,
        "server",
        fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
        #[cfg(feature = "fdomain")]
        &scope.domain(),
    );
    assert_get_attributes!(
        proxy,
        fio::NodeAttributesQuery::all(),
        immutable_attributes!(
            fio::NodeAttributesQuery::all(),
            Immutable {
                protocols: fio::NodeProtocolKinds::CONNECTOR,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::CONNECT,
            }
        )
    );

    #[cfg(feature = "fdomain")]
    let (cloned_proxy, server_end) = {
        let client = scope.domain();
        client.create_proxy::<fio::NodeMarker>()
    };
    #[cfg(not(feature = "fdomain"))]
    let (cloned_proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
    proxy.clone(server_end.into_channel().into()).unwrap();
    assert_get_attributes!(
        cloned_proxy,
        fio::NodeAttributesQuery::all(),
        immutable_attributes!(
            fio::NodeAttributesQuery::all(),
            Immutable {
                protocols: fio::NodeProtocolKinds::CONNECTOR,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::CONNECT,
            }
        )
    );
    assert_get_attributes!(
        proxy,
        fio::NodeAttributesQuery::all(),
        immutable_attributes!(
            fio::NodeAttributesQuery::all(),
            Immutable {
                protocols: fio::NodeProtocolKinds::CONNECTOR,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::CONNECT,
            }
        )
    );
    assert_close!(cloned_proxy);
    assert_close!(proxy);
}

#[fuchsia::test]
async fn update_attributes_not_supported() {
    let dir = pseudo_directory! {
        "server" => endpoint(|_scope, _channel| ()),
    };
    #[cfg(feature = "fdomain")]
    let scope = crate::execution_scope::ExecutionScope::new(flex_local::local_client_empty());
    #[cfg(not(feature = "fdomain"))]
    let scope = crate::execution_scope::ExecutionScope::new();
    let root = serve(dir, scope.clone(), fio::PERM_READABLE);
    let proxy = connect_at(
        &root,
        "server",
        fio::Flags::PROTOCOL_NODE,
        #[cfg(feature = "fdomain")]
        &scope.domain(),
    );
    let response = proxy.update_attributes(&fio::MutableNodeAttributes::default()).await.unwrap();
    assert_eq!(response, Err(Status::BAD_HANDLE.into_raw()));
}
