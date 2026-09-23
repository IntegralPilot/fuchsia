// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device_listener_registry::DeviceListenerRegistry;
use crate::display_ownership::DisplayOwnership;
use crate::focus_listener::FocusListener;
use crate::input_device::{InputEventType, InputPipelineFeatureFlags};
use crate::input_handler::Handler;
use crate::{Dispatcher, Incoming, Transport, input_device, input_handler, metrics};
use anyhow::{Context, Error, format_err};
use fidl::endpoints::Proxy;
use focus_chain_provider::FocusChainProviderPublisher;

use fuchsia_inspect::NumericProperty;
use fuchsia_inspect::health::Reporter;
use fuchsia_sync::Mutex;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::future::LocalBoxFuture;
use futures::{FutureExt, StreamExt, TryStreamExt};
use itertools::Itertools;
use metrics_registry::*;
use sorted_vec_map::SortedVecMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use strum::EnumCount;

/// Use a self incremental u32 unique id for device_id.
///
/// device id start from 10 to avoid conflict with default devices in Starnix.
/// Currently, Starnix using 0 and 1 as default devices' id. Starnix need to
/// use default devices to deliver events from physical devices until we have
/// API to expose device changes to UI clients.
static NEXT_DEVICE_ID: LazyLock<AtomicU32> = LazyLock::new(|| AtomicU32::new(10));

/// Each time this function is invoked, it returns the current value of its
/// internal counter (serving as a unique id for device_id) and then increments
/// that counter in preparation for the next call.
fn get_next_device_id() -> u32 {
    NEXT_DEVICE_ID.fetch_add(1, Ordering::SeqCst)
}

type BoxedInputDeviceBinding = Box<dyn input_device::InputDeviceBinding>;

pub struct InputDeviceBindingAndTask {
    pub binding: BoxedInputDeviceBinding,
    pub task: crate::dispatcher::TaskHandle<()>,
}

/// An [`InputDeviceBindingMap`] maps an input device to one or more InputDeviceBindings and their tasks.
/// It uses unique device id as key.
pub type InputDeviceBindingMap = Arc<Mutex<SortedVecMap<u32, Vec<InputDeviceBindingAndTask>>>>;

/// An input pipeline assembly.
///
/// Represents a partial stage of the input pipeline which accepts inputs through an asynchronous
/// sender channel, and emits outputs through an asynchronous receiver channel.  Use [new] to
/// create a new assembly.  Use [add_handler], or [add_all_handlers] to add the input pipeline
/// handlers to use.  When done, [InputPipeline::new] can be used to make a new input pipeline.
///
/// # Implementation notes
///
/// Internally, when a new [InputPipelineAssembly] is created with multiple [InputHandler]s, the
/// handlers are connected together using async queues.  This allows fully streamed processing of
/// input events, and also allows some pipeline stages to generate events spontaneously, i.e.
/// without an external stimulus.
pub struct InputPipelineAssembly {
    /// The top-level sender: send into this queue to inject an event into the input
    /// pipeline.
    sender: UnboundedSender<Vec<input_device::InputEvent>>,
    /// The bottom-level receiver: any events that fall through the entire pipeline can
    /// be read from this receiver.
    receiver: UnboundedReceiver<Vec<input_device::InputEvent>>,

    /// The input handlers that comprise the input pipeline.
    handlers: Vec<Rc<dyn input_handler::BatchInputHandler>>,

    /// The display ownership watcher task.
    display_ownership_fut: Option<LocalBoxFuture<'static, ()>>,

    /// The focus listener task.
    focus_listener_fut: Option<LocalBoxFuture<'static, ()>>,

    /// The metrics logger.
    metrics_logger: metrics::MetricsLogger,
}

impl InputPipelineAssembly {
    /// Create a new but empty [InputPipelineAssembly]. Use [add_handler] or similar
    /// to add new handlers to it.
    pub fn new(metrics_logger: metrics::MetricsLogger) -> Self {
        let (sender, receiver) = mpsc::unbounded();
        InputPipelineAssembly {
            sender,
            receiver,
            handlers: vec![],
            metrics_logger,
            display_ownership_fut: None,
            focus_listener_fut: None,
        }
    }

    /// Adds another [input_handler::BatchInputHandler] into the [InputPipelineAssembly]. The handlers
    /// are invoked in the order they are added. Returns `Self` for chaining.
    pub fn add_handler(mut self, handler: Rc<dyn input_handler::BatchInputHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    /// Adds all handlers into the assembly in the order they appear in `handlers`.
    pub fn add_all_handlers(self, handlers: Vec<Rc<dyn input_handler::BatchInputHandler>>) -> Self {
        handlers.into_iter().fold(self, |assembly, handler| assembly.add_handler(handler))
    }

    pub fn add_display_ownership(
        mut self,
        display_ownership_event: zx::Event,
        input_handlers_node: &fuchsia_inspect::Node,
    ) -> InputPipelineAssembly {
        let h = DisplayOwnership::new(
            display_ownership_event,
            input_handlers_node,
            self.metrics_logger.clone(),
        );
        let metrics_logger_clone = self.metrics_logger.clone();
        let h_clone = h.clone();
        let sender_clone = self.sender.clone();
        let display_ownership_fut = Box::pin(async move {
            h_clone.clone().set_handler_healthy();
            h_clone.clone()
                .handle_ownership_change(sender_clone)
                .await
                .map_err(|e| {
                    metrics_logger_clone.log_error(
                        InputPipelineErrorMetricDimensionEvent::InputPipelineDisplayOwnershipIsNotSupposedToTerminate,
                        std::format!(
                            "display ownership is not supposed to terminate - this is likely a problem: {:?}", e));
                        })
                        .unwrap();
            h_clone.set_handler_unhealthy("Receive loop terminated for handler: DisplayOwnership");
        });
        self.display_ownership_fut = Some(display_ownership_fut);
        self.add_handler(h)
    }

    /// Deconstructs the assembly into constituent components, used when constructing
    /// [InputPipeline].
    ///
    /// You should call [catch_unhandled] on the returned [async_channel::Receiver], and
    /// [run] on the returned [fuchsia_async::Tasks] (or supply own equivalents).
    fn into_components(
        self,
    ) -> (
        UnboundedSender<Vec<input_device::InputEvent>>,
        UnboundedReceiver<Vec<input_device::InputEvent>>,
        Vec<Rc<dyn input_handler::BatchInputHandler>>,
        metrics::MetricsLogger,
        Option<LocalBoxFuture<'static, ()>>,
        Option<LocalBoxFuture<'static, ()>>,
    ) {
        (
            self.sender,
            self.receiver,
            self.handlers,
            self.metrics_logger,
            self.display_ownership_fut,
            self.focus_listener_fut,
        )
    }

    pub fn add_focus_listener(
        mut self,
        incoming: &Incoming,
        focus_chain_publisher: FocusChainProviderPublisher,
    ) -> Self {
        let metrics_logger_clone = self.metrics_logger.clone();
        let incoming2 = incoming.clone();
        let focus_listener_fut = Box::pin(async move {
            if let Ok(mut focus_listener) = FocusListener::new(
                &incoming2,
                focus_chain_publisher,
                metrics_logger_clone,
            )
            .map_err(|e| {
                log::warn!("could not create focus listener, focus will not be dispatched: {:?}", e)
            }) {
                // This will await indefinitely and process focus messages in a loop, unless there
                // is a problem.
                let _result = focus_listener
                    .dispatch_focus_changes()
                    .await
                    .map(|_| {
                        log::warn!("dispatch focus loop ended, focus will no longer be dispatched")
                    })
                    .map_err(|e| {
                        panic!("could not dispatch focus changes, this is a fatal error: {:?}", e)
                    });
            }
        });
        self.focus_listener_fut = Some(focus_listener_fut);
        self
    }
}

/// An [`InputPipeline`] manages input devices and propagates input events through input handlers.
///
/// On creation, clients declare what types of input devices an [`InputPipeline`] manages. The
/// [`InputPipeline`] will continuously detect new input devices of supported type(s).
///
/// # Example
/// ```
/// let ime_handler =
///     ImeHandler::new(scene_manager.session.clone(), scene_manager.compositor_id).await?;
/// let touch_handler = TouchHandler::new(
///     scene_manager.session.clone(),
///     scene_manager.compositor_id,
///     scene_manager.display_size
/// ).await?;
///
/// let assembly = InputPipelineAssembly::new()
///     .add_handler(Box::new(ime_handler)),
///     .add_handler(Box::new(touch_handler)),
/// let input_pipeline = InputPipeline::new(
///     vec![
///         input_device::InputDeviceType::Touch,
///         input_device::InputDeviceType::Keyboard,
///     ],
///     assembly,
/// );
/// input_pipeline.handle_input_events().await;
/// ```
pub struct InputPipeline {
    /// The entry point into the input handler pipeline. Incoming input events should
    /// be inserted into this async queue, and the input pipeline will ensure that they
    /// are propagated through all the input handlers in the appropriate sequence.
    pipeline_sender: UnboundedSender<Vec<input_device::InputEvent>>,

