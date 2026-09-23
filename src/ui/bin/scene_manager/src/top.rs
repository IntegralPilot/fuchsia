// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::color_transform_manager::ColorTransformManager;
use crate::lib::input_device::InputDeviceType;
use crate::lib::light_sensor::Configuration as LightSensorConfiguration;
use crate::lib::{Dispatcher, Incoming};
use crate::scene_management::{SceneManager, SceneManagerTrait, ViewingDistance};
use anyhow::{Context, Error};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_accessibility::{ColorTransformHandlerMarker, ColorTransformProxy};
use fidl_fuchsia_accessibility_scene as a11y_view;
use fidl_fuchsia_element::{
    GraphicalPresenterRequest, GraphicalPresenterRequestStream, PresentViewError, ViewSpec,
};
use fidl_fuchsia_input_injection::InputDeviceRegistryRequestStream;
use fidl_fuchsia_lightsensor::SensorRequestStream as LightSensorRequestStream;
use fidl_fuchsia_recovery_policy::DeviceRequestStream as FactoryResetDeviceRequestStream;
use fidl_fuchsia_recovery_ui::FactoryResetCountdownRequestStream;
use fidl_fuchsia_session_scene::{
    ManagerRequest as SceneManagerRequest, ManagerRequestStream as SceneManagerRequestStream,
    PresentRootViewError,
};
use fidl_fuchsia_ui_brightness::{
    ColorAdjustmentHandlerRequestStream, ColorAdjustmentRequestStream,
};
use fidl_fuchsia_ui_composition as flatland;
use fidl_fuchsia_ui_composition_internal as fcomp;
use fidl_fuchsia_ui_display_color as color;
use fidl_fuchsia_ui_display_singleton as singleton_display;
use fidl_fuchsia_ui_focus::FocusChainProviderRequestStream;
use fidl_fuchsia_ui_input::DeviceListenerRegistryRequestStream;
use fidl_fuchsia_ui_policy::{
    DeviceListenerRegistryRequestStream as MediaButtonsListenerRegistryRequestStream,
    DisplayBacklightRequestStream,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::stats::InspectorExt;
use fuchsia_inspect::{Inspector, InspectorConfig};
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use log::{error, info, warn};
use scene_manager_structured_config::Config;
use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::rc::Rc;
use zx;

enum ExposedServices {
    ColorAdjustment(ColorAdjustmentRequestStream),
    ColorAdjustmentHandler(ColorAdjustmentHandlerRequestStream),
    DeviceListenerRegistry(DeviceListenerRegistryRequestStream),
    MediaButtonsListenerRegistry(MediaButtonsListenerRegistryRequestStream),
    DisplayBacklight(DisplayBacklightRequestStream),
    FactoryResetCountdown(FactoryResetCountdownRequestStream),
    FactoryReset(FactoryResetDeviceRequestStream),
    FocusChainProvider(FocusChainProviderRequestStream),
    GraphicalPresenter(GraphicalPresenterRequestStream),
    InputDeviceRegistry(InputDeviceRegistryRequestStream),
    LightSensor(LightSensorRequestStream),
    SceneManager(SceneManagerRequestStream),
}

const LIGHT_SENSOR_CONFIGURATION: &'static str = "/sensor-config/config.json";

pub async fn start(
    incoming: Incoming,
    outgoing_dir: fidl::endpoints::ServerEnd<fidl_fuchsia_io::DirectoryMarker>,
    config: zx::Vmo,
    role_name: &str,
    binary_path: &str,
) -> Result<(), Error> {
    if let Err(e) = fuchsia_scheduler::set_role_for_root_vmar(role_name) {
        warn!(e:%; "failed to set vmar role");
    }

    let mut fs = ServiceFs::new_local();

    // Create an inspector that's large enough to store 10 seconds of touchpad
    // events.
    // * Empirically, when all events have two fingers, the total inspect data
    //   size is about 260 KB.
    // * Use a slightly larger value here to allow some headroom. E.g. perhaps
    //   some events have a third finger.
    let inspector = Inspector::new(InspectorConfig::default().size(300 * 1024));
    let inspect_sink_client = incoming
        .connect_protocol::<ClientEnd<fidl_fuchsia_inspect::InspectSinkMarker>>()
        .expect("fuchsia.inspect.InspectSink");
    let _inspect_server_task = inspect_runtime::publish(
        &inspector,
        inspect_runtime::PublishOptions::default().on_inspect_sink_client(inspect_sink_client),
    );

    // Report data on the size of the inspect VMO, and the number of allocation
    // failures encountered. (Allocation failures can lead to missing data.)
    inspector.record_lazy_stats();

    // Initialize tracing.
    //
    // This is done once by the process, rather than making the libraries
    // linked into the component (e.g. input pipeline) initialize tracing.
    //
    // Initializing at the process-level more closely models how a trace
    // provider (e.g. scene_manager) interacts with the trace manager.
    let registry = incoming
        .connect_protocol::<ClientEnd<fidl_fuchsia_tracing_provider::RegistryMarker>>()
        .expect("fuchsia.tracing.provider.Registry");
    fuchsia_trace_provider::trace_provider_create_with_service(registry.into_channel().into_raw());

    // Do not reorder the services below.
    fs.dir("svc")
        .add_fidl_service(ExposedServices::ColorAdjustmentHandler)
        .add_fidl_service(ExposedServices::ColorAdjustment)
        .add_fidl_service(ExposedServices::DeviceListenerRegistry)
        .add_fidl_service(ExposedServices::MediaButtonsListenerRegistry)
        .add_fidl_service(ExposedServices::DisplayBacklight)
        .add_fidl_service(ExposedServices::FactoryResetCountdown)
        .add_fidl_service(ExposedServices::FactoryReset)
        .add_fidl_service(ExposedServices::FocusChainProvider)
        .add_fidl_service(ExposedServices::GraphicalPresenter)
        .add_fidl_service(ExposedServices::InputDeviceRegistry)
        .add_fidl_service(ExposedServices::SceneManager);

    let light_sensor_configuration: Option<LightSensorConfiguration> =
        match File::open(LIGHT_SENSOR_CONFIGURATION) {
            Ok(mut file) => {
                let mut contents = String::new();
                let _: usize =
                    file.read_to_string(&mut contents).context("reading configuration")?;
                Some(serde_json::from_str(&contents).context("parsing configuration")?)
            }
            // Not found signifies that no configuration is supplied for the light sensor, and so it
            // should be configured off.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e).context("opening light sensor config");
            }
        };
    let (light_sensor_server, light_sensor_request_stream_receiver) =
        if light_sensor_configuration.is_some() {
            let (light_sensor_server, light_sensor_request_stream_receiver) =
                crate::light_sensor_server::make_server_and_receiver();
            (Some(light_sensor_server), Some(light_sensor_request_stream_receiver))
        } else {
            (None, None)
        };

    let (input_device_registry_server, input_device_registry_request_stream_receiver) =
        crate::input_device_registry_server::make_server_and_receiver();

    let (device_listener_registry_server, device_listener_registry_request_stream_receiver) =
        crate::device_listener_registry_server::make_server_and_receiver();

    let (
        media_buttons_listener_registry_server,
        media_buttons_listener_registry_request_stream_receiver,
    ) = crate::media_buttons_listener_registry_server::make_server_and_receiver();

    let (factory_reset_countdown_server, factory_reset_countdown_request_stream_receiver) =
        crate::factory_reset_countdown_server::make_server_and_receiver();

    let (factory_reset_device_server, factory_reset_device_request_stream_receiver) =
        crate::factory_reset_device_server::make_server_and_receiver();

    let ownership_proxy = incoming
        .connect_protocol::<fcomp::DisplayOwnershipProxy>()
        .context("while connecting to DisplayOwnership proxy")?;
    let display_ownership = ownership_proxy.get_event().await.expect(
        "Failed to get display ownership. This is usually an upstream issue (Scenic and up), not scene_manager.",
    );
    info!("Instantiating SceneManager");

    let Config {
        supported_input_devices,
        display_rotation,
        display_pixel_density,
        viewing_distance,
        attach_a11y_view,
        enable_merge_touch_events,
        prefetch,
        visual_debugging_level,
        ..
    } = Config::from_vmo(&config).expect("bad config vmo");

    if prefetch {
        let file_proxy = fuchsia_component::directory::open_file_async(
            &incoming,
            binary_path,
            fidl_fuchsia_io::Rights::READ_BYTES | fidl_fuchsia_io::Rights::EXECUTE,
        )
        .context(format!("open {}", binary_path))?;

        let vmo = file_proxy
            .get_backing_memory(
                fidl_fuchsia_io::VmoFlags::READ | fidl_fuchsia_io::VmoFlags::EXECUTE,
            )
            .await
            .context("call get_backing_memory")?
            .map_err(zx::Status::err_from_raw)
            .context("get_backing_memory")?;
        let size = vmo.get_size().context("get executable vmo size")?;
        vmo.op_range(zx::VmoOp::ALWAYS_NEED, 0, size).context("pin executable vmo")?;

        info!("Prefetched binary: {}", binary_path);
    }

    let display_pixel_density = match display_pixel_density.trim().parse::<f32>() {
        Ok(density) => {
            if density < 0.0 {
                None
            } else {
                Some(density)
            }
        }
        Err(_) => {
            warn!(
                "Failed to parse display_pixel_density value from structured config - expected a decimal, but got: {display_pixel_density}. Falling back to default."
            );
            None
        }
    };

    let viewing_distance = match viewing_distance.to_lowercase().trim() {
        "handheld" => Some(ViewingDistance::Handheld),
        "close" => Some(ViewingDistance::Close),
        "near" => Some(ViewingDistance::Near),
        "midrange" => Some(ViewingDistance::Midrange),
        "far" => Some(ViewingDistance::Far),
        _ => {
            warn!("No viewing_distance config value provided, falling back to default.");
            None
        }
    };

    let flatland_display = incoming.connect_protocol::<flatland::FlatlandDisplayProxy>()?;
    let singleton_display_info = incoming.connect_protocol::<singleton_display::InfoProxy>()?;
    let root_flatland = incoming.connect_protocol::<flatland::FlatlandProxy>()?;
    let pointerinjector_flatland = incoming.connect_protocol::<flatland::FlatlandProxy>()?;
    let scene_flatland = incoming.connect_protocol::<flatland::FlatlandProxy>()?;
    let a11y_view_provider = if attach_a11y_view {
        Some(incoming.connect_protocol::<a11y_view::ProviderProxy>()?)
    } else {
        None
    };
    let scene_manager: Rc<dyn SceneManagerTrait> = Rc::new(
        SceneManager::new(
            flatland_display,
            singleton_display_info,
            root_flatland,
            pointerinjector_flatland,
            scene_flatland,
            a11y_view_provider,
            display_rotation,
            display_pixel_density,
            viewing_distance,
        )
        .await?,
    );

    let (focus_chain_publisher, focus_chain_stream_handler) =
        focus_chain_provider::make_publisher_and_stream_handler();

    // Create a node under root to hang all input pipeline inspect data off of.
    let inspect_node = inspector.root().create_child("input_pipeline");

    // Start input pipeline.
    let has_light_sensor_configuration = light_sensor_configuration.is_some();
    #[allow(clippy::collection_is_never_read)]
    let mut _input_tasks = Vec::new();
    if let Ok((input_pipeline, tasks)) = crate::input_pipeline::handle_input(
        &incoming,
        scene_manager.clone(),
        input_device_registry_request_stream_receiver,
        light_sensor_request_stream_receiver,
        media_buttons_listener_registry_request_stream_receiver,
        device_listener_registry_request_stream_receiver,
        factory_reset_countdown_request_stream_receiver,
        factory_reset_device_request_stream_receiver,
        inspect_node,
        display_ownership,
        focus_chain_publisher,
        supported_input_devices,
        light_sensor_configuration,
        enable_merge_touch_events,
        visual_debugging_level,
    )
    .await
    {
        _input_tasks = tasks;
        if input_pipeline.input_device_types().contains(&InputDeviceType::LightSensor)
            && has_light_sensor_configuration
        {
            fs.dir("svc").add_fidl_service(ExposedServices::LightSensor);
        }
        _input_tasks.push(crate::input_pipeline::InputTask::Handle(Dispatcher::spawn_local(
            input_pipeline.handle_input_events(),
        )));
    };

    let color_transform_manager =
        create_color_transform_manager(&incoming, attach_a11y_view, Rc::clone(&scene_manager))
            .await?;

    fs.serve_connection(outgoing_dir).context("serve outgoing dir")?;

    // Concurrency note: spawn a local task in the match branch if the protocol must serve more
    // than a single client at a time.
    let mut service_tasks = FuturesUnordered::new();
    let mut fs = fs.fuse();
    loop {
        futures::select! {
            service_request = fs.next() => {
                let request = match service_request {
                    Some(r) => r,
                    None => break,
                };
                match request {
                    ExposedServices::ColorAdjustmentHandler(request_stream) => {
                        if attach_a11y_view {
                            ColorTransformManager::handle_color_adjustment_handler_request_stream(
                                Rc::clone(color_transform_manager.as_ref().unwrap()),
                                request_stream,
                            );
                        } else {
                            warn!("failed to forward as A11y protocols are disabled");
                        }
                    }
                    ExposedServices::ColorAdjustment(request_stream) => {
                        if attach_a11y_view {
                            ColorTransformManager::handle_color_adjustment_request_stream(
                                Rc::clone(color_transform_manager.as_ref().unwrap()),
                                request_stream,
                            );
                        } else {
                            warn!("failed to forward as A11y protocols are disabled");
                        }
                    }
                    ExposedServices::DisplayBacklight(request_stream) => {
                        if attach_a11y_view {
                            ColorTransformManager::handle_display_backlight_request_stream(
                                Rc::clone(color_transform_manager.as_ref().unwrap()),
                                request_stream,
                            );
                        } else {
                            warn!("failed to forward as A11y protocols are disabled");
                        }
                    }
                    ExposedServices::FocusChainProvider(request_stream) => {
                        let task = focus_chain_stream_handler.handle_request_stream(request_stream);
                        service_tasks.push(task);
                    }
                    ExposedServices::SceneManager(request_stream) => {
                        let task = fasync::Task::local(handle_scene_manager_request_stream(
                            request_stream,
                            Rc::clone(&scene_manager),
                        ));
                        service_tasks.push(task);
                    }
                    ExposedServices::InputDeviceRegistry(request_stream) => {
                        match &input_device_registry_server.handle_request(request_stream).await {
                            Ok(()) => (),
                            Err(e) => {
                                // If `handle_request()` returns `Err`, then the `unbounded_send()` call
                                // from `handle_request()` failed with either:
                                // * `TrySendError::SendErrorKind::Full`, or
                                // * `TrySendError::SendErrorKind::Disconnected`.
                                //
                                // These are unexpected, because:
                                // * `Full` can't happen, because `InputDeviceRegistryServer`
                                //   uses an `UnboundedSender`.
                                // * `Disconnected` is highly unlikely, because the corresponding
                                //   `UnboundedReceiver` lives in `main::input_fut`, and `input_fut's`
                                //   lifetime is nearly as long as `input_device_registry_server's`.
                                //
                                // Nonetheless, InputDeviceRegistry isn't critical to production use.
                                // So we just log the error and move on.
                                warn!(
                                    "failed to forward InputDeviceRegistryRequestStream: {:?}; \
                                        must restart to enable input injection",
                                    e
                                )
                            }
                        }
                    }
                    ExposedServices::LightSensor(request_stream) => {
                        if let Some(light_sensor_server) = light_sensor_server.as_ref() {
                            match light_sensor_server.handle_request(request_stream).await {
                                Ok(()) => (),
                                Err(e) => {
                                    warn!(
                                        "failed to forward light sensor request via LightSensorRequestStream: {e:?}"
                                    );
                                }
                            }
                        }
                    }
                    ExposedServices::DeviceListenerRegistry(request_stream) => {
                        match &device_listener_registry_server.handle_request(request_stream).await {
                            Ok(()) => (),
                            Err(e) => {
                                warn!(
                                    "failed to forward device listener request via DeviceListenerRegistryRequestStream: {:?}",
                                    e
                                )
                            }
                        }
                    }
                    ExposedServices::MediaButtonsListenerRegistry(request_stream) => {
                        match &media_buttons_listener_registry_server.handle_request(request_stream).await {
                            Ok(()) => (),
                            Err(e) => {
                                warn!(
                                    "failed to forward media buttons listener request via DeviceListenerRegistryRequestStream: {:?}",
                                    e
                                )
                            }
                        }
                    }
                    ExposedServices::FactoryResetCountdown(request_stream) => {
                        match &factory_reset_countdown_server.handle_request(request_stream).await {
                            Ok(()) => (),
                            Err(e) => {
                                warn!("failed to forward FactoryResetCountdown: {:?}", e)
                            }
                        }
                    }
                    ExposedServices::FactoryReset(request_stream) => {
                        match &factory_reset_device_server.handle_request(request_stream).await {
                            Ok(()) => (),
                            Err(e) => {
                                warn!("failed to forward fuchsia.recovery.policy.Device: {:?}", e)
                            }
                        }
                    }
                    ExposedServices::GraphicalPresenter(stream) => {
                        let task = fasync::Task::local(handle_graphical_presenter_request_stream(
                            stream,
                            Rc::clone(&scene_manager),
                        ));
                        service_tasks.push(task);
                    }
                }
            }
            _ = service_tasks.select_next_some() => {}
        }
    }

    info!("Finished service handler loop; exiting main.");
    Ok(())
}

