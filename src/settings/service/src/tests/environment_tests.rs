// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::EnvironmentBuilder;
use crate::ingress::fidl;
use crate::migration::MIGRATION_FILE_NAME;
use ::fidl::Error::ClientChannelClosed;
use ::fidl::endpoints::create_proxy_and_stream;
use assert_matches::assert_matches;
use fidl_fuchsia_settings::{
    AccessibilityMarker, AudioMarker, DisplayMarker, DoNotDisturbMarker, FactoryResetMarker,
    InputMarker, IntlMarker, KeyboardMarker, NightModeMarker, PrivacyMarker, SetupMarker,
};
use fidl_fuchsia_stash::StoreMarker;
use fuchsia_async as fasync;
use fuchsia_inspect::component;
use futures::StreamExt;
use settings_common::inspect::config_logger::InspectConfigLogger;
use settings_light::build_light_default_settings;
use settings_test_common::storage::InMemoryStorageFactory;
use std::rc::Rc;
use zx::Status;

const ENV_NAME: &str = "settings_service_environment_test";

#[fuchsia::test]
async fn migration_error_does_not_cause_early_exit() {
    const UNKNOWN_ID: u64 = u64::MAX;
    let fs = tempfile::tempdir().expect("failed to create tempdir");
    std::fs::write(fs.path().join(MIGRATION_FILE_NAME), UNKNOWN_ID.to_string())
        .expect("failed to write migration file");
    let directory = fuchsia_fs::directory::open_in_namespace(
        fs.path().to_str().expect("tempdir path is not valid UTF-8"),
        fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
    )
    .expect("failed to open connection to tempdir");
    let (store_proxy, mut request_stream) = create_proxy_and_stream::<StoreMarker>();
    fasync::Task::local(async move {
        while let Some(request) = request_stream.next().await {
            match request.unwrap() {
                fidl_fuchsia_stash::StoreRequest::Identify { .. } => {}
                fidl_fuchsia_stash::StoreRequest::CreateAccessor { accessor_request, .. } => {
                    let mut stream = accessor_request.into_stream();
                    fasync::Task::local(async move {
                        if let Some(r) = stream.next().await {
                            panic!("unexpected call to store before migration id checked: {r:?}");
                        }
                    })
                    .detach();
                }
            }
        }
    })
    .detach();

    let config_logger =
        Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(component::inspector().root())));

    let _ = EnvironmentBuilder::new(Rc::new(InMemoryStorageFactory::new()))
        .fidl_interfaces(&[fidl::Interface::Light])
        .store_proxy(store_proxy)
        .storage_dir(directory)
        .light_configuration(build_light_default_settings(config_logger))
        .spawn_nested(ENV_NAME)
        .await
        .expect("environment should be built");
}

#[fuchsia::test(allow_stalls = false)]
async fn test_channel_failure_watch() {
    let env = EnvironmentBuilder::new(Rc::new(InMemoryStorageFactory::new()))
        .spawn_and_get_protocol_connector(ENV_NAME)
        .await
        .unwrap();

    let audio_proxy = env.connect_to_protocol::<AudioMarker>().expect("should get");
    let result = audio_proxy.watch2().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let a11y_proxy = env.connect_to_protocol::<AccessibilityMarker>().expect("should get");
    let result = a11y_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let display_proxy = env.connect_to_protocol::<DisplayMarker>().expect("should get");
    let result = display_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let dnd_proxy = env.connect_to_protocol::<DoNotDisturbMarker>().expect("should get");
    let result = dnd_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let factory_reset_proxy = env.connect_to_protocol::<FactoryResetMarker>().expect("should get");
    let result = factory_reset_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let input_proxy = env.connect_to_protocol::<InputMarker>().expect("should get");
    let result = input_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let intl_proxy = env.connect_to_protocol::<IntlMarker>().expect("should get");
    let result = intl_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let keyboard_proxy = env.connect_to_protocol::<KeyboardMarker>().expect("should get");
    let result = keyboard_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let night_mode_proxy = env.connect_to_protocol::<NightModeMarker>().expect("should get");
    let result = night_mode_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let privacy_proxy = env.connect_to_protocol::<PrivacyMarker>().expect("should get");
    let result = privacy_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );

    let setup_proxy = env.connect_to_protocol::<SetupMarker>().expect("should get");
    let result = setup_proxy.watch().await;
    assert_matches!(
        result,
        Err(ClientChannelClosed { epitaph, .. }) if epitaph == Status::NOT_FOUND
    );
}