    /// A clone of this sender is given to every InputDeviceBinding that this pipeline owns.
    /// Each InputDeviceBinding will send InputEvents to the pipeline through this channel.
    device_event_sender: UnboundedSender<Vec<input_device::InputEvent>>,

    /// Receives InputEvents from all InputDeviceBindings that this pipeline owns.
    device_event_receiver: UnboundedReceiver<Vec<input_device::InputEvent>>,

    /// The types of devices this pipeline supports.
    input_device_types: Vec<input_device::InputDeviceType>,

    /// The InputDeviceBindings bound to this pipeline.
    input_device_bindings: InputDeviceBindingMap,

    /// This node is bound to the lifetime of this InputPipeline.
    /// Inspect data will be dumped for this pipeline as long as it exists.
    inspect_node: fuchsia_inspect::Node,

    /// The metrics logger.
    metrics_logger: metrics::MetricsLogger,

    /// The feature flags for the input pipeline.
    pub feature_flags: input_device::InputPipelineFeatureFlags,
    /// The display ownership future, if configured.
    display_ownership_fut: Option<LocalBoxFuture<'static, ()>>,

    /// The focus listener future, if configured.
    focus_listener_fut: Option<LocalBoxFuture<'static, ()>>,

    /// The watcher future.
    watcher_fut: Option<LocalBoxFuture<'static, ()>>,

    /// The runner future.
    runner_fut: Option<LocalBoxFuture<'static, ()>>,

    /// The registry for device listeners.
    pub device_listener_registry: DeviceListenerRegistry,
}

impl InputPipeline {
    fn new_common(
        input_device_types: Vec<input_device::InputDeviceType>,
        assembly: InputPipelineAssembly,
        inspect_node: fuchsia_inspect::Node,
        feature_flags: input_device::InputPipelineFeatureFlags,
    ) -> Self {
        let (
            pipeline_sender,
            receiver,
            handlers,
            metrics_logger,
            display_ownership_fut,
            focus_listener_fut,
        ) = assembly.into_components();

        let mut handlers_count = handlers.len();
        let display_ownership_fut = if let Some(fut) = display_ownership_fut {
            handlers_count += 1;
            Some(
                async move {
                    fut.await;
                    panic!("display_ownership_fut exited unexpectedly, which compromises device state tracking. Terminating to avoid inconsistent state.");
                }
                .boxed_local(),
            )
        } else {
            None
        };

        let focus_listener_fut = if let Some(fut) = focus_listener_fut {
            handlers_count += 1;
            Some(
                async move {
                    fut.await;
                    panic!("focus_listener_fut exited unexpectedly, which breaks input routing. Terminating to avoid inconsistent state.");
                }
                .boxed_local(),
            )
        } else {
            None
        };

        // Add properties to inspect node
        inspect_node.record_string("supported_input_devices", input_device_types.iter().join(", "));
        inspect_node.record_uint("handlers_registered", handlers_count as u64);
        inspect_node.record_uint("handlers_healthy", handlers_count as u64);

        // Initializes all handlers and starts the input pipeline loop.
        let runner_fut = Some(InputPipeline::run(receiver, handlers, metrics_logger.clone()));

        let (device_event_sender, device_event_receiver) = futures::channel::mpsc::unbounded();
        let input_device_bindings: InputDeviceBindingMap =
            Arc::new(Mutex::new(SortedVecMap::new()));
        let device_listener_registry = DeviceListenerRegistry::new();
        InputPipeline {
            pipeline_sender,
            device_event_sender,
            device_event_receiver,
            input_device_types,
            input_device_bindings,
            inspect_node,
            metrics_logger,
            feature_flags,
            display_ownership_fut,
            focus_listener_fut,
            watcher_fut: None,
            runner_fut,
            device_listener_registry,
        }
    }

    /// Creates a new [`InputPipeline`] for integration testing.
    /// Unlike a production input pipeline, this pipeline will not monitor
    /// `/svc/fuchsia.input.report.Service` for devices.
    ///
    /// # Parameters
    /// - `input_device_types`: The types of devices the new [`InputPipeline`] will support.
    /// - `assembly`: The input handlers that the [`InputPipeline`] sends InputEvents to.
    pub fn new_for_test(
        input_device_types: Vec<input_device::InputDeviceType>,
        assembly: InputPipelineAssembly,
    ) -> Self {
        let inspector = fuchsia_inspect::Inspector::default();
        let root = inspector.root();
        let test_node = root.create_child("input_pipeline");
        Self::new_common(
            input_device_types,
            assembly,
            test_node,
            input_device::InputPipelineFeatureFlags { enable_merge_touch_events: false },
        )
    }

