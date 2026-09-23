// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::app::strategies::base::AppStrategy;
use crate::app::{Config, InternalSender, MessageInternal};
use crate::drawing::DisplayRotation;
use crate::geometry::IntSize;
use crate::input::report::InputReportHandler;
use crate::input::{self, DeviceId, listen_for_user_input};
use crate::view::ViewKey;
use crate::view::strategies::base::{DisplayDirectParams, ViewStrategyParams, ViewStrategyPtr};
use crate::view::strategies::display_direct::DisplayDirectViewStrategy;
use anyhow::{Context, Error, anyhow, bail};
use async_trait::async_trait;
use euclid::size2;
use fidl::endpoints::{self};
use fidl_fuchsia_hardware_display::{
    ClientPriorityValue, CoordinatorListenerMarker, CoordinatorListenerRequest, CoordinatorMarker,
    CoordinatorProxy, ProviderProxy, ServiceMarker as DisplayServiceMarker,
};
use fidl_fuchsia_input_report as fidl_input_report;
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt};
use fuchsia_component::client::Service;
use futures::channel::mpsc::UnboundedSender;
use futures::{StreamExt, TryFutureExt};
use keymaps::Keymap;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;
use zx::Status;

pub(crate) struct AutoRepeatContext {
    app_sender: UnboundedSender<MessageInternal>,
    #[allow(unused)]
    keyboard_autorepeat_task: Option<fasync::Task<()>>,
    repeat_interval: std::time::Duration,
}

pub(crate) trait AutoRepeatTimer {
    fn schedule_autorepeat_timer(&mut self, device_id: &DeviceId);
    fn continue_autorepeat_timer(&mut self, device_id: &DeviceId);
    fn cancel_autorepeat_timer(&mut self) {}
}

impl AutoRepeatContext {
    pub(crate) fn new(app_sender: &UnboundedSender<MessageInternal>) -> Self {
        Self {
            app_sender: app_sender.clone(),
            keyboard_autorepeat_task: None,
            repeat_interval: Config::get().keyboard_autorepeat_slow_interval,
        }
    }

    fn schedule(&mut self, device_id: &DeviceId) {
        let timer =
            fasync::Timer::new(fuchsia_async::MonotonicInstant::after(self.repeat_interval.into()));
        let app_sender = self.app_sender.clone();
        let device_id = device_id.clone();
        let task = fasync::Task::local(async move {
            timer.await;
            app_sender
                .unbounded_send(MessageInternal::KeyboardAutoRepeat(device_id))
                .expect("unbounded_send");
        });
        self.keyboard_autorepeat_task = Some(task);
    }
}

impl AutoRepeatTimer for AutoRepeatContext {
    fn schedule_autorepeat_timer(&mut self, device_id: &DeviceId) {
        self.repeat_interval = Config::get().keyboard_autorepeat_slow_interval;
        self.schedule(device_id);
    }

    fn continue_autorepeat_timer(&mut self, device_id: &DeviceId) {
        self.repeat_interval =
            (self.repeat_interval * 3 / 4).max(Config::get().keyboard_autorepeat_fast_interval);
        self.schedule(device_id);
    }

    fn cancel_autorepeat_timer(&mut self) {
        self.keyboard_autorepeat_task = None;
    }
}

pub type CoordinatorProxyPtr = Rc<CoordinatorProxy>;

/// Connects to the coordinator Provider provided by the
/// `fuchsia.hardware.display.Service`. Watches for any available service
/// instance for up to `timeout` (or forever if `timeout` is None).
pub async fn connect_to_display_provider(
    timeout: Option<zx::MonotonicDuration>,
) -> Result<ProviderProxy, Error> {
    let deadline = match timeout {
        Some(timeout) => timeout.after_now(),
        None => zx::MonotonicInstant::INFINITE.into(),
    };
    Service::open(DisplayServiceMarker)
        .context("failed to open service")?
        .watch_for_any()
        .on_timeout(deadline, || Err(anyhow!("timed out watching for display service")))
        .await
        .context("failed to watch for display device")?
        .connect_to_provider()
        .context("failed to connect to provider")
}

pub struct DisplayCoordinator {
    pub coordinator: CoordinatorProxyPtr,
}

