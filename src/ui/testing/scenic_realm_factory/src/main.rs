// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use cm_rust::{self, FidlIntoNative};
use fidl_fuchsia_scheduler::RoleManagerMarker;
use fidl_fuchsia_ui_composition::{
    AllocatorMarker, FlatlandDisplayMarker, FlatlandFactoryMarker, FlatlandMarker,
    ScreenCaptureMarker, ScreenshotMarker, TrustedFlatlandFactoryMarker,
};
use fidl_fuchsia_ui_composition_internal::{
    DisplayOwnershipMarker, ScreenCaptureMarker as ScreenCaptureMarker2,
};
use fidl_fuchsia_ui_display_singleton::{InfoMarker as DisplayInfoMarker, VsyncSourceMarker};
use fidl_fuchsia_ui_focus::FocusChainListenerRegistryMarker;
use fidl_fuchsia_ui_observation_scope::RegistryMarker as ScopedObservationRegistryMarker;
use fidl_fuchsia_ui_observation_test::RegistryMarker as ObservationRegistryMarker;
use fidl_fuchsia_ui_pointer_augment::LocalHitMarker;
use fidl_fuchsia_ui_pointerinjector::RegistryMarker as PointerInjectorRegistryMarker;
use fidl_fuchsia_ui_test_context as ui_test_context;
use fidl_fuchsia_ui_views::ViewRefInstalledMarker;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use futures::{StreamExt, TryStreamExt};
use zx_status;

/// All FIDL services that are exposed by this component's ServiceFs.
enum Service {
    ScenicRealmFactoryServer(ui_test_context::ScenicRealmFactoryRequestStream),
}

#[fuchsia::main(logging_tags = ["scenic_realm_factory"])]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(Service::ScenicRealmFactoryServer);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(None, |conn| async move {
        match conn {
            Service::ScenicRealmFactoryServer(stream) => run_realm_factory_server(stream).await,
        }
    })
    .await;
    Ok(())
}

async fn run_realm_factory_server(stream: ui_test_context::ScenicRealmFactoryRequestStream) {
    stream
        .try_for_each_concurrent(None, |request| async {
            log::debug!("received a request: {:?}", &request);
            let scope = fasync::Scope::new();
            match request {
                ui_test_context::ScenicRealmFactoryRequest::CreateRealm { payload, responder } => {
                    let realm_server = payload.realm_server.expect("missing realm_server");

                    // Create the test realm.
                    let realm = assemble_realm(
                        payload.renderer,
                        payload.display_rotation,
                        payload.display_composition,
                        payload.display_dimensions,
                        payload.display_refresh_rate_millihertz,
                        payload.display_max_layer_count,
                    )
                    .await;

                    let request_stream = realm_server.into_stream();
                    scope.spawn(async move {
                        realm_proxy::service::serve(realm, request_stream)
                            .await
                            .expect("invalid realm proxy server");
                    });
                    responder.send(Ok(())).expect("failed to response");
                }
                ui_test_context::ScenicRealmFactoryRequest::_UnknownMethod {
                    control_handle,
                    ..
                } => {
                    log::warn!("realm factory receive an unknown request");
                    control_handle.shutdown_with_epitaph(zx_status::Status::NOT_SUPPORTED);
                    unimplemented!();
                }
            }

            scope.join().await;
            Ok(())
        })
        .await
        .expect("failed to serve test realm factory request stream");
}

const SCENIC: &str = "scenic";
const SCENIC_REALM_URL: &str = "#meta/scenic_no_config.cm";
const CONFIG: &str = "config";
const CONFIG_URL: &str = "#meta/config.cm";

// Must match the child name specified in `scenic.shard.cml`.
const FAKE_DISPLAY_STACK_HOST: &str = "fake-display-stack-host";