    /// Creates a new [`InputPipeline`] for production use.
    ///
    /// # Parameters
    /// - `input_device_types`: The types of devices the new [`InputPipeline`] will support.
    /// - `assembly`: The input handlers that the [`InputPipeline`] sends InputEvents to.
    /// - `inspect_node`: The root node for InputPipeline's Inspect tree
    pub fn new(
        incoming: &Incoming,
        input_device_types: Vec<input_device::InputDeviceType>,
        assembly: InputPipelineAssembly,
        inspect_node: fuchsia_inspect::Node,
        feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<Self, Error> {
        let mut input_pipeline =
            Self::new_common(input_device_types, assembly, inspect_node, feature_flags);
        let input_device_types = input_pipeline.input_device_types.clone();
        let input_event_sender = input_pipeline.device_event_sender.clone();
        let input_device_bindings = input_pipeline.input_device_bindings.clone();
        let device_listener_registry = input_pipeline.device_listener_registry.clone();
        let devices_node = input_pipeline.inspect_node.create_child("input_devices");
        let devices_node_weak = devices_node.clone_weak();
        input_pipeline.inspect_node.record(devices_node);
        let feature_flags = input_pipeline.feature_flags.clone();
        let incoming = incoming.clone();
        let watcher_fut = async move {
            let devices_discovered = devices_node_weak.create_uint("devices_discovered", 0);
            let devices_connected = devices_node_weak.create_uint("devices_connected", 0);
            // Watches the input device service for new input devices. Creates new InputDeviceBindings
            // that send InputEvents to `input_event_receiver`.
            match async {
                let service = incoming
                    .open_service(fidl_fuchsia_input_report::ServiceMarker)
                    .context("failed to open service")?;
                Self::watch_for_devices(
                    service,
                    input_device_types,
                    input_event_sender,
                    input_device_bindings,
                    &devices_node_weak,
                    &devices_discovered,
                    &devices_connected,
                    false, /* break_on_idle */
                    feature_flags,
                    metrics_logger.clone(),
                    device_listener_registry,
                )
                .await
                .context("failed to watch for devices")
            }
            .await
            {
                Ok(()) => {}
                Err(err) => {
                    // This error is usually benign in tests: it means that the setup does not
                    // support dynamic device discovery. Almost no tests support dynamic
                    // device discovery, and they also do not need those.
                    metrics_logger.log_warn(
                        InputPipelineErrorMetricDimensionEvent::InputPipelineUnableToWatchForNewInputDevices,
                        std::format!(
                            "Input pipeline is unable to watch for new input devices: {:?}",
                            err
                        ));
                }
            }
            devices_node_weak.record(devices_discovered);
            devices_node_weak.record(devices_connected);
        }.boxed_local();

        input_pipeline.watcher_fut = Some(watcher_fut);

        Ok(input_pipeline)
    }

    /// Gets the input device bindings.
    pub fn input_device_bindings(&self) -> &InputDeviceBindingMap {
        &self.input_device_bindings
    }

    /// Gets the input device sender: this is the channel that should be cloned
    /// and used for injecting events from the drivers into the input pipeline.
    pub fn input_event_sender(&self) -> &UnboundedSender<Vec<input_device::InputEvent>> {
        &self.device_event_sender
    }

    /// Gets a list of input device types supported by this input pipeline.
    pub fn input_device_types(&self) -> &[input_device::InputDeviceType] {
        &self.input_device_types
    }

    /// Gets the device listener registry.
    pub fn device_listener_registry(&self) -> &DeviceListenerRegistry {
        &self.device_listener_registry
    }

    /// Forwards all input events into the input pipeline.
    pub async fn handle_input_events(self) {
        let metrics_logger_clone = self.metrics_logger.clone();
        let mut device_event_receiver = self.device_event_receiver;
        let pipeline_sender = self.pipeline_sender.clone();

        let forwarder = async move {
            while let Some(input_event) = device_event_receiver.next().await {
                if let Err(e) = pipeline_sender.unbounded_send(input_event) {
                    metrics_logger_clone.log_error(
                        InputPipelineErrorMetricDimensionEvent::InputPipelineCouldNotForwardEventFromDriver,
                        std::format!("could not forward event from driver: {:?}", e));
                }
            }

            metrics_logger_clone.log_error(
                InputPipelineErrorMetricDimensionEvent::InputPipelineStopHandlingEvents,
                "Input pipeline stopped handling input events.".to_string(),
            );
        }.boxed_local();

        let tasks = crate::task::InputPipelineTasks {
            watcher: self.watcher_fut.unwrap_or_else(|| Box::pin(futures::future::pending())),
            runner: self.runner_fut.unwrap_or_else(|| Box::pin(futures::future::pending())),
            display_ownership: self.display_ownership_fut,
            focus_listener: self.focus_listener_fut,
            forwarder,
        };

        tasks.run().await;
    }

    async fn connect_and_bind_device(
        instance: &fidl_fuchsia_input_report::ServiceProxy,
        device_types: &[input_device::InputDeviceType],
        input_event_sender: &UnboundedSender<Vec<input_device::InputEvent>>,
        bindings: &InputDeviceBindingMap,
        input_devices_node: &fuchsia_inspect::Node,
        devices_discovered: &fuchsia_inspect::UintProperty,
        devices_connected: &fuchsia_inspect::UintProperty,
        feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
        device_listener_registry: DeviceListenerRegistry,
    ) {
        let filename = instance.instance_name().to_string();
        log::info!("found input device {}", filename);
        devices_discovered.add(1);

        let res = async {
            let device_proxy =
                instance.connect_to_input_device().context("connect to input device")?;
            let channel = device_proxy
                .into_client_end()
                .map_err(|_| format_err!("failed to get client end"))?
                .into_channel();
            let device_client = fidl_next::ClientEnd::<
                fidl_next_fuchsia_input_report::InputDevice,
                zx::Channel,
            >::from_untyped(channel);
            let device_client = Dispatcher::client_from_zx_channel(device_client);
            let (device_client, join_handle) = device_client.spawn_full();
            let device_id = get_next_device_id();
            if let Some(descriptor) = add_device_bindings(
                device_types,
                &filename,
                device_client,
                input_event_sender,
                bindings,
                device_id,
                input_devices_node,
                Some(devices_connected),
                feature_flags,
                metrics_logger,
                false,
            )
            .await
            {
                device_listener_registry.notify_device_changed(
                    fidl_next_fuchsia_ui_input::Action::Added,
                    device_id,
                    descriptor.clone(),
                );
                let device_listener_registry_clone = device_listener_registry.clone();
                let bindings_clone = bindings.clone();
                let watch_task = Dispatcher::spawn_local(async move {
                    let _ = join_handle.await;
                    log::info!("Device {} disconnected", device_id);
                    device_listener_registry_clone.notify_device_changed(
                        fidl_next_fuchsia_ui_input::Action::Removed,
                        device_id,
                        descriptor,
                    );
                    bindings_clone.lock().remove(&device_id);
                });
                watch_task.detach();
            }
            Ok::<(), Error>(())
        }
        .await;

        if let Err(e) = res {
            log::error!("Failed to connect and bind input device {}: {:?}", filename, e);
        }
    }

    /// Watches the input report service for new input devices. Creates InputDeviceBindings
    /// if new devices match a type in `device_types`.
    ///
    /// # Parameters
    /// - `service`: The service to watch for new devices.
    /// - `device_types`: The types of devices to watch for.
    /// - `input_event_sender`: The channel new InputDeviceBindings will send InputEvents to.
    /// - `bindings`: Holds all the InputDeviceBindings
    /// - `input_devices_node`: The parent node for all device bindings' inspect nodes.
    /// - `devices_discovered`: Inspect property to track discovered devices.
    /// - `devices_connected`: Inspect property to track connected devices.
    /// - `break_on_idle`: If true, stops watching for devices once all existing devices are handled.
    /// - `feature_flags`: The feature flags.
    /// - `metrics_logger`: The metrics logger.
    ///
    /// # Errors
    /// If the input report service or a connection within it fails.
    async fn watch_for_devices(
        service: fuchsia_component::client::Service<fidl_fuchsia_input_report::ServiceMarker>,
        device_types: Vec<input_device::InputDeviceType>,
        input_event_sender: UnboundedSender<Vec<input_device::InputEvent>>,
        bindings: InputDeviceBindingMap,
        input_devices_node: &fuchsia_inspect::Node,
        devices_discovered: &fuchsia_inspect::UintProperty,
        devices_connected: &fuchsia_inspect::UintProperty,
        break_on_idle: bool,
        feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
        device_listener_registry: DeviceListenerRegistry,
    ) -> Result<(), Error> {
        if break_on_idle {
            let instances = service.enumerate().await?;
            for instance in instances {
                Self::connect_and_bind_device(
                    &instance,
                    &device_types,
                    &input_event_sender,
                    &bindings,
                    input_devices_node,
                    devices_discovered,
                    devices_connected,
                    feature_flags.clone(),
                    metrics_logger.clone(),
                    device_listener_registry.clone(),
                )
                .await;
            }
            Ok(())
        } else {
            let mut instances = service
                .watch()
                .await
                .context("failed to watch input-report service")?
                .err_into::<Error>()
                .boxed();

            while let Some(instance) = instances.try_next().await? {
                Self::connect_and_bind_device(
                    &instance,
                    &device_types,
                    &input_event_sender,
                    &bindings,
                    input_devices_node,
                    devices_discovered,
                    devices_connected,
                    feature_flags.clone(),
                    metrics_logger.clone(),
                    device_listener_registry.clone(),
                )
                .await;
            }

            Err(format_err!("Input pipeline stopped watching for new input devices."))
        }
    }

    /// Handles the incoming InputDeviceRegistryRequestStream.
    ///
    /// This method will end when the request stream is closed. If the stream closes with an
    /// error the error will be returned in the Result.
    ///
    /// **NOTE**: Only one stream is handled at a time. https://fxbug.dev/42061078
    ///
    /// # Parameters
    /// - `stream`: The stream of InputDeviceRegistryRequests.
    /// - `device_types`: The types of devices to watch for.
    /// - `input_event_sender`: The channel new InputDeviceBindings will send InputEvents to.
    /// - `bindings`: Holds all the InputDeviceBindings associated with the InputPipeline.
    /// - `input_devices_node`: The parent node for all injected devices' inspect nodes.
    /// - `metrics_logger`: The metrics logger.
    pub async fn handle_input_device_registry_request_stream(
        mut stream: fidl_fuchsia_input_injection::InputDeviceRegistryRequestStream,
        device_types: &[input_device::InputDeviceType],
        input_event_sender: &UnboundedSender<Vec<input_device::InputEvent>>,
        bindings: &InputDeviceBindingMap,
        input_devices_node: &fuchsia_inspect::Node,
        feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
        device_listener_registry: DeviceListenerRegistry,
        task_sender: UnboundedSender<crate::dispatcher::TaskHandle<()>>,
    ) -> Result<(), Error> {
        while let Some(request) = stream
            .try_next()
            .await
            .context("Error handling input device registry request stream")?
        {
            match request {
                fidl_fuchsia_input_injection::InputDeviceRegistryRequest::Register {
                    device,
                    ..
                } => {
                    // Add a binding if the device is a type being tracked
                    let device = fidl_next::ClientEnd::<
                        fidl_next_fuchsia_input_report::InputDevice,
                        zx::Channel,
                    >::from_untyped(device.into_channel());
                    let device = Dispatcher::client_from_zx_channel(device);
                    let (device, join_handle) = device.spawn_full();
                    let device_id = get_next_device_id();

                    if let Some(descriptor) = add_device_bindings(
                        device_types,
                        &format!("input-device-registry-{}", device_id),
                        device,
                        input_event_sender,
                        bindings,
                        device_id,
                        input_devices_node,
                        None,
                        feature_flags.clone(),
                        metrics_logger.clone(),
                        true,
                    )
                    .await
                    {
                        device_listener_registry.notify_device_changed(
                            fidl_next_fuchsia_ui_input::Action::Added,
                            device_id,
                            descriptor.clone(),
                        );

                        let device_listener_registry_clone = device_listener_registry.clone();
                        let bindings_clone = bindings.clone();
                        let watch_task = Dispatcher::spawn_local(async move {
                            let _ = join_handle.await;
                            log::info!("Injected device {} disconnected", device_id);
                            device_listener_registry_clone.notify_device_changed(
                                fidl_next_fuchsia_ui_input::Action::Removed,
                                device_id,
                                descriptor,
                            );
                            bindings_clone.lock().remove(&device_id);
                        });
                        let _ = task_sender.unbounded_send(watch_task);
                    }
                }
                fidl_fuchsia_input_injection::InputDeviceRegistryRequest::RegisterAndGetDeviceInfo {
                    device,
                    responder,
                    .. } => {
                    // Add a binding if the device is a type being tracked
                    let device = fidl_next::ClientEnd::<
                        fidl_next_fuchsia_input_report::InputDevice,
                        zx::Channel,
                    >::from_untyped(device.into_channel());
                    let device = Dispatcher::client_from_zx_channel(device);
                    let (device, join_handle) = device.spawn_full();
                    let device_id = get_next_device_id();

                    let descriptor_opt = add_device_bindings(
                        device_types,
                        &format!("input-device-registry-{}", device_id),
                        device,
                        input_event_sender,
                        bindings,
                        device_id,
                        input_devices_node,
                        None,
                        feature_flags.clone(),
                        metrics_logger.clone(),
                        true,
                    )
                    .await;

                    if let Some(descriptor) = descriptor_opt {
                        device_listener_registry.notify_device_changed(
                            fidl_next_fuchsia_ui_input::Action::Added,
                            device_id,
                            descriptor.clone(),
                        );

                        let device_listener_registry_clone = device_listener_registry.clone();
                        let bindings_clone = bindings.clone();
                        let watch_task = Dispatcher::spawn_local(async move {
                            let _ = join_handle.await;
                            log::info!("Injected device {} disconnected", device_id);
                            device_listener_registry_clone.notify_device_changed(
                                fidl_next_fuchsia_ui_input::Action::Removed,
                                device_id,
                                descriptor.clone(),
                            );
                            bindings_clone.lock().remove(&device_id);
                        });
                        let _ = task_sender.unbounded_send(watch_task);
                    }

                    responder.send(fidl_fuchsia_input_injection::InputDeviceRegistryRegisterAndGetDeviceInfoResponse{
                        device_id: Some(device_id),
                        ..Default::default()
                    }).expect("Failed to respond to RegisterAndGetDeviceInfo request");
                }
            }
        }

        Ok(())
    }

    /// Initializes all handlers and starts the input pipeline loop in an asynchronous executor.
    fn run(
        mut receiver: UnboundedReceiver<Vec<input_device::InputEvent>>,
        handlers: Vec<Rc<dyn input_handler::BatchInputHandler>>,
        metrics_logger: metrics::MetricsLogger,
    ) -> LocalBoxFuture<'static, ()> {
        async move {
            for handler in &handlers {
                handler.clone().set_handler_healthy();
            }

            let mut handlers_by_type: [Vec<Rc<dyn input_handler::BatchInputHandler>>;
                InputEventType::COUNT] = Default::default();

            // TODO: b/478262850 - We can use supported_input_devices to populate this list.
            let event_types = vec![
                InputEventType::Keyboard,
                InputEventType::LightSensor,
                InputEventType::ConsumerControls,
                InputEventType::Mouse,
                InputEventType::TouchScreen,
                InputEventType::Touchpad,
                #[cfg(test)]
                InputEventType::Fake,
            ];

            for event_type in event_types {
                let handlers_for_type: Vec<Rc<dyn input_handler::BatchInputHandler>> = handlers
                    .iter()
                    .filter(|h| h.interest().contains(&event_type))
                    .cloned()
                    .collect();
                handlers_by_type[event_type as usize] = handlers_for_type;
            }

            while let Some(events) = receiver.next().await {
                if events.is_empty() {
                    continue;
                }

                let mut groups_seen = 0;
                let events = events.into_iter().chunk_by(|e| InputEventType::from(&e.device_event));
                let events = events.into_iter().map(|(k, v)| (k, v.collect::<Vec<_>>()));
                for (event_type, event_group) in events {
                    groups_seen += 1;
                    if groups_seen == 2 {
                        metrics_logger.log_error(
                                InputPipelineErrorMetricDimensionEvent::InputFrameContainsMultipleTypesOfEvents,
                                "it is not recommended to contain multiple types of events in 1 send".to_string(),
                            );
                    }
                    let mut events_in_group = event_group;

                    // Get pre-computed handlers for this event type.
                    let handlers = &handlers_by_type[event_type as usize];

                    for handler in handlers {
                        events_in_group =
                            handler.clone().handle_input_events(events_in_group).await;
                    }

                    for event in events_in_group {
                        if event.handled == input_device::Handled::No {
                            log::warn!("unhandled input event: {:?}", event);
                        }
                        if let Some(trace_id) = event.trace_id {
                            fuchsia_trace::flow_end!(
                                "input",
                                "event_in_input_pipeline",
                                trace_id.into()
                            );
                        }
                    }
                }
            }
            for handler in &handlers {
                handler.clone().set_handler_unhealthy("Pipeline loop terminated");
            }
            panic!("Runner task is not supposed to terminate.")
        }
        .boxed_local()
    }
}

/// Adds `InputDeviceBinding`s to `bindings` for all `device_types` exposed by `device_proxy`.
///
/// # Parameters
/// - `device_types`: The types of devices to watch for.
/// - `device_proxy`: A proxy to the input device.
/// - `input_event_sender`: The channel new InputDeviceBindings will send InputEvents to.
/// - `bindings`: Holds all the InputDeviceBindings associated with the InputPipeline.
/// - `device_id`: The device id of the associated bindings.
/// - `input_devices_node`: The parent node for all device bindings' inspect nodes.
///
/// # Note
/// This will create multiple bindings, in the case where
/// * `device_proxy().get_descriptor()` returns a `fidl_fuchsia_input_report::DeviceDescriptor`
///   with multiple table fields populated, and
/// * multiple populated table fields correspond to device types present in `device_types`
///
/// This is used, for example, to support the Atlas touchpad. In that case, a single
/// instance of `fuchsia.input.report.Service` provides both a `fuchsia.input.report.MouseDescriptor` and
/// a `fuchsia.input.report.TouchDescriptor`.
async fn add_device_bindings(
    device_types: &[input_device::InputDeviceType],
    instance_name: &str,
    device_proxy: fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, Transport>,
    input_event_sender: &UnboundedSender<Vec<input_device::InputEvent>>,
    bindings: &InputDeviceBindingMap,
    device_id: u32,
    input_devices_node: &fuchsia_inspect::Node,
    devices_connected: Option<&fuchsia_inspect::UintProperty>,
    feature_flags: InputPipelineFeatureFlags,
    metrics_logger: metrics::MetricsLogger,
    is_injected: bool,
) -> Option<fidl_next_fuchsia_input_report::DeviceDescriptor> {
    let mut matched_device_types = vec![];
    let descriptor = match device_proxy.get_descriptor().await {
        Ok(res) => {
            for device_type in device_types {
                if input_device::is_device_type(&res.descriptor, *device_type).await {
                    matched_device_types.push(device_type);
                    match devices_connected {
                        Some(dev_connected) => {
                            let _ = dev_connected.add(1);
                        }
                        None => (),
                    };
                }
            }
            if matched_device_types.is_empty() {
                log::info!(
                    "device {} did not match any supported device types: {:?}",
                    instance_name,
                    device_types
                );
                let device_node =
                    input_devices_node.create_child(format!("{}_Unsupported", instance_name));
                let mut health = fuchsia_inspect::health::Node::new(&device_node);
                health.set_unhealthy("Unsupported device type.");
                device_node.record(health);
                input_devices_node.record(device_node);
                return None;
            }
            res.descriptor
        }
        Err(_) => {
            metrics_logger.clone().log_error(
                InputPipelineErrorMetricDimensionEvent::InputPipelineNoDeviceDescriptor,
                std::format!("cannot bind device {} without a device descriptor", instance_name),
            );
            return None;
        }
    };

    log::info!(
        "binding {} to device types: {}",
        instance_name,
        matched_device_types
            .iter()
            .fold(String::new(), |device_types_string, device_type| device_types_string
                + &format!("{:?}, ", device_type))
    );

    let mut new_bindings: Vec<InputDeviceBindingAndTask> = vec![];
    for device_type in matched_device_types {
        // Clone `device_proxy`, so that multiple bindings (e.g. a `MouseBinding` and a
        // `TouchBinding`) can read data from the same `fuchsia.input.report.Service` instance.
        //
        // There's no conflict in having multiple bindings read from the same node,
        // since:
        // * each binding will create its own `fuchsia.input.report.InputReportsReader`, and
        // * the device driver will copy each incoming report to each connected reader.
        //
        // This does mean that reports from the Atlas touchpad device get read twice
        // (by a `MouseBinding` and a `TouchBinding`), regardless of whether the device
        // is operating in mouse mode or touchpad mode.
        //
        // This hasn't been an issue because:
        // * Semantically: things are fine, because each binding discards irrelevant reports.
        //   (E.g. `MouseBinding` discards anything that isn't a `MouseInputReport`), and
        // * Performance wise: things are fine, because the data rate of the touchpad is low
        //   (125 HZ).
        //
        // If we add additional cases where bindings share an underlying service instance,
        // we might consider adding a multiplexing binding, to avoid reading duplicate reports.
        let proxy = device_proxy.clone();
        let device_node =
            input_devices_node.create_child(format!("{}_{}", instance_name, device_type));
        match input_device::get_device_binding(
            *device_type,
            proxy,
            device_id,
            input_event_sender.clone(),
            device_node,
            feature_flags.clone(),
            metrics_logger.clone(),
            is_injected,
        )
        .await
        {
            Ok((binding, task)) => new_bindings.push(InputDeviceBindingAndTask { binding, task }),
            Err(e) => {
                metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::InputPipelineFailedToBind,
                    std::format!("failed to bind {} as {:?}: {}", instance_name, device_type, e),
                );
            }
        }
    }