pub async fn create_color_transform_manager(
    incoming: &Incoming,
    attach_a11y_view: bool,
    scene_manager: Rc<dyn SceneManagerTrait>,
) -> Result<Option<Rc<RefCell<ColorTransformManager>>>, Error> {
    // Create and register a ColorTransformManager if we are attaching A11y View.
    if !attach_a11y_view {
        return Ok(None);
    }

    let color_converter = incoming.connect_protocol::<color::ConverterProxy>()?;
    let color_transform_manager =
        ColorTransformManager::new(color_converter, Rc::clone(&scene_manager));

    let (color_transform_handler_client, color_transform_handler_server) =
        fidl::endpoints::create_request_stream::<ColorTransformHandlerMarker>();
    match incoming.connect_protocol::<ColorTransformProxy>() {
        Err(e) => {
            error!("Failed to connect to fuchsia.accessibility.color_transform: {:?}", e);
            Err(e.into())
        }
        Ok(proxy) => match proxy.register_color_transform_handler(color_transform_handler_client) {
            Err(e) => {
                error!("Failed to call RegisterColorTransformHandler: {:?}", e);
                Err(e.into())
            }
            Ok(()) => {
                ColorTransformManager::handle_color_transform_request_stream(
                    Rc::clone(&color_transform_manager),
                    color_transform_handler_server,
                );
                Ok(Some(color_transform_manager))
            }
        },
    }
}

