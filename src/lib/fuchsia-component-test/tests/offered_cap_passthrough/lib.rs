// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use fidl_fidl_examples_routing_echo as fecho;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use futures::channel::mpsc;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _};

#[fuchsia::test]
async fn offered_capability_passed_through_nested_component_manager() {
    let builder = RealmBuilder::new().await.unwrap();
    let (send_echo_client_results, mut receive_echo_client_results) = mpsc::channel(1);
    let echo_client = builder
        .add_local_child(
            "echo-client",
            move |h| echo_client_mock(send_echo_client_results.clone(), h).boxed(),
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fidl.examples.routing.echo.Echo"))
                .from(Ref::parent())
                .to(&echo_client),
        )
        .await
        .unwrap();
    let cm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    assert!(
        receive_echo_client_results.next().await.is_some(),
        "failed to observe the mock client report success",
    );
    cm_instance.destroy().await.unwrap();
}

async fn echo_client_mock(
    mut send_echo_client_results: mpsc::Sender<()>,
    handles: LocalComponentHandles,
) -> Result<(), Error> {
    const DEFAULT_ECHO_STR: &'static str = "Hello Fuchsia!";
    let echo: fecho::EchoProxy = handles.connect_to_protocol()?;
    let out = echo.echo_string(Some(DEFAULT_ECHO_STR)).await?;
    if Some(DEFAULT_ECHO_STR.to_string()) != out {
        return Err(format_err!("unexpected echo result: {:?}", out));
    }
    send_echo_client_results.send(()).await.expect("failed to send results");
    Ok(())
}
