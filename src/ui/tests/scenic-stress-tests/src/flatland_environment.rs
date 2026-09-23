// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Args;
use crate::flatland_actor::FlatlandActor;
use crate::flatland_instance::{DISPLAY_HEIGHT, DISPLAY_WIDTH, FlatlandInstance};
use crate::input_actor::InputActor;
use async_trait::async_trait;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker;
use fidl_fuchsia_ui_composition as flatland;
use fidl_fuchsia_ui_pointerinjector as pointerinjector;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::lock::Mutex;
use rand::SeedableRng as _;
use rand::rngs::SmallRng;
use std::sync::Arc;
use std::time::Duration;
use stress_test::actor::ActorRunner;
use stress_test::environment::Environment;

/// Contains the running instance of scenic and the actors that operate on it.
/// This object lives for the entire duration of the test.
pub struct FlatlandEnvironment {
    args: Args,
    realm_instance: Arc<RealmInstance>,
}

impl FlatlandEnvironment {
    pub async fn new(args: Args) -> Self {
        let builder = RealmBuilder::new().await.unwrap();
        let fake_display_stack_host = builder
            .add_child(
                "fake-display-stack-host",
                "#meta/fake-display-stack-host.cm",
                ChildOptions::new(),
            )
            .await
            .unwrap();
        let scenic =
            builder.add_child("scenic", "#meta/scenic.cm", ChildOptions::new()).await.unwrap();
        let config =
            builder.add_child("config", "#meta/config.cm", ChildOptions::new()).await.unwrap();

        let cobalt = builder
            .add_local_child(
                "cobalt",
                move |handles| {
                    Box::pin(crate::metrics_discarder::serve_metric_event_logger_factory(handles))
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<MetricEventLoggerFactoryMarker>())
                    .from(&cobalt)
                    .to(&scenic),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.scenic.Renderer"))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.FrameSchedulerMinPredictedFrameDurationInUs",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.FramePredictionMarginInUs",
                    ))
                    .capability(Capability::configuration("fuchsia.scenic.PointerAutoFocus"))
                    .capability(Capability::configuration("fuchsia.scenic.DisplayComposition"))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.DisplayCompositionEnableHeuristics",
                    ))
                    .capability(Capability::configuration("fuchsia.scenic.ICanHazDisplayId"))
                    .capability(Capability::configuration("fuchsia.scenic.ICanHazDisplayMode"))
                    .capability(Capability::configuration("fuchsia.scenic.DisplayRotation"))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.MinDisplayHorizontalResolutionPx",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.MaxDisplayHorizontalResolutionPx",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.MinDisplayVerticalResolutionPx",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.MaxDisplayVerticalResolutionPx",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.MinDisplayRefreshRateMillihertz",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.scenic.MaxDisplayRefreshRateMillihertz",
                    ))
                    .capability(Capability::configuration("fuchsia.scenic.FrameCounterOverlay"))
                    .capability(Capability::configuration("fuchsia.scenic.InputNoncritical"))
                    .capability(Capability::configuration("fuchsia.scenic.UseSeparateInputThread"))
                    .capability(Capability::configuration("fuchsia.ui.VisualDebuggingLevel"))
                    .capability(Capability::configuration("fuchsia.ui.Prefetch"))
                    .capability(Capability::configuration("fuchsia.ui.VisualDebuggingLevel"))
                    .from(&config)
                    .to(&scenic),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.sysmem.Allocator"))
                    .capability(Capability::protocol_by_name("fuchsia.sysmem2.Allocator"))
                    .capability(Capability::protocol_by_name("fuchsia.tracing.provider.Registry"))
                    .from(Ref::parent())
                    .to(&scenic)
                    .to(&fake_display_stack_host),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .from(Ref::parent())
                    .to(&scenic)
                    .to(&fake_display_stack_host),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.vulkan.loader.Loader"))
                    .from(Ref::parent())
                    .to(&scenic),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::service_by_name("fuchsia.hardware.display.Service"))
                    .from(&fake_display_stack_host)
                    .to(&scenic),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<flatland::FlatlandFactoryMarker>())
                    .capability(Capability::protocol::<flatland::FlatlandDisplayMarker>())
                    .capability(Capability::protocol::<pointerinjector::RegistryMarker>())
                    .from(&scenic)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        // Set up fake-display-stack-host's config.
        builder.init_mutable_config_from_package(&fake_display_stack_host).await.unwrap();

        let realm_instance = Arc::new(builder.build().await.expect("Failed to create realm"));
        Self { args, realm_instance }
    }
}

impl std::fmt::Debug for FlatlandEnvironment {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("FlatlandEnvironment").field("args", &self.args).finish()
    }
}

#[async_trait]
impl Environment for FlatlandEnvironment {
    fn target_operations(&self) -> Option<u64> {
        self.args.num_operations
    }

    fn timeout_seconds(&self) -> Option<u64> {
        self.args.time_limit_secs
    }

    async fn actor_runners(&mut self) -> Vec<ActorRunner> {
        // We want deterministic but randomly spread values, so we hardcode the seed.
        let mut rng = SmallRng::seed_from_u64(0);

        let (root_flatland_instance, context_view_ref, target_view_ref) =
            FlatlandInstance::new_root(&self.realm_instance.root).await;

        let flatland_instance_runner = {
            // Create the flatland_instance actor
            let rng = SmallRng::from_rng(&mut rng);
            let flatland_instance_actor = Arc::new(Mutex::new(FlatlandActor::new(
                rng,
                root_flatland_instance,
                Arc::clone(&self.realm_instance),
            )));
            ActorRunner::new(
                "flatland_instance_actor",
                Some(Duration::from_millis(250)),
                flatland_instance_actor,
            )
        };

        let input_runner = {
            let injector_registry_proxy: pointerinjector::RegistryProxy = self
                .realm_instance
                .root
                .connect_to_protocol_at_exposed_dir()
                .expect("Failed to connect to pointerinjector registry");
            let identity_matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
            let viewport = pointerinjector::Viewport {
                extents: Some([[0.0, 0.0], [DISPLAY_WIDTH as f32, DISPLAY_HEIGHT as f32]]),
                viewport_to_context_transform: Some(identity_matrix),
                ..Default::default()
            };
            let config = pointerinjector::Config {
                device_id: Some(0),
                device_type: Some(pointerinjector::DeviceType::Touch),
                context: Some(pointerinjector::Context::View(context_view_ref)),
                target: Some(pointerinjector::Target::View(target_view_ref)),
                viewport: Some(viewport),
                dispatch_policy: Some(pointerinjector::DispatchPolicy::TopHitAndAncestorsInTarget),
                scroll_v_range: None,
                scroll_h_range: None,
                buttons: None,
                ..Default::default()
            };
            let (device_proxy, device_server) = create_proxy::<pointerinjector::DeviceMarker>();
            injector_registry_proxy
                .register(config, device_server)
                .await
                .expect("Failed to register injector");

            // Create the input actor.
            let input_actor =
                Arc::new(Mutex::new(InputActor::new(device_proxy, DISPLAY_WIDTH, DISPLAY_HEIGHT)));
            ActorRunner::new("input_actor", Some(Duration::from_millis(16)), input_actor)
        };

        vec![flatland_instance_runner, input_runner]
    }

    async fn reset(&mut self) {
        unreachable!("This stress test does not reset");
    }
}