impl DisplayCoordinator {
    pub(crate) async fn open(
        provider: ProviderProxy,
        client_priority: Option<ClientPriorityValue>,
        app_sender: &UnboundedSender<MessageInternal>,
    ) -> Result<Self, Error> {
        let (coordinator, coordinator_server) = endpoints::create_proxy::<CoordinatorMarker>();
        let (listener_client, mut listener_requests) =
            endpoints::create_request_stream::<CoordinatorListenerMarker>();
        let () = {
            let priority = fidl_fuchsia_hardware_display::ClientPriority {
                value: client_priority
                    .unwrap_or(fidl_fuchsia_hardware_display::COMPOSITOR_CLIENT_PRIORITY_VALUE),
            };
            provider
                .open_coordinator(fidl_fuchsia_hardware_display::ProviderOpenCoordinatorRequest {
                    coordinator: Some(coordinator_server),
                    coordinator_listener: Some(listener_client),
                    priority: Some(priority),
                    __source_breaking: fidl::marker::SourceBreaking,
                })
                .await
        }
        .context("failed to perform FIDL call")?
        .map_err(Status::err_from_raw)
        .context("failed to open display coordinator")?;

        let app_sender = app_sender.clone();
        let f = async move {
            loop {
                if let Some(listener_request) = listener_requests.next().await {
                    if let Ok(listener_request) = listener_request {
                        app_sender
                            .unbounded_send(MessageInternal::DisplayCoordinatorListenerRequest(
                                listener_request,
                            ))
                            .expect("unbounded_send");
                    }
                }
            }
        };
        fasync::Task::local(f).detach();

        Ok(Self { coordinator: Rc::new(coordinator) })
    }

    pub(crate) async fn watch_displays(app_sender: UnboundedSender<MessageInternal>) {
        let provider = Service::open(DisplayServiceMarker)
            .expect("open service")
            .watch_for_any()
            .await
            .expect("watch for any service instance")
            .connect_to_provider()
            .expect("connect to provider");
        app_sender
            .unbounded_send(MessageInternal::NewDisplayCoordinator(provider))
            .expect("unbounded_send");
    }
}

pub type DisplayId = display_utils::DisplayId;

#[derive(Debug)]
pub struct DisplayInfo {
    preferred_size: IntSize,
    info: fidl_fuchsia_hardware_display::Info,
}

pub(crate) struct DisplayDirectAppStrategy<'a> {
    pub display_coordinator: Option<DisplayCoordinator>,
    pub display_rotation: DisplayRotation,
    pub keymap: &'a Keymap<'a>,
    pub input_report_handlers: HashMap<DeviceId, InputReportHandler<'a>>,
    pub context: AutoRepeatContext,
    pub app_sender: UnboundedSender<MessageInternal>,
    pub owned: bool,
    pub primary_display: Option<DisplayInfo>,
    views: HashMap<DisplayId, Vec<ViewKey>>,
}

impl<'a> DisplayDirectAppStrategy<'a> {
    pub fn new(
        display_coordinator: Option<DisplayCoordinator>,
        keymap: &'a Keymap<'a>,
        app_sender: UnboundedSender<MessageInternal>,
        app_config: &Config,
    ) -> DisplayDirectAppStrategy<'a> {
        DisplayDirectAppStrategy {
            display_coordinator: display_coordinator,
            display_rotation: app_config.display_rotation,
            keymap,
            input_report_handlers: HashMap::new(),
            context: AutoRepeatContext::new(&app_sender),
            app_sender,
            owned: false,
            primary_display: None,
            views: Default::default(),
        }
    }

    async fn handle_displays_changed(
        &mut self,
        added: Vec<fidl_fuchsia_hardware_display::Info>,
        removed: Vec<fidl_fuchsia_hardware_display_types::DisplayId>,
    ) -> Result<(), Error> {
        let display_coordinator = self.display_coordinator.as_ref().expect("display_coordinator");
        for display_id in removed {
            self.views.remove(&display_id.into());
            self.app_sender
                .unbounded_send(MessageInternal::CloseViewsOnDisplay(display_id.into()))
                .expect("unbounded");
        }

        for info in added {
            // We use the preferred mode of the first display as the preferred size.
            // This makes it more likely that input events will translate across
            // different displays.
            let preferred_size = self
                .primary_display
                .get_or_insert_with(|| {
                    let mode = &info.modes[0];
                    DisplayInfo {
                        preferred_size: size2(mode.active_area.width, mode.active_area.height)
                            .to_i32(),
                        info: info.clone(),
                    }
                })
                .preferred_size;
            self.app_sender
                .unbounded_send(MessageInternal::CreateView(ViewStrategyParams::DisplayDirect(
                    DisplayDirectParams {
                        view_key: None,
                        coordinator: display_coordinator.coordinator.clone(),
                        info,
                        preferred_size,
                    },
                )))
                .expect("send");
        }

        Ok(())
    }
}