pub async fn handle_scene_manager_request_stream(
    mut request_stream: SceneManagerRequestStream,
    scene_manager: Rc<dyn SceneManagerTrait>,
) {
    while let Ok(Some(request)) = request_stream.try_next().await {
        match request {
            SceneManagerRequest::SetRootView { view_provider, responder } => {
                info!("Processing SceneManagerRequest::SetRootView().");
                let proxy = view_provider.into_proxy();
                let set_root_view_result =
                    scene_manager.set_root_view_deprecated(proxy).await.map_err(|e| {
                        error!("Failed to obtain ViewRef from SetRootView(): {}", e);
                        map_present_root_view_error(e)
                    });
                if let Err(e) = responder.send(set_root_view_result) {
                    error!("Error responding to SetRootView(): {}", e);
                }
            }
            SceneManagerRequest::PresentRootViewLegacy {
                view_holder_token: _,
                view_ref: _,
                responder,
            } => {
                error!(
                    "Unsupported call to fuchsia.session.scene.Manager/PresentRootViewLegacy() (GFX only)."
                );
                if let Err(e) = responder.send(Err(PresentRootViewError::InternalError)) {
                    error!("Error responding to PresentRootViewLegacy(): {}", e);
                }
            }
            SceneManagerRequest::PresentRootView { viewport_creation_token, responder } => {
                info!("Processing SceneManagerRequest::PresentRootView().");
                let set_root_view_result =
                    scene_manager.set_root_view(viewport_creation_token, None).await.map_err(|e| {
                        error!("Failed to obtain ViewRef from PresentRootView(): {}", e);
                        map_present_root_view_error(e)
                    });
                if let Err(e) = responder.send(set_root_view_result) {
                    error!("Error responding to PresentRootView(): {}", e);
                }
            }
        };
    }
}

