// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod mocks;
mod packaged_component;
mod traits;

use crate::mocks::factory_reset_mock::FactoryResetMock;
use crate::mocks::input_report_mock::InputReportMock;
use crate::mocks::pointer_injector_mock::PointerInjectorMock;
use crate::mocks::sound_player_mock::{
    SoundPlayerBehavior, SoundPlayerMock, SoundPlayerRequestName,
};
use crate::packaged_component::PackagedComponent;
use crate::traits::realm_builder_ext::RealmBuilderExt as _;
use crate::traits::test_realm_component::TestRealmComponent;
use fidl_fuchsia_ui_pointerinjector as pointerinjector;
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_component_test::{
    Capability, DirectoryContents, RealmBuilder, RealmBuilderParams, RealmInstance,
};
use futures::StreamExt;
use input_synthesis::{modern_backend, synthesizer};

/// Creates a test realm with
/// a) routes from the given mocks to the input pipeline, and
/// b) all other capabilities routed from hermetically instantiated packages where possible, and
/// c) non-hermetic capabilities routed in from above the test realm.
async fn assemble_realm(
    sound_player_mock: SoundPlayerMock,
    pointer_injector_mock: PointerInjectorMock,
    factory_reset_mock: FactoryResetMock,
    input_report_mock: Option<InputReportMock>,
    test_name: &str,
) -> RealmInstance {
    let b = RealmBuilder::with_params(RealmBuilderParams::new().realm_name(test_name))
        .await
        .expect("Failed to create RealmBuilder");

    // Declare packaged components.
    let scenic_test_realm =
        PackagedComponent::new_from_modern_url("scenic-test-realm", "#meta/scenic_with_config.cm");
    let a11y_test_realm =
        PackagedComponent::new_from_modern_url("a11y-test-realm", "#meta/fake-a11y-manager.cm");
    // We launch scene_manager eagerly because the tests for physical factory reset
    // spoof a real device via directory watching. Since we no longer connect to
    // scene_manager's InputDeviceRegistry to inject events (which are now ignored
    // by FactoryResetHandler), scene_manager would remain dormant unless run eagerly.
    let scene_manager =
        PackagedComponent::new_eager_from_modern_url("input-owner", SCENE_MANAGER_URL);
    let scene_manager_config =
        PackagedComponent::new_from_modern_url("scene-manager-config", SCENE_MANAGER_CONFIG_URL);

    // Add packaged components and mocks to the test realm.
    b.add(&scenic_test_realm).await;
    b.add(&a11y_test_realm).await;
    b.add(&scene_manager).await;
    b.add(&scene_manager_config).await;
    b.add(&sound_player_mock).await;
    b.add(&pointer_injector_mock).await;
    b.add(&factory_reset_mock).await;
    if let Some(mock) = &input_report_mock {
        b.add(mock).await;
    }

    // Allow Scenic to access the capabilities it needs. Capabilities that can't
    // be run hermetically are routed from the parent realm. The remainder are
    // routed from peers.
    b.route_from_parent::<fidl_fuchsia_tracing_provider::RegistryMarker>(&scenic_test_realm).await;
    b.route_from_parent::<fidl_fuchsia_sysmem::AllocatorMarker>(&scenic_test_realm).await;
    b.route_from_parent::<fidl_fuchsia_sysmem2::AllocatorMarker>(&scenic_test_realm).await;
    b.route_from_parent::<fidl_fuchsia_vulkan_loader::LoaderMarker>(&scenic_test_realm).await;
    b.route_from_parent::<fidl_fuchsia_scheduler::RoleManagerMarker>(&scenic_test_realm).await;

    // Allow the a11y manager to access the capabilities it needs.
    b.route_to_peer::<fidl_fuchsia_ui_observation_scope::RegistryMarker>(
        &scenic_test_realm,
        &a11y_test_realm,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_ui_composition::FlatlandFactoryMarker>(
        &scenic_test_realm,
        &a11y_test_realm,
    )
    .await;

    // Allow scene manager to access the capabilities it needs to provide
    // input. All of these capabilities are run hermetically, so they are all
    // routed from peers.
    b.route_to_peer::<fidl_fuchsia_ui_composition::FlatlandMarker>(
        &scenic_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_ui_composition::FlatlandDisplayMarker>(
        &scenic_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_ui_display_singleton::InfoMarker>(
        &scenic_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_ui_pointerinjector::RegistryMarker>(
        &scenic_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_ui_composition_internal::DisplayOwnershipMarker>(
        &scenic_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_ui_focus::FocusChainListenerRegistryMarker>(
        &scenic_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_accessibility_scene::ProviderMarker>(
        &a11y_test_realm,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_media_sounds::PlayerMarker>(&sound_player_mock, &scene_manager)
        .await;
    b.route_to_peer::<fidl_fuchsia_ui_pointerinjector_configuration::SetupMarker>(
        &pointer_injector_mock,
        &scene_manager,
    )
    .await;
    b.route_to_peer::<fidl_fuchsia_recovery::FactoryResetMarker>(
        &factory_reset_mock,
        &scene_manager,
    )
    .await;
    b.add_route(
        fuchsia_component_test::Route::new()
            .capability(Capability::configuration("fuchsia.scenic.DisplayRotation"))
            .capability(Capability::configuration("fuchsia.ui.AttachA11yView"))
            .capability(Capability::configuration("fuchsia.ui.DisplayPixelDensity"))
            .capability(Capability::configuration("fuchsia.ui.EnableButtonBatonPassing"))
            .capability(Capability::configuration("fuchsia.ui.EnableMouseBatonPassing"))
            .capability(Capability::configuration("fuchsia.ui.EnableTouchBatonPassing"))
            .capability(Capability::configuration("fuchsia.ui.EnableMergeTouchEvents"))
            .capability(Capability::configuration("fuchsia.ui.IdleThresholdMs"))
            .capability(Capability::configuration("fuchsia.ui.SupportedInputDevices"))
            .capability(Capability::configuration("fuchsia.ui.ViewingDistance"))
            .capability(Capability::configuration("fuchsia.power.SuspendEnabled"))
            .capability(Capability::configuration("fuchsia.ui.Prefetch"))
            .capability(Capability::configuration("fuchsia.ui.VisualDebuggingLevel"))
            .from(scene_manager_config.ref_())
            .to(scene_manager.ref_()),
    )
    .await
    .unwrap();

    // Route injection to input pipeline out to the test.
    b.route_to_parent::<fidl_fuchsia_input_injection::InputDeviceRegistryMarker>(&scene_manager)
        .await;

    if let Some(mock) = &input_report_mock {
        b.add_route(
            fuchsia_component_test::Route::new()
                .capability(fuchsia_component_test::Capability::service_by_name(
                    "fuchsia.input.report.Service",
                ))
                .from(mock.ref_())
                .to(scene_manager.ref_()),
        )
        .await
        .unwrap();
    } else {
        // scene_manager requires a route for fuchsia.input.report.Service to compile and
        // validate. If we aren't testing physical reset (no mock device), we route from void()
        // so that the service directory is empty.
        b.add_route(
            fuchsia_component_test::Route::new()
                .capability(
                    fuchsia_component_test::Capability::service_by_name(
                        "fuchsia.input.report.Service",
                    )
                    .optional(),
                )
                .from(fuchsia_component_test::Ref::void())
                .to(scene_manager.ref_()),
        )
        .await
        .unwrap();
    }

    // Route required config files to input pipeline.
    b.route_read_only_directory(
        String::from("config-data"),
        &scene_manager,
        DirectoryContents::new().add_file("chirp-start-tone.wav", ""),
    )
    .await;

    b.route_read_only_directory(
        String::from("sensor-config"),
        &scene_manager,
        DirectoryContents::new().add_file("empty.json", ""),
    )
    .await;

    // Create the test realm.
    b.build().await.expect("Failed to create realm")
}

fn default_viewport() -> pointerinjector::Viewport {
    pointerinjector::Viewport {
        extents: Some([[0.0, 0.0], [100.0, 100.0]]),
        viewport_to_context_transform: Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        ..Default::default()
    }
}

const SOUND_PLAYER_NAME: &'static str = "mock_sound_player";
const POINTER_INJECTOR_NAME: &'static str = "mock_pointer_injector";
const FACTORY_RESET_NAME: &'static str = "mock_factory_reset";
const INPUT_REPORT_NAME: &'static str = "mock_input_report";
const SCENE_MANAGER_URL: &'static str = "#meta/scene_manager.cm";
const SCENE_MANAGER_CONFIG_URL: &'static str = "#meta/scene_manager_config.cm";

#[fuchsia::test]
async fn injected_factory_reset_is_ignored() {
    let (reset_request_relay_write_end, mut reset_request_relay_read_end) =
        futures::channel::mpsc::unbounded();
    let sound_player_mock =
        SoundPlayerMock::new(SOUND_PLAYER_NAME, SoundPlayerBehavior::Succeed, None);
    let pointer_injector_mock = PointerInjectorMock::new(POINTER_INJECTOR_NAME, default_viewport());
    let factory_reset_mock =
        FactoryResetMock::new(FACTORY_RESET_NAME, reset_request_relay_write_end);
    let realm = assemble_realm(
        sound_player_mock.clone(),
        pointer_injector_mock.clone(),
        factory_reset_mock.clone(),
        None,
        "factory_reset_is_ignored",
    )
    .await;

    // Press buttons for factory reset.
    let injection_registry = realm
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("Failed to connect to InputDeviceRegistry");
    let mut device_registry = modern_backend::InputDeviceRegistry::new(injection_registry);
    synthesizer::media_button_event([synthesizer::MediaButton::FactoryReset], &mut device_registry)
        .await
        .expect("Failed to inject reset event");

    // Wait and verify that `factory_reset_mock` DOES NOT receive the reset request.
    let result = reset_request_relay_read_end
        .next()
        .on_timeout(
            fasync::MonotonicInstant::after(fasync::MonotonicDuration::from_millis(3000)),
            || None,
        )
        .await;

    assert!(result.is_none(), "Injected factory reset should explicitly be ignored");

    realm.destroy().await.unwrap();
}

#[fuchsia::test]
async fn real_factory_reset_is_handled() {
    let (reset_request_relay_write_end, mut reset_request_relay_read_end) =
        futures::channel::mpsc::unbounded();
    let (sound_request_relay_write_end, mut sound_request_relay_read_end) =
        futures::channel::mpsc::unbounded();

    let sound_player_mock = SoundPlayerMock::new(
        SOUND_PLAYER_NAME,
        SoundPlayerBehavior::Succeed,
        Some(sound_request_relay_write_end),
    );
    let pointer_injector_mock = PointerInjectorMock::new(POINTER_INJECTOR_NAME, default_viewport());
    let factory_reset_mock =
        FactoryResetMock::new(FACTORY_RESET_NAME, reset_request_relay_write_end);
    let input_report_mock = InputReportMock::new(INPUT_REPORT_NAME);

    let realm = assemble_realm(
        sound_player_mock.clone(),
        pointer_injector_mock.clone(),
        factory_reset_mock.clone(),
        Some(input_report_mock.clone()),
        "real_factory_reset_is_handled",
    )
    .await;

    assert_eq!(
        sound_request_relay_read_end.next().await,
        Some(SoundPlayerRequestName::AddSoundFromFile)
    );
    assert_eq!(sound_request_relay_read_end.next().await, Some(SoundPlayerRequestName::PlaySound2));

    let result = reset_request_relay_read_end.next().await;
    assert!(result.is_some(), "Hardware factory reset should be explicitly handled");

    realm.destroy().await.unwrap();
}

#[fuchsia::test]
async fn failure_to_load_sound_doesnt_block_factory_reset() {
    let (reset_request_relay_write_end, mut reset_request_relay_read_end) =
        futures::channel::mpsc::unbounded();

    let sound_player_mock =
        SoundPlayerMock::new(SOUND_PLAYER_NAME, SoundPlayerBehavior::FailAddSound, None);
    let pointer_injector_mock = PointerInjectorMock::new(POINTER_INJECTOR_NAME, default_viewport());
    let factory_reset_mock =
        FactoryResetMock::new(FACTORY_RESET_NAME, reset_request_relay_write_end);
    let input_report_mock = InputReportMock::new(INPUT_REPORT_NAME);

    let realm = assemble_realm(
        sound_player_mock.clone(),
        pointer_injector_mock.clone(),
        factory_reset_mock.clone(),
        Some(input_report_mock.clone()),
        "failure_to_load_sound_doesnt_block_factory_reset",
    )
    .await;

    let result = reset_request_relay_read_end.next().await;
    assert!(result.is_some(), "Hardware factory reset should be explicitly handled");

    realm.destroy().await.unwrap();
}

#[fuchsia::test]
async fn failure_to_play_sound_doesnt_block_factory_reset() {
    let (reset_request_relay_write_end, mut reset_request_relay_read_end) =
        futures::channel::mpsc::unbounded();

    let sound_player_mock =
        SoundPlayerMock::new(SOUND_PLAYER_NAME, SoundPlayerBehavior::FailPlaySound, None);
    let pointer_injector_mock = PointerInjectorMock::new(POINTER_INJECTOR_NAME, default_viewport());
    let factory_reset_mock =
        FactoryResetMock::new(FACTORY_RESET_NAME, reset_request_relay_write_end);
    let input_report_mock = InputReportMock::new(INPUT_REPORT_NAME);

    let realm = assemble_realm(
        sound_player_mock.clone(),
        pointer_injector_mock.clone(),
        factory_reset_mock.clone(),
        Some(input_report_mock.clone()),
        "failure_to_play_sound_doesnt_block_factory_reset",
    )
    .await;

    let result = reset_request_relay_read_end.next().await;
    assert!(result.is_some(), "Hardware factory reset should be explicitly handled");

    realm.destroy().await.unwrap();
}