    if !new_bindings.is_empty() {
        let mut bindings = bindings.lock();
        if let Some(v) = bindings.get_mut(&device_id) {
            v.extend(new_bindings);
        } else {
            bindings.insert(device_id, new_bindings);
        }
        Some(descriptor)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Transport;
    use fidl_next::Request;

    struct MockDeviceListener {
        event_sender:
            futures::channel::mpsc::UnboundedSender<fidl_next_fuchsia_ui_input::DeviceEvent>,
    }

    impl fidl_next_fuchsia_ui_input::DeviceListenerServerHandler<Transport> for MockDeviceListener {
        async fn on_device_changed(
            &mut self,
            request: Request<
                fidl_next_fuchsia_ui_input::device_listener::OnDeviceChanged,
                Transport,
            >,
        ) {
            let event = request.payload().event.clone();
            let _ = self.event_sender.unbounded_send(event);
        }
    }
    use crate::input_device::{InputDeviceBinding, InputEventType};
    use crate::utils::Position;
    use crate::{fake_input_device_binding, mouse_binding, observe_fake_events_input_handler};
    use async_trait::async_trait;
    use diagnostics_assertions::AnyProperty;
    use fidl::endpoints::{create_proxy_and_stream, create_request_stream};
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use futures::{FutureExt, StreamExt};
    use pretty_assertions::assert_eq;
    use rand::RngExt as _;
    use sorted_vec_map::SortedVecSet;
    use vfs::{pseudo_directory, service as pseudo_fs_service};

    /// Returns the InputEvent sent over `sender`.
    ///
    /// # Parameters
    /// - `sender`: The channel to send the InputEvent over.
    fn send_input_event(
        sender: UnboundedSender<Vec<input_device::InputEvent>>,
    ) -> Vec<input_device::InputEvent> {
        let mut rng = rand::rng();
        let offset =
            Position { x: rng.random_range(0..10) as f32, y: rng.random_range(0..10) as f32 };
        let input_event = input_device::InputEvent {
            device_event: input_device::InputDeviceEvent::Mouse(mouse_binding::MouseEvent::new(
                mouse_binding::MouseLocation::Relative(mouse_binding::RelativeLocation {
                    counts: Position { x: offset.x, y: offset.y },
                }),
                None, /* wheel_delta_v */
                None, /* wheel_delta_h */
                mouse_binding::MousePhase::Move,
                SortedVecSet::new(),
                SortedVecSet::new(),
                None, /* is_precision_scroll */
                None, /* wake_lease */
            )),
            device_descriptor: input_device::InputDeviceDescriptor::Mouse(
                mouse_binding::MouseDeviceDescriptor {
                    device_id: 1,
                    absolute_x_range: None,
                    absolute_y_range: None,
                    wheel_v_range: None,
                    wheel_h_range: None,
                    buttons: None,
                },
            ),
            event_time: zx::MonotonicInstant::get(),
            handled: input_device::Handled::No,
            trace_id: None,
        };
        match sender.unbounded_send(vec![input_event.clone()]) {
            Err(_) => assert!(false),
            _ => {}
        }

        vec![input_event]
    }

    /// Returns a MouseDescriptor on an InputDeviceRequest.
    ///
    /// # Parameters
    /// - `input_device_request`: The request to handle.
    fn handle_input_device_request(
        input_device_request: fidl_fuchsia_input_report::InputDeviceRequest,
    ) {
        match input_device_request {
            fidl_fuchsia_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                let _ = responder.send(&fidl_fuchsia_input_report::DeviceDescriptor {
                    device_information: None,
                    mouse: Some(fidl_fuchsia_input_report::MouseDescriptor {
                        input: Some(fidl_fuchsia_input_report::MouseInputDescriptor {
                            movement_x: None,
                            movement_y: None,
                            scroll_v: None,
                            scroll_h: None,
                            buttons: Some(vec![0]),
                            position_x: None,
                            position_y: None,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    sensor: None,
                    touch: None,
                    keyboard: None,
                    consumer_control: None,
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    /// Tests that an input pipeline handles events from multiple devices.
    #[fuchsia::test]
    async fn multiple_devices_single_handler() {
        // Create two fake device bindings.
        let (device_event_sender, device_event_receiver) = futures::channel::mpsc::unbounded();
        let first_device_binding =
            fake_input_device_binding::FakeInputDeviceBinding::new(device_event_sender.clone());
        let second_device_binding =
            fake_input_device_binding::FakeInputDeviceBinding::new(device_event_sender.clone());

        // Create a fake input handler.
        let (handler_event_sender, mut handler_event_receiver) =
            futures::channel::mpsc::channel(100);
        let input_handler = observe_fake_events_input_handler::ObserveFakeEventsInputHandler::new(
            handler_event_sender,
        );

        // Build the input pipeline.
        let (sender, receiver, handlers, _, _, _) =
            InputPipelineAssembly::new(metrics::MetricsLogger::default())
                .add_handler(input_handler)
                .into_components();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");
        let runner_fut =
            Some(InputPipeline::run(receiver, handlers, metrics::MetricsLogger::default()));
        let input_pipeline = InputPipeline {
            pipeline_sender: sender,
            device_event_sender,
            device_event_receiver,
            input_device_types: vec![],
            input_device_bindings: Arc::new(Mutex::new(SortedVecMap::new())),
            inspect_node: test_node,
            metrics_logger: metrics::MetricsLogger::default(),
            feature_flags: input_device::InputPipelineFeatureFlags::default(),
            display_ownership_fut: None,
            focus_listener_fut: None,
            watcher_fut: None,
            runner_fut,
            device_listener_registry: DeviceListenerRegistry::new(),
        };

        // Send an input event from each device.
        let first_device_events = send_input_event(first_device_binding.input_event_sender());
        let second_device_events = send_input_event(second_device_binding.input_event_sender());

        // Run the pipeline.
        let _pipeline_task = fasync::Task::local(input_pipeline.handle_input_events());

        // Assert the handler receives the events.
        let first_handled_event = handler_event_receiver.next().await;
        assert_eq!(first_handled_event, first_device_events.into_iter().next());

        let second_handled_event = handler_event_receiver.next().await;
        assert_eq!(second_handled_event, second_device_events.into_iter().next());
    }

    /// Tests that an input pipeline handles events through multiple input handlers.
    #[fuchsia::test]
    async fn single_device_multiple_handlers() {
        // Create two fake device bindings.
        let (device_event_sender, device_event_receiver) = futures::channel::mpsc::unbounded();
        let input_device_binding =
            fake_input_device_binding::FakeInputDeviceBinding::new(device_event_sender.clone());

        // Create two fake input handlers.
        let (first_handler_event_sender, mut first_handler_event_receiver) =
            futures::channel::mpsc::channel(100);
        let first_input_handler =
            observe_fake_events_input_handler::ObserveFakeEventsInputHandler::new(
                first_handler_event_sender,
            );
        let (second_handler_event_sender, mut second_handler_event_receiver) =
            futures::channel::mpsc::channel(100);
        let second_input_handler =
            observe_fake_events_input_handler::ObserveFakeEventsInputHandler::new(
                second_handler_event_sender,
            );

        // Build the input pipeline.
        let (sender, receiver, handlers, _, _, _) =
            InputPipelineAssembly::new(metrics::MetricsLogger::default())
                .add_handler(first_input_handler)
                .add_handler(second_input_handler)
                .into_components();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");
        let runner_fut =
            Some(InputPipeline::run(receiver, handlers, metrics::MetricsLogger::default()));
        let input_pipeline = InputPipeline {
            pipeline_sender: sender,
            device_event_sender,
            device_event_receiver,
            input_device_types: vec![],
            input_device_bindings: Arc::new(Mutex::new(SortedVecMap::new())),
            inspect_node: test_node,
            metrics_logger: metrics::MetricsLogger::default(),
            feature_flags: input_device::InputPipelineFeatureFlags::default(),
            display_ownership_fut: None,
            focus_listener_fut: None,
            watcher_fut: None,
            runner_fut,
            device_listener_registry: DeviceListenerRegistry::new(),
        };

        // Send an input event.
        let input_events = send_input_event(input_device_binding.input_event_sender());

        // Run the pipeline.
        let _pipeline_task = fasync::Task::local(input_pipeline.handle_input_events());

        // Assert both handlers receive the event.
        let expected_event = input_events.into_iter().next();
        let first_handler_event = first_handler_event_receiver.next().await;
        assert_eq!(first_handler_event, expected_event);
        let second_handler_event = second_handler_event_receiver.next().await;
        assert_eq!(second_handler_event, expected_event);
    }

    /// Tests that a single mouse device binding is created for the one input device in the
    /// input report service directory.
    #[fuchsia::test]
    async fn watch_devices_one_match_exists() {
        let dir = pseudo_directory! {
            "fuchsia.input.report.Service" => pseudo_directory! {
                "instance_0" => pseudo_directory! {
                    "input_device" => pseudo_fs_service::host(
                        move |mut request_stream: fidl_fuchsia_input_report::InputDeviceRequestStream| {
                            async move {
                                while let Some(input_device_request) =
                                    request_stream.try_next().await.unwrap()
                                {
                                    handle_input_device_request(input_device_request);
                                }

                            }.boxed()
                        },
                    )
                }
            }
        };

        // Get a proxy to the pseudo directory for the input pipeline. The input pipeline uses this
        // proxy to get connections to input devices.
        let svc_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());

        let (input_event_sender, _input_event_receiver) = futures::channel::mpsc::unbounded();
        let bindings: InputDeviceBindingMap = Arc::new(Mutex::new(SortedVecMap::new()));
        let supported_device_types = vec![input_device::InputDeviceType::Mouse];

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");
        test_node.record_string(
            "supported_input_devices",
            supported_device_types.clone().iter().join(", "),
        );
        let input_devices = test_node.create_child("input_devices");
        // Assert that inspect tree is initialized with no devices.
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                supported_input_devices: "Mouse",
                input_devices: {}
            }
        });

        let devices_discovered = input_devices.create_uint("devices_discovered", 0);
        let devices_connected = input_devices.create_uint("devices_connected", 0);

        let dir = fuchsia_fs::directory::open_directory_async(
            &svc_proxy,
            "fuchsia.input.report.Service",
            fio::PERM_READABLE,
        )
        .expect("open service directory");
        let service = fuchsia_component::client::Service::from_service_dir_proxy(
            dir,
            fidl_fuchsia_input_report::ServiceMarker,
        );

        let _ = InputPipeline::watch_for_devices(
            service,
            supported_device_types,
            input_event_sender,
            bindings.clone(),
            &input_devices,
            &devices_discovered,
            &devices_connected,
            true, /* break_on_idle */
            InputPipelineFeatureFlags { enable_merge_touch_events: false },
            metrics::MetricsLogger::default(),
            DeviceListenerRegistry::new(),
        )
        .await;

        // Assert that one mouse device with accurate device id was found.
        {
            let bindings_map = bindings.lock();
            assert_eq!(bindings_map.len(), 1);
            let bindings_vector = bindings_map.get(&10);
            assert!(bindings_vector.is_some());
            assert_eq!(bindings_vector.unwrap().len(), 1);
            let boxed_mouse_binding = bindings_vector.unwrap().get(0);
            assert!(boxed_mouse_binding.is_some());
            assert_eq!(
                boxed_mouse_binding.unwrap().binding.get_device_descriptor(),
                input_device::InputDeviceDescriptor::Mouse(mouse_binding::MouseDeviceDescriptor {
                    device_id: 10,
                    absolute_x_range: None,
                    absolute_y_range: None,
                    wheel_v_range: None,
                    wheel_h_range: None,
                    buttons: Some(vec![0]),
                })
            );
        }

        // Assert that inspect tree reflects new device discovered and connected.
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                supported_input_devices: "Mouse",
                input_devices: {
                    devices_discovered: 1u64,
                    devices_connected: 1u64,
                    "instance_0_Mouse": contains {
                        reports_received_count: 0u64,
                        reports_filtered_count: 0u64,
                        events_generated: 0u64,
                        last_received_timestamp_ns: 0u64,
                        last_generated_timestamp_ns: 0u64,
                        "fuchsia.inspect.Health": {
                            status: "OK",
                            // Timestamp value is unpredictable and not relevant in this context,
                            // so we only assert that the property is present.
                            start_timestamp_nanos: AnyProperty
                        },
                    }
                }
            }
        });
    }

    /// Tests that no device bindings are created because the input pipeline looks for keyboard devices
    /// but only a mouse exists.
    #[fuchsia::test]
    async fn watch_devices_no_matches_exist() {
        let dir = pseudo_directory! {
            "fuchsia.input.report.Service" => pseudo_directory! {
                "instance_0" => pseudo_directory! {
                    "input_device" => pseudo_fs_service::host(
                        move |mut request_stream: fidl_fuchsia_input_report::InputDeviceRequestStream| {
                            async move {
                                while let Some(input_device_request) =
                                    request_stream.try_next().await.unwrap()
                                {
                                    handle_input_device_request(input_device_request);
                                }

                            }.boxed()
                        },
                    )
                }
            }
        };

        // Get a proxy to the pseudo directory for the input pipeline. The input pipeline uses this
        // proxy to get connections to input devices.
        let svc_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());

        let (input_event_sender, _input_event_receiver) = futures::channel::mpsc::unbounded();
        let bindings: InputDeviceBindingMap = Arc::new(Mutex::new(SortedVecMap::new()));
        let supported_device_types = vec![input_device::InputDeviceType::Keyboard];

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");
        test_node.record_string(
            "supported_input_devices",
            supported_device_types.clone().iter().join(", "),
        );
        let input_devices = test_node.create_child("input_devices");
        // Assert that inspect tree is initialized with no devices.
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                supported_input_devices: "Keyboard",
                input_devices: {}
            }
        });

        let devices_discovered = input_devices.create_uint("devices_discovered", 0);
        let devices_connected = input_devices.create_uint("devices_connected", 0);

        let dir = fuchsia_fs::directory::open_directory_async(
            &svc_proxy,
            "fuchsia.input.report.Service",
            fio::PERM_READABLE,
        )
        .expect("open service directory");
        let service = fuchsia_component::client::Service::from_service_dir_proxy(
            dir,
            fidl_fuchsia_input_report::ServiceMarker,
        );

        let _ = InputPipeline::watch_for_devices(
            service,
            supported_device_types,
            input_event_sender,
            bindings.clone(),
            &input_devices,
            &devices_discovered,
            &devices_connected,
            true, /* break_on_idle */
            InputPipelineFeatureFlags { enable_merge_touch_events: false },
            metrics::MetricsLogger::default(),
            DeviceListenerRegistry::new(),
        )
        .await;

        // Assert that no devices were found.
        {
            let bindings = bindings.lock();
            assert_eq!(bindings.len(), 0);
        }

        // Assert that inspect tree reflects new device discovered, but not connected.
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                supported_input_devices: "Keyboard",
                input_devices: {
                    devices_discovered: 1u64,
                    devices_connected: 0u64,
                    "instance_0_Unsupported": {
                        "fuchsia.inspect.Health": {
                            status: "UNHEALTHY",
                            message: "Unsupported device type.",
                            // Timestamp value is unpredictable and not relevant in this context,
                            // so we only assert that the property is present.
                            start_timestamp_nanos: AnyProperty
                        },
                    }
                }
            }
        });
    }

    /// Tests that a single keyboard device binding is created for the input device registered
    /// through InputDeviceRegistry.
    #[fuchsia::test]
    async fn handle_input_device_registry_request_stream() {
        let (input_device_registry_proxy, input_device_registry_request_stream) =
            create_proxy_and_stream::<fidl_fuchsia_input_injection::InputDeviceRegistryMarker>();
        let (input_device_client_end, mut input_device_request_stream) =
            create_request_stream::<fidl_fuchsia_input_report::InputDeviceMarker>();

        let device_types = vec![input_device::InputDeviceType::Mouse];
        let (input_event_sender, _input_event_receiver) = futures::channel::mpsc::unbounded();
        let bindings: InputDeviceBindingMap = Arc::new(Mutex::new(SortedVecMap::new()));

        let device_listener_registry = DeviceListenerRegistry::new();
        let (listener_client, listener_server) =
            fidl_next::fuchsia::create_channel::<fidl_next_fuchsia_ui_input::DeviceListener>();
        let listener_client = listener_client.spawn();
        let _existing_devices = device_listener_registry.add_listener(listener_client);

        let (event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();
        let _server_task = listener_server.spawn(MockDeviceListener { event_sender });

        // Handle input device requests.
        let _task = fasync::Task::local(async move {
            // Register a device.
            let _ = input_device_registry_proxy.register(input_device_client_end);

            let mut count = 0;
            while count < 2 {
                if let Some(input_device_request) =
                    input_device_request_stream.try_next().await.unwrap()
                {
                    handle_input_device_request(input_device_request);
                    count += 1;
                } else {
                    break;
                }
            }

            // End handle_input_device_registry_request_stream() by taking the event stream.
            input_device_registry_proxy.take_event_stream();
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");

        // Start listening for InputDeviceRegistryRequests.
        let bindings_clone = bindings.clone();
        let device_listener_registry_clone = device_listener_registry.clone();
        let (task_sender, mut task_receiver) = futures::channel::mpsc::unbounded();
        let mut tasks = futures::stream::FuturesUnordered::new();
        let registry_fut = InputPipeline::handle_input_device_registry_request_stream(
            input_device_registry_request_stream,
            &device_types,
            &input_event_sender,
            &bindings_clone,
            &test_node,
            InputPipelineFeatureFlags { enable_merge_touch_events: false },
            metrics::MetricsLogger::default(),
            device_listener_registry_clone,
            task_sender,
        )
        .fuse();

        let mut registry_fut = std::pin::pin!(registry_fut);
        loop {
            futures::select! {
                res = registry_fut => {
                    res.unwrap();
                    break;
                }
                task = task_receiver.next() => {
                    if let Some(task) = task {
                        tasks.push(task);
                    }
                }
                _ = tasks.select_next_some() => {}
            }
        }

        // Assert that a device was registered.
        let device_id = {
            let bindings = bindings.lock();
            assert_eq!(bindings.len(), 1);
            *bindings.keys().next().unwrap()
        };

        // Assert that device listener was notified.
        if let Some(event) = event_receiver.next().await {
            assert_eq!(event.action, Some(fidl_next_fuchsia_ui_input::Action::Added));
            assert_eq!(event.device_id, Some(device_id));
        } else {
            panic!("Expected a request on listener stream");
        }
    }

    /// Tests that an injected device is removed and listeners are notified with Action::Removed
    /// when the injected device channel is closed.
    #[fasync::run_singlethreaded(test)]
    async fn handle_input_device_registry_disconnection() {
        let (input_device_registry_proxy, input_device_registry_request_stream) =
            create_proxy_and_stream::<fidl_fuchsia_input_injection::InputDeviceRegistryMarker>();
        let (input_device_client_end, mut input_device_request_stream) =
            create_request_stream::<fidl_fuchsia_input_report::InputDeviceMarker>();

        let device_types = vec![input_device::InputDeviceType::Mouse];
        let (input_event_sender, _input_event_receiver) = futures::channel::mpsc::unbounded();
        let bindings: InputDeviceBindingMap = Arc::new(Mutex::new(SortedVecMap::new()));

        let device_listener_registry = DeviceListenerRegistry::new();
        let (listener_client, listener_server) =
            fidl_next::fuchsia::create_channel::<fidl_next_fuchsia_ui_input::DeviceListener>();
        let listener_client = listener_client.spawn();
        let _existing_devices = device_listener_registry.add_listener(listener_client);

        let (event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();
        let _server_task = listener_server.spawn(MockDeviceListener { event_sender });

        // Handle input device requests.
        let _task = fasync::Task::local(async move {
            // Register a device.
            let _ = input_device_registry_proxy.register(input_device_client_end);

            let mut count = 0;
            while count < 2 {
                if let Some(input_device_request) =
                    input_device_request_stream.try_next().await.unwrap()
                {
                    handle_input_device_request(input_device_request);
                    count += 1;
                } else {
                    break;
                }
            }

            // Close the device channel by dropping the request stream.
            std::mem::drop(input_device_request_stream);

            // Wait a bit to let the local task run.
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100),
            ))
            .await;

            // End handle_input_device_registry_request_stream() by taking the event stream.
            input_device_registry_proxy.take_event_stream();
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");

        // Start listening for InputDeviceRegistryRequests.
        let bindings_clone = bindings.clone();
        let device_listener_registry_clone = device_listener_registry.clone();
        let (task_sender, mut task_receiver) = futures::channel::mpsc::unbounded();
        let mut tasks = futures::stream::FuturesUnordered::new();
        let registry_fut = InputPipeline::handle_input_device_registry_request_stream(
            input_device_registry_request_stream,
            &device_types,
            &input_event_sender,
            &bindings_clone,
            &test_node,
            InputPipelineFeatureFlags { enable_merge_touch_events: false },
            metrics::MetricsLogger::default(),
            device_listener_registry_clone,
            task_sender,
        )
        .fuse();

        let mut registry_fut = std::pin::pin!(registry_fut);
        loop {
            futures::select! {
                res = registry_fut => {
                    res.unwrap();
                    break;
                }
                task = task_receiver.next() => {
                    if let Some(task) = task {
                        tasks.push(task);
                    }
                }
                _ = tasks.select_next_some() => {}
            }
        }
        while let Some(_) = tasks.next().await {}

        // Assert that the device was registered and then removed.
        {
            let bindings = bindings.lock();
            assert_eq!(bindings.len(), 0);
        }

        // Assert that device listener was notified of Addition.
        let event_added = if let Some(event) = event_receiver.next().await {
            event
        } else {
            panic!("Expected Added event");
        };
        assert_eq!(event_added.action, Some(fidl_next_fuchsia_ui_input::Action::Added));

        // Assert that device listener was notified of Removal.
        let event_removed = if let Some(event) = event_receiver.next().await {
            event
        } else {
            panic!("Expected Removed event");
        };
        assert_eq!(event_removed.action, Some(fidl_next_fuchsia_ui_input::Action::Removed));
    }

    // Tests that correct properties are added to inspect node when InputPipeline is created.
    #[fuchsia::test]
    async fn check_inspect_node_has_correct_properties() {
        let device_types = vec![
            input_device::InputDeviceType::Touch,
            input_device::InputDeviceType::ConsumerControls,
        ];
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("input_pipeline");
        // Create fake input handler for assembly
        let (fake_handler_event_sender, _fake_handler_event_receiver) =
            futures::channel::mpsc::channel(100);
        let fake_input_handler =
            observe_fake_events_input_handler::ObserveFakeEventsInputHandler::new(
                fake_handler_event_sender,
            );
        let assembly = InputPipelineAssembly::new(metrics::MetricsLogger::default())
            .add_handler(fake_input_handler);
        let _test_input_pipeline = InputPipeline::new(
            &Incoming::new(),
            device_types,
            assembly,
            test_node,
            InputPipelineFeatureFlags { enable_merge_touch_events: false },
            metrics::MetricsLogger::default(),
        );
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                supported_input_devices: "Touch, ConsumerControls",
                handlers_registered: 1u64,
                handlers_healthy: 1u64,
                input_devices: {}
            }
        });
    }

    struct SpecificInterestFakeHandler {
        interest_types: Vec<input_device::InputEventType>,
        event_sender: std::cell::RefCell<futures::channel::mpsc::Sender<input_device::InputEvent>>,
    }

    impl SpecificInterestFakeHandler {
        pub fn new(
            interest_types: Vec<input_device::InputEventType>,
            event_sender: futures::channel::mpsc::Sender<input_device::InputEvent>,
        ) -> Rc<Self> {
            Rc::new(SpecificInterestFakeHandler {
                interest_types,
                event_sender: std::cell::RefCell::new(event_sender),
            })
        }
    }

    impl Handler for SpecificInterestFakeHandler {
        fn set_handler_healthy(self: std::rc::Rc<Self>) {}
        fn set_handler_unhealthy(self: std::rc::Rc<Self>, _msg: &str) {}
        fn get_name(&self) -> &'static str {
            "SpecificInterestFakeHandler"
        }

        fn interest(&self) -> Vec<input_device::InputEventType> {
            self.interest_types.clone()
        }
    }

    #[async_trait(?Send)]
    impl input_handler::InputHandler for SpecificInterestFakeHandler {
        async fn handle_input_event(
            self: Rc<Self>,
            input_event: input_device::InputEvent,
        ) -> Vec<input_device::InputEvent> {
            match self.event_sender.borrow_mut().try_send(input_event.clone()) {
                Err(e) => panic!("SpecificInterestFakeHandler failed to send event: {:?}", e),
                Ok(_) => {}
            }
            vec![input_event]
        }
    }

    #[fuchsia::test]
    async fn run_only_sends_events_to_interested_handlers() {
        // Mouse Handler (Specific Interest: Mouse)
        let (mouse_sender, mut mouse_receiver) = futures::channel::mpsc::channel(1);
        let mouse_handler =
            SpecificInterestFakeHandler::new(vec![InputEventType::Mouse], mouse_sender);

        // Fake Handler (Specific Interest: Fake)
        let (fake_sender, mut fake_receiver) = futures::channel::mpsc::channel(1);
        let fake_handler =
            SpecificInterestFakeHandler::new(vec![InputEventType::Fake], fake_sender);

        let (pipeline_sender, pipeline_receiver, handlers, _, _, _) =
            InputPipelineAssembly::new(metrics::MetricsLogger::default())
                .add_handler(mouse_handler)
                .add_handler(fake_handler)
                .into_components();

        // Run the pipeline logic
        let _runner_task = fasync::Task::local(InputPipeline::run(
            pipeline_receiver,
            handlers,
            metrics::MetricsLogger::default(),
        ));

        // Create a Fake event
        let fake_event = input_device::InputEvent {
            device_event: input_device::InputDeviceEvent::Fake,
            device_descriptor: input_device::InputDeviceDescriptor::Fake,
            event_time: zx::MonotonicInstant::get(),
            handled: input_device::Handled::No,
            trace_id: None,
        };

        // Send the Fake event
        pipeline_sender.unbounded_send(vec![fake_event.clone()]).expect("failed to send event");

        // Verify Fake Handler received it
        let received_by_fake = fake_receiver.next().await;
        assert_eq!(received_by_fake, Some(fake_event));

        // Verify Mouse Handler did NOT receive it
        assert!(mouse_receiver.try_next().is_err());
    }

    fn create_mouse_event(x: f32, y: f32) -> input_device::InputEvent {
        input_device::InputEvent {
            device_event: input_device::InputDeviceEvent::Mouse(mouse_binding::MouseEvent::new(
                mouse_binding::MouseLocation::Relative(mouse_binding::RelativeLocation {
                    counts: Position { x, y },
                }),
                None,
                None,
                mouse_binding::MousePhase::Move,
                SortedVecSet::new(),
                SortedVecSet::new(),
                None,
                None,
            )),
            device_descriptor: input_device::InputDeviceDescriptor::Mouse(
                mouse_binding::MouseDeviceDescriptor {
                    device_id: 1,
                    absolute_x_range: None,
                    absolute_y_range: None,
                    wheel_v_range: None,
                    wheel_h_range: None,
                    buttons: None,
                },
            ),
            event_time: zx::MonotonicInstant::get(),
            handled: input_device::Handled::No,
            trace_id: None,
        }
    }

    #[fuchsia::test]
    async fn run_mixed_event_types_dispatched_correctly() {
        // Mouse Handler (Specific Interest: Mouse)
        let (mouse_sender, mut mouse_receiver) = futures::channel::mpsc::channel(10);
        let mouse_handler =
            SpecificInterestFakeHandler::new(vec![InputEventType::Mouse], mouse_sender);

        // Fake Handler (Specific Interest: Fake)
        let (fake_sender, mut fake_receiver) = futures::channel::mpsc::channel(10);
        let fake_handler =
            SpecificInterestFakeHandler::new(vec![InputEventType::Fake], fake_sender);

        let (pipeline_sender, pipeline_receiver, handlers, _, _, _) =
            InputPipelineAssembly::new(metrics::MetricsLogger::default())
                .add_handler(mouse_handler)
                .add_handler(fake_handler)
                .into_components();

        // Run the pipeline logic
        let _runner_task = fasync::Task::local(InputPipeline::run(
            pipeline_receiver,
            handlers,
            metrics::MetricsLogger::default(),
        ));

        // Create events
        let mouse_event_1 = create_mouse_event(1.0, 1.0);
        let mouse_event_2 = create_mouse_event(2.0, 2.0);
        let mouse_event_3 = create_mouse_event(3.0, 3.0);

        let fake_event_1 = input_device::InputEvent {
            device_event: input_device::InputDeviceEvent::Fake,
            device_descriptor: input_device::InputDeviceDescriptor::Fake,
            event_time: zx::MonotonicInstant::get(),
            handled: input_device::Handled::No,
            trace_id: None,
        };

        // Send mixed batch: [Mouse, Mouse, Fake, Mouse]
        // This should result in 3 chunks: [Mouse, Mouse], [Fake], [Mouse]
        let mixed_batch = vec![
            mouse_event_1.clone(),
            mouse_event_2.clone(),
            fake_event_1.clone(),
            mouse_event_3.clone(),
        ];
        pipeline_sender.unbounded_send(mixed_batch).expect("failed to send events");

        // Verify Mouse Handler received M1, M2, and then M3
        assert_eq!(mouse_receiver.next().await, Some(mouse_event_1));
        assert_eq!(mouse_receiver.next().await, Some(mouse_event_2));
        assert_eq!(mouse_receiver.next().await, Some(mouse_event_3));

        // Verify Fake Handler received F1
        assert_eq!(fake_receiver.next().await, Some(fake_event_1));
    }
}