pub async fn handle_graphical_presenter_request_stream(
    mut request_stream: GraphicalPresenterRequestStream,
    scene_manager: Rc<dyn SceneManagerTrait>,
) {
    while let Ok(Some(request)) = request_stream.try_next().await {
        match request {
            GraphicalPresenterRequest::PresentView { view_spec, responder, .. } => {
                match view_spec {
                    ViewSpec {
                        view_holder_token: Some(_),
                        view_ref: _,
                        viewport_creation_token: None,
                        ..
                    } => {
                        error!(
                            "Processing fuchsia.element.GraphicalPresenter/PresentView() with GFX view tokens."
                        );
                        if let Err(e) = responder.send(Err(PresentViewError::InvalidArgs)) {
                            error!("Error responding to PresentView(): {}", e);
                        }
                    }
                    ViewSpec {
                        viewport_creation_token: Some(viewport_creation_token),
                        view_holder_token: None,
                        view_ref: None,
                        ..
                    } => {
                        info!(
                            "Processing fuchsia.element.GraphicalPresenter/PresentView() with Flatland view tokens."
                        );
                        let set_root_view_result = scene_manager
                            .set_root_view(viewport_creation_token, None)
                            .await
                            .map_err(|e| {
                                error!("Failed to PresentView() - Flatland: {}", e);
                                PresentViewError::InvalidArgs
                            });
                        if let Err(e) = responder.send(set_root_view_result) {
                            error!("Error responding to PresentView(): {}", e);
                        }
                    }
                    _ => {
                        error!("Failed to retrieve valid tokens from ViewSpec");
                        if let Err(e) = responder.send(Err(PresentViewError::InvalidArgs)) {
                            error!("Error responding to PresentView(): {}", e);
                        }
                    }
                };
            }
        };
        info!("No longer processing fuchsia.element.GraphicalPresenter request stream.");
    }
}

