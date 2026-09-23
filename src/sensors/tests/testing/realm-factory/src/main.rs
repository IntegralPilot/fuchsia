// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl_fuchsia_sensors_realm::{RealmFactoryRequest, RealmFactoryRequestStream};
use fuchsia_component::runtime::Dictionary;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::{StreamExt, TryStreamExt};

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: RealmFactoryRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_realm_factory).await;
    Ok(())
}

async fn serve_realm_factory(mut stream: RealmFactoryRequestStream) {
    let mut realms = vec![];

    let result: Result<(), Error> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                RealmFactoryRequest::CreateRealm { dictionary, responder } => {
                    let realm = create_realm().await?;
                    let output_dictionary_handle =
                        realm.root.controller().get_output_dictionary().await?.unwrap();
                    let output_dictionary = Dictionary::from(output_dictionary_handle);
                    output_dictionary.associate_with_handle(dictionary).await;
                    realms.push(realm);
                    let _ = responder.send(Ok(()));
                }
                RealmFactoryRequest::_UnknownMethod { .. } => unimplemented!(),
            }
        }

        Ok(())
    }
    .await;

    if let Err(err) = result {
        // Sensors tests allow error logs so we panic to ensure test failure.
        panic!("{:?}", err);
    }
}

async fn create_realm() -> Result<RealmInstance, Error> {
    let builder = RealmBuilder::new().await?;

    let sensors_ref = builder.add_child("sensors", "#meta/sensors.cm", ChildOptions::new()).await?;

    let playback_ref = builder
        .add_child(
            "sensors_playback",
            "sensors_playback#meta/sensors_playback.cm",
            ChildOptions::new(),
        )
        .await?;

    // Route the Driver and Playback protocols to the SensorManager.
    builder
        .add_route(
            Route::new()
                .capability(Capability::service_by_name("fuchsia.hardware.sensors.Service"))
                .capability(Capability::protocol_by_name("fuchsia.hardware.sensors.Playback"))
                .from(&playback_ref)
                .to(&sensors_ref),
        )
        .await?;

    // Add a second playback driver that will be used to validate that the sensor manager can get
    // data from multiple driver providers.
    let second_playback_ref = builder
        .add_child(
            "sensors_playback2",
            "sensors_playback#meta/sensors_playback.cm",
            ChildOptions::new(),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::service_by_name("fuchsia.hardware.sensors.Service"))
                .from(&second_playback_ref)
                .to(&sensors_ref),
        )
        .await?;

    // Route playback to the tests to configure the second playback driver.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.hardware.sensors.Playback"))
                .from(&second_playback_ref)
                .to(Ref::parent()),
        )
        .await?;

    // Route the SensorManager to the test suite.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.sensors.Manager"))
                .from(&sensors_ref)
                .to(Ref::parent()),
        )
        .await?;

    let realm = builder.build().await?;
    Ok(realm)
}