#[async_trait(?Send)]
impl<'a> AppStrategy for DisplayDirectAppStrategy<'a> {
    async fn create_view_strategy(
        &mut self,
        key: ViewKey,
        app_sender: UnboundedSender<MessageInternal>,
        strategy_params: ViewStrategyParams,
    ) -> Result<ViewStrategyPtr, Error> {
        let strategy_params = match strategy_params {
            ViewStrategyParams::DisplayDirect(params) => params,
            _ => bail!(
                "Incorrect ViewStrategyParams passed to create_view_strategy for frame buffer"
            ),
        };
        let views_on_display = self.views.entry(strategy_params.info.id.into()).or_default();
        views_on_display.push(key);
        Ok(DisplayDirectViewStrategy::new(
            key,
            strategy_params.coordinator,
            app_sender.clone(),
            strategy_params.info,
            strategy_params.preferred_size,
        )
        .await?)
    }

    fn create_view_strategy_params_for_additional_view(
        &mut self,
        view_key: ViewKey,
    ) -> ViewStrategyParams {
        let primary_display = self.primary_display.as_ref().expect("primary_display");
        ViewStrategyParams::DisplayDirect(DisplayDirectParams {
            view_key: Some(view_key),
            coordinator: self
                .display_coordinator
                .as_ref()
                .expect("display_coordinator")
                .coordinator
                .clone(),
            info: primary_display.info.clone(),
            preferred_size: primary_display.preferred_size,
        })
    }

    fn supports_scenic(&self) -> bool {
        return false;
    }

    async fn post_setup(&mut self, internal_sender: &InternalSender) -> Result<(), Error> {
        let input_report_sender = internal_sender.clone();
        fasync::Task::local(
            listen_for_user_input(input_report_sender)
                .unwrap_or_else(|e: anyhow::Error| eprintln!("error: listening for input {:?}", e)),
        )
        .detach();
        Ok(())
    }

    fn handle_input_report(
        &mut self,
        device_id: &input::DeviceId,
        input_report: &fidl_input_report::InputReport,
    ) -> Vec<input::Event> {
        let handler = self.input_report_handlers.get_mut(device_id).expect("input_report_handler");
        handler.handle_input_report(device_id, input_report, &mut self.context)
    }

    fn handle_register_input_device(
        &mut self,
        device_id: &input::DeviceId,
        device_descriptor: &fidl_input_report::DeviceDescriptor,
    ) {
        let frame_buffer_size =
            self.primary_display.as_ref().expect("primary_display").preferred_size;
        self.input_report_handlers.insert(
            device_id.clone(),
            InputReportHandler::new(
                device_id.clone(),
                frame_buffer_size,
                self.display_rotation,
                device_descriptor,
                self.keymap,
            ),
        );
    }

    fn handle_keyboard_autorepeat(&mut self, device_id: &input::DeviceId) -> Vec<input::Event> {
        let handler = self.input_report_handlers.get_mut(device_id).expect("input_report_handler");
        handler.handle_keyboard_autorepeat(device_id, &mut self.context)
    }

    async fn handle_new_display_coordinator(&mut self, provider: ProviderProxy) {
        if self.display_coordinator.is_none() {
            let display_coordinator =
                DisplayCoordinator::open(provider, Config::get().client_priority, &self.app_sender)
                    .await
                    .expect("DisplayCoordinator::open");
            self.display_coordinator = Some(display_coordinator);
        }
    }

    async fn handle_display_coordinator_event(&mut self, event: CoordinatorListenerRequest) {
        match event {
            CoordinatorListenerRequest::OnDisplaysChanged { added, removed, control_handle: _ } => {
                self.handle_displays_changed(added, removed)
                    .await
                    .expect("handle_displays_changed");
            }
            CoordinatorListenerRequest::OnClientOwnershipChange {
                has_ownership,
                control_handle: _,
            } => {
                self.owned = has_ownership;
                self.app_sender
                    .unbounded_send(MessageInternal::OwnershipChanged(has_ownership))
                    .expect("unbounded_send");
            }
            CoordinatorListenerRequest::OnVsync { .. } => {
                panic!("App strategy should not see vsync events");
            }
            CoordinatorListenerRequest::_UnknownMethod { ordinal, .. } => {
                panic!("Unknown method #{:}", ordinal);
            }
        }
    }

    fn get_focused_view_key(&self) -> Option<ViewKey> {
        self.views.keys().next().and_then(|first_display| {
            self.views.get(first_display).expect("first_display").last().cloned()
        })
    }

    fn get_visible_view_key_for_display(&self, display_id: DisplayId) -> Option<ViewKey> {
        self.views
            .get(&display_id)
            .and_then(|views_on_first_display| views_on_first_display.last().cloned())
    }

    fn handle_view_closed(&mut self, view_key: ViewKey) {
        for views_on_display in self.views.values_mut() {
            views_on_display.retain(|a_view_key| view_key != *a_view_key);
        }
    }
}