fn map_present_root_view_error(e: anyhow::Error) -> PresentRootViewError {
    if let Some(fidl_err) = e.downcast_ref::<fidl::Error>() {
        if let fidl::Error::ClientChannelClosed { epitaph, .. } = fidl_err
            && *epitaph == zx::Status::TIMED_OUT
        {
            return PresentRootViewError::ViewConnectionTimeout;
        }
    }
    PresentRootViewError::InternalError
}

#[cfg(test)]

mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_element::GraphicalPresenterMarker;
    use fuchsia_scenic as scenic;
    use scene_management_mocks::MockSceneManager;

    /// Tests that handle_graphical_presenter_request_stream, when receiving a GFX present_view request, errors.
    #[fuchsia::test]
    async fn handle_graphical_presenter_request_stream_present_view_gfx_errors() -> Result<(), Error>
    {
        let (proxy, stream) = create_proxy_and_stream::<GraphicalPresenterMarker>();
        let scene_manager = Rc::new(MockSceneManager::new());
        let mock_scene_manager = Rc::clone(&scene_manager);
        let _presenter_task = fasync::Task::local(handle_graphical_presenter_request_stream(
            stream,
            mock_scene_manager,
        ));

        let view_token_pair = scenic::ViewTokenPair::new()?;
        let view_ref_pair = scenic::ViewRefPair::new()?;
        let view_spec = ViewSpec {
            view_holder_token: Some(view_token_pair.view_holder_token),
            view_ref: Some(view_ref_pair.view_ref),
            ..Default::default()
        };
        if let Err(present_view_result) = proxy
            .present_view(
                view_spec, /* annotation controller */ None, /* view controller */ None,
            )
            .await
            .unwrap()
        {
            assert_eq!(present_view_result, PresentViewError::InvalidArgs);
        } else {
            panic!("Expected an error from present_view().");
        }

        Ok(())
    }

    /// Tests that handle_graphical_presenter_request_stream, when receiving a Flatland present_view request, passes the viewport_creation_token and None to set_root_view().
    #[fuchsia::test]
    async fn handle_graphical_presenter_request_stream_presents_view_flatland() -> Result<(), Error>
    {
        let (proxy, stream) = create_proxy_and_stream::<GraphicalPresenterMarker>();
        let scene_manager = Rc::new(MockSceneManager::new());
        let mock_scene_manager = Rc::clone(&scene_manager);
        let _presenter_task = fasync::Task::local(handle_graphical_presenter_request_stream(
            stream,
            mock_scene_manager,
        ));

        let view_creation_token_pair = scenic::flatland::ViewCreationTokenPair::new()?;
        let expected_viewport_creation_token_koid =
            view_creation_token_pair.viewport_creation_token.value.koid();
        let view_spec = ViewSpec {
            viewport_creation_token: Some(view_creation_token_pair.viewport_creation_token),
            ..Default::default()
        };

        let _ = proxy
            .present_view(
                view_spec, /* annotation controller */ None, /* view controller */ None,
            )
            .await;

        let (recorded_viewport_creation_token, recorded_view_ref) =
            scene_manager.get_set_root_view_called_args();
        assert_eq!(
            recorded_viewport_creation_token.value.koid(),
            expected_viewport_creation_token_koid
        );

        assert_eq!(recorded_view_ref, None);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_create_color_transform_manager_attach_a11y_view_false() -> Result<(), Error> {
        let scene_manager: Rc<dyn SceneManagerTrait> = Rc::new(MockSceneManager::new());
        let result =
            create_color_transform_manager(&crate::lib::Incoming::new(), false, scene_manager)
                .await?;
        assert!(result.is_none());
        Ok(())
    }
}