async fn assemble_realm(
    renderer: Option<ui_test_context::RendererType>,
    display_rotation: Option<u64>,
    display_composition: Option<bool>,
    display_dimensions: Option<fidl_fuchsia_math::SizeU>,
    display_refresh_rate_millihertz: Option<u32>,
    display_max_layer_count: Option<u32>,
) -> RealmInstance {
    let builder =
        RealmBuilder::with_params(RealmBuilderParams::new().from_relative_url(SCENIC_REALM_URL))
            .await
            .expect("Failed to create RealmBuilder.");

    // Only fuchsia.scheduler.RoleManager is not already included by scenic_no_config.cml.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<RoleManagerMarker>())
                .from(Ref::parent())
                .to(Ref::child(SCENIC)),
        )
        .await
        .expect("Failed to route fuchsia.scheduler.RoleManager capabilities.");

    let config = builder
        .add_child(CONFIG, CONFIG_URL, ChildOptions::new())
        .await
        .expect("Failed to add config component.");

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.scenic.DisplayCompositionEnableHeuristics",
                ))
                .capability(Capability::configuration("fuchsia.scenic.FrameCounterOverlay"))
                .capability(Capability::configuration("fuchsia.scenic.FramePredictionMarginInUs"))
                .capability(Capability::configuration(
                    "fuchsia.scenic.FrameSchedulerMinPredictedFrameDurationInUs",
                ))
                .capability(Capability::configuration("fuchsia.scenic.ICanHazDisplayId"))
                .capability(Capability::configuration("fuchsia.scenic.ICanHazDisplayMode"))
                .capability(Capability::configuration(
                    "fuchsia.scenic.MaxDisplayHorizontalResolutionPx",
                ))
                .capability(Capability::configuration(
                    "fuchsia.scenic.MaxDisplayRefreshRateMillihertz",
                ))
                .capability(Capability::configuration(
                    "fuchsia.scenic.MaxDisplayVerticalResolutionPx",
                ))
                .capability(Capability::configuration(
                    "fuchsia.scenic.MinDisplayHorizontalResolutionPx",
                ))
                .capability(Capability::configuration(
                    "fuchsia.scenic.MinDisplayRefreshRateMillihertz",
                ))
                .capability(Capability::configuration(
                    "fuchsia.scenic.MinDisplayVerticalResolutionPx",
                ))
                .capability(Capability::configuration("fuchsia.scenic.PointerAutoFocus"))
                .capability(Capability::configuration("fuchsia.scenic.UseSeparateInputThread"))
                .capability(Capability::configuration("fuchsia.ui.Prefetch"))
                .capability(Capability::configuration("fuchsia.ui.VisualDebuggingLevel"))
                .from(&config)
                .to(Ref::child(SCENIC)),
        )
        .await
        .expect("Failed to route config capabilities.");

    let renderer_type = match renderer {
        Some(ui_test_context::RendererType::Null) => "null".to_string(),
        Some(ui_test_context::RendererType::Vulkan) => "vulkan".to_string(),
        _ => "vulkan".to_string(),
    };

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.scenic.Renderer".to_string().fidl_into_native(),
            value: renderer_type.into(),
        }))
        .await
        .expect("Failed to set fuchsia.scenic.Renderer.");

    let display_rotation_value: u64 = match display_rotation {
        Some(value) => value,
        None => 0,
    };

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.scenic.DisplayRotation".to_string().fidl_into_native(),
            value: display_rotation_value.into(),
        }))
        .await
        .expect("Failed to set fuchsia.scenic.DisplayRotation.");

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.scenic.Renderer"))
                .capability(Capability::configuration("fuchsia.scenic.DisplayRotation"))
                .from(Ref::self_())
                .to(Ref::child(SCENIC)),
        )
        .await
        .expect("Failed to route config capabilities.");

    let display_composition: bool = match display_composition {
        Some(value) => value,
        None => true,
    };

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.scenic.DisplayComposition".to_string().fidl_into_native(),
            value: display_composition.into(),
        }))
        .await
        .expect("Failed to set fuchsia.scenic.DisplayComposition.");

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.scenic.DisplayComposition"))
                .from(Ref::self_())
                .to(Ref::child(SCENIC)),
        )
        .await
        .expect("Failed to route config capabilities.");

    // Expose UI capabilities.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<FlatlandMarker>())
                .capability(Capability::protocol::<FlatlandFactoryMarker>())
                .capability(Capability::protocol::<FlatlandDisplayMarker>())
                .capability(Capability::protocol::<AllocatorMarker>())
                .capability(Capability::protocol::<ScreenCaptureMarker>())
                .capability(Capability::protocol::<ScreenCaptureMarker2>())
                .capability(Capability::protocol::<DisplayOwnershipMarker>())
                .capability(Capability::protocol::<DisplayInfoMarker>())
                .capability(Capability::protocol::<VsyncSourceMarker>())
                .capability(Capability::protocol::<ObservationRegistryMarker>())
                .capability(Capability::protocol::<FocusChainListenerRegistryMarker>())
                .capability(Capability::protocol::<ScopedObservationRegistryMarker>())
                .capability(Capability::protocol::<PointerInjectorRegistryMarker>())
                .capability(Capability::protocol::<LocalHitMarker>())
                .capability(Capability::protocol::<ViewRefInstalledMarker>())
                .capability(Capability::protocol::<ScreenshotMarker>())
                .capability(Capability::protocol::<TrustedFlatlandFactoryMarker>())
                .from(Ref::child(SCENIC))
                .to(Ref::parent()),
        )
        .await
        .expect("Failed to route capabilities.");

    // Set up fake-display-stack-host's config.
    builder
        .init_mutable_config_from_package(FAKE_DISPLAY_STACK_HOST)
        .await
        .expect("failed to init mutable config for fake display stack host");
    if let Some(dimensions) = display_dimensions {
        builder
            .set_config_value(FAKE_DISPLAY_STACK_HOST, "active_width_px", dimensions.width.into())
            .await
            .expect("failed to set width");
        builder
            .set_config_value(FAKE_DISPLAY_STACK_HOST, "active_height_px", dimensions.height.into())
            .await
            .expect("failed to set height");
    }
    if let Some(refresh_rate_millihertz) = display_refresh_rate_millihertz {
        builder
            .set_config_value(
                FAKE_DISPLAY_STACK_HOST,
                "refresh_rate_millihertz",
                refresh_rate_millihertz.into(),
            )
            .await
            .expect("failed to set refresh rate");
    }
    if let Some(max_layer_count) = display_max_layer_count {
        builder
            .set_config_value(FAKE_DISPLAY_STACK_HOST, "max_layer_count", max_layer_count.into())
            .await
            .expect("failed to set max layer count");
    }

    // Create the test realm.
    builder.build().await.expect("Failed to create test realm.")
}
