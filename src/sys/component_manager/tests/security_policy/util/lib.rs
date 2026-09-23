// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use component_events::events::*;
use component_events::matcher::EventMatcher;
use component_events::sequence::{EventSequence, Ordering};
use fidl::endpoints::{create_proxy, ProtocolMarker};
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_io as fio, fidl_fuchsia_sys2 as fsys,
};

pub async fn start_policy_test(
    component_manager_url: &str,
    root_component_url: &str,
) -> Result<(RealmInstance, fcomponent::RealmProxy, EventStream), Error> {
    let builder = RealmBuilder::new().await.unwrap();
    let root_child =
        builder.add_child("root", root_component_url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .from(Ref::parent())
                .to(&root_child),
        )
        .await
        .unwrap();
    let instance = builder.build_in_nested_component_manager(component_manager_url).await.unwrap();
    let proxy: fcomponent::EventStreamProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    proxy.wait_for_ready().await.unwrap();

    let event_stream = EventStream::new(proxy);

    instance.start_component_tree().await.unwrap();

    // Wait for the root component to be started so we can connect to its Realm service.
    let event_stream = EventSequence::new()
        .has_subset(
            vec![EventMatcher::ok().r#type(Started::TYPE).moniker("./root")],
            Ordering::Unordered,
        )
        .expect_and_giveback(event_stream)
        .await
        .unwrap();
    // Get to the Realm protocol
    let realm_query: fsys::RealmQueryProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (exposed_dir, server_end) = create_proxy();
    realm_query
        .open_directory("./root", fsys::OpenDirType::ExposedDir, server_end)
        .await
        .unwrap()
        .unwrap();
    let (realm, server_end) = create_proxy::<fcomponent::RealmMarker>();
    exposed_dir
        .open(
            fcomponent::RealmMarker::DEBUG_NAME,
            fio::Flags::PROTOCOL_SERVICE,
            &Default::default(),
            server_end.into_channel(),
        )
        .unwrap();
    Ok((instance, realm, event_stream))
}

pub async fn open_exposed_dir(
    realm: &fcomponent::RealmProxy,
    name: &str,
) -> Result<fio::DirectoryProxy, fcomponent::Error> {
    let child_ref = fdecl::ChildRef { name: name.to_string(), collection: None };
    let (exposed_dir, server_end) = create_proxy();
    realm
        .open_exposed_dir(&child_ref, server_end)
        .await
        .expect("open_exposed_dir failed")
        .map(|_| exposed_dir)
}
