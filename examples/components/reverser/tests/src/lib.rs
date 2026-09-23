// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_examples_reverser as freverser;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};

async fn run_reverser(
    switch_case: bool,
) -> Result<(fuchsia_component_test::RealmInstance, freverser::ReverserProxy), Error> {
    let builder = RealmBuilder::new().await?;
    let reverser = builder.add_child("reverser", "#meta/reverser.cm", ChildOptions::new()).await?;

    builder.init_mutable_config_from_package(&reverser).await?;
    builder.set_config_value(&reverser, "switch_case", switch_case.into()).await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.examples.reverser.Reverser"))
                .from(&reverser)
                .to(Ref::parent()),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&reverser),
        )
        .await?;

    let instance = builder.build().await?;
    let proxy = instance.root.connect_to_protocol_at_exposed_dir::<freverser::ReverserProxy>()?;

    Ok((instance, proxy))
}

#[fuchsia::test]
async fn reverse_string_test() -> Result<(), Error> {
    let (_instance, client) = run_reverser(false).await?;
    let input = "Hello Fuchsia!";
    let output = client.reverse(input).await.expect("reverse call failed");
    assert_eq!(output, "!aishcuF olleH");
    Ok(())
}

#[fuchsia::test]
async fn reverse_string_switch_case_test() -> Result<(), Error> {
    let (_instance, client) = run_reverser(true).await?;

    // Without switch case: Hello Fuchsia! -> !aishcuF olleH
    // With switch case, the casing of every alphabetical character
    // is inverted in addition to the string being reversed.
    let output = client.reverse("Hello Fuchsia!").await.expect("reverse call failed");
    assert_eq!(output, "!AISHCUf OLLEh");

    let a_c = client.reverse("aC").await.expect("reverse call failed");
    // When reversed, "aC" becomes "Ca". Inverting the case yields "cA".
    assert_eq!(a_c, "cA");

    Ok(())
}
