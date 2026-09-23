// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::app::strategies::flatland::FlatlandAppStrategy;
use crate::app::strategies::framebuffer::{
    DisplayCoordinator, DisplayDirectAppStrategy, DisplayId, connect_to_display_provider,
};
use crate::app::{Config, InternalSender, MessageInternal, ViewMode};
use crate::input::{self};
use crate::view::ViewKey;
use crate::view::strategies::base::{ViewStrategyParams, ViewStrategyPtr};
use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_hardware_display::ProviderProxy;
use fidl_fuchsia_input_report as fidl_input_report;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use futures::channel::mpsc::UnboundedSender;
use keymaps::US_QWERTY;

// This trait exists to keep the hosted implementation and the
// direct implementations as separate as possible.
// At the moment this abstraction is quite leaky, but it is good
// enough and can be refined with experience.
#[async_trait(?Send)]
pub(crate) trait AppStrategy {
    async fn create_view_strategy(
        &mut self,
        key: ViewKey,
        app_sender: UnboundedSender<MessageInternal>,
        strategy_params: ViewStrategyParams,
    ) -> Result<ViewStrategyPtr, Error>;
    #[allow(dead_code)]
    fn supports_scenic(&self) -> bool;
    fn create_view_for_testing(&self, _: &UnboundedSender<MessageInternal>) -> Result<(), Error> {
        Ok(())
    }
    fn create_view_strategy_params_for_additional_view(
        &mut self,
        view_key: ViewKey,
    ) -> ViewStrategyParams;
    fn start_services<'a, 'b>(
        &self,
        _app_sender: UnboundedSender<MessageInternal>,
        _fs: &'a mut ServiceFs<ServiceObjLocal<'b, ()>>,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn post_setup(&mut self, _internal_sender: &InternalSender) -> Result<(), Error>;
    fn handle_input_report(
        &mut self,
        _device_id: &input::DeviceId,
        _input_report: &fidl_input_report::InputReport,
    ) -> Vec<input::Event> {
        Vec::new()
    }
    fn handle_keyboard_autorepeat(&mut self, _device_id: &input::DeviceId) -> Vec<input::Event> {
        Vec::new()
    }
    fn handle_register_input_device(
        &mut self,
        _device_id: &input::DeviceId,
        _device_descriptor: &fidl_input_report::DeviceDescriptor,
    ) {
    }
    async fn handle_new_display_coordinator(&mut self, _provider: ProviderProxy) {}
    async fn handle_display_coordinator_event(
        &mut self,
        _event: fidl_fuchsia_hardware_display::CoordinatorListenerRequest,
    ) {
    }

    fn handle_view_closed(&mut self, _view_key: ViewKey) {}
    fn get_focused_view_key(&self) -> Option<ViewKey> {
        panic!("get_focused_view_key not implemented");
    }
    fn get_visible_view_key_for_display(&self, _display_id: DisplayId) -> Option<ViewKey> {
        panic!("get_visible_view_key_for_display not implemented");
    }
}

pub(crate) type AppStrategyPtr = Box<dyn AppStrategy>;

fn make_flatland_app_strategy() -> Result<AppStrategyPtr, Error> {
    Ok::<AppStrategyPtr, Error>(Box::new(FlatlandAppStrategy {}))
}

fn make_direct_app_strategy(
    display_coordinator: Option<DisplayCoordinator>,
    app_config: &Config,
    internal_sender: InternalSender,
) -> Result<AppStrategyPtr, Error> {
    let strat = DisplayDirectAppStrategy::new(
        display_coordinator,
        &US_QWERTY,
        internal_sender,
        &app_config,
    );

    Ok(Box::new(strat))
}

pub(crate) async fn create_app_strategy(
    internal_sender: &InternalSender,
) -> Result<AppStrategyPtr, Error> {
    let app_config = Config::get();
    match app_config.view_mode {
        ViewMode::Auto => {
            // Tries to open the display coordinator. If a display service is not available
            // within `TIMEOUT`, or the DisplayCoordinator creation fails, assume we want to run
            // as hosted.
            // TODO(https://fxbug.dev/437869025): Revisit this policy.
            const TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(2);
            let display_coordinator = match connect_to_display_provider(Some(TIMEOUT)).await {
                Ok(provider) => {
                    DisplayCoordinator::open(provider, app_config.client_priority, &internal_sender)
                        .await
                        .ok()
                }
                Err(error) => {
                    eprintln!(
                        "display coordinator is not available, fall back to flatland: {}",
                        error
                    );
                    None
                }
            };
            if display_coordinator.is_none() {
                make_flatland_app_strategy()
            } else {
                make_direct_app_strategy(display_coordinator, app_config, internal_sender.clone())
            }
        }
        ViewMode::Direct => {
            DisplayCoordinator::watch_displays(internal_sender.clone()).await;
            make_direct_app_strategy(None, app_config, internal_sender.clone())
        }
        ViewMode::Hosted => make_flatland_app_strategy(),
    }
}
