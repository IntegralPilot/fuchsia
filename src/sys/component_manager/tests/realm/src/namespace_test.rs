// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_examples_routing_echo::EchoMarker;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_component::client::*;

#[fuchsia::test]
pub async fn connect_to_incoming_capabilities_for_component_without_program() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    // We can't open the component namespace until the component is resolved.
    let (_, server_end) = fidl::endpoints::create_proxy();
    let err = query
        .open_directory("no_program", fsys::OpenDirType::NamespaceDir, server_end)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err, fsys::OpenError::InstanceNotResolved);

    // Trigger resolution.
    let lifecycle =
        connect_to_protocol::<fsys::LifecycleControllerMarker>().expect("connected to lifecycle");
    let _ = lifecycle
        .resolve_instance("./no_program")
        .await
        .expect("fidl ok")
        .expect("resolved instance");

    // Connect to the echo protocol in the component's namespace directory.
    let (namespace_dir, server_end) = fidl::endpoints::create_proxy();
    query
        .open_directory("no_program", fsys::OpenDirType::NamespaceDir, server_end)
        .await
        .unwrap()
        .unwrap();
    let echo = connect_to_protocol_at_dir_svc::<EchoMarker>(&namespace_dir).unwrap();
    let reply = echo.echo_string(Some("test")).await.expect("called echo string");
    assert_eq!(reply.unwrap(), "test");
}
