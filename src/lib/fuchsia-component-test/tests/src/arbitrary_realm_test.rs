// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fidl_examples_routing_echo::{self as fecho};
use fidl_fuchsia_component as fcomponent;
use fuchsia_component::client as fclient;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route,
};

const V2_ECHO_SERVER_ABSOLUTE_URL: &'static str =
    "fuchsia-pkg://fuchsia.com/fuchsia-component-test-tests#meta/echo_server.cm";

const DEFAULT_ECHO_STR: &'static str = "Hello Fuchsia!";

const CUSTOM_COLLECTION: &'static str = "custom_coll";

#[fuchsia::test]
async fn launch_component_in_default_col() -> Result<(), Error> {
    let realm_proxy = fclient::connect_to_protocol::<fcomponent::RealmMarker>()?;
    let params = RealmBuilderParams::new().with_realm_proxy(realm_proxy);
    let builder = RealmBuilder::with_params(params).await?;
    launch_and_test_echo_server(builder).await
}

#[fuchsia::test]
async fn launch_component_in_custom_col() -> Result<(), Error> {
    let realm_proxy = fclient::connect_to_protocol::<fcomponent::RealmMarker>()?;
    let params =
        RealmBuilderParams::new().with_realm_proxy(realm_proxy).in_collection(CUSTOM_COLLECTION);

    let builder = RealmBuilder::with_params(params).await?;
    launch_and_test_echo_server(builder).await
}

async fn launch_and_test_echo_server(builder: RealmBuilder) -> Result<(), Error> {
    let child =
        builder.add_child("child", V2_ECHO_SERVER_ABSOLUTE_URL, ChildOptions::new()).await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(&child)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&child),
        )
        .await?;
    let instance = builder.build().await?;
    let echo_proxy: fecho::EchoProxy = instance.root.connect_to_protocol_at_exposed_dir()?;
    assert_eq!(
        Some(DEFAULT_ECHO_STR.to_string()),
        echo_proxy.echo_string(Some(DEFAULT_ECHO_STR)).await?,
    );
    Ok(())
}
