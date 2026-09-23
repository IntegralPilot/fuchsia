// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::app::strategies::framebuffer::{CoordinatorProxyPtr, DisplayId};
use crate::geometry::{IntSize, Size};
use crate::view::{UserInputMessage, ViewAssistantPtr, ViewDetails};
use crate::{ViewAssistantContext, ViewKey};
use anyhow::Error;
use async_trait::async_trait;

#[async_trait(?Send)]
pub(crate) trait ViewStrategy {
    fn initial_metrics(&self) -> Size {
        Size::zero()
    }

    fn initial_physical_size(&self) -> Size {
        Size::zero()
    }

    fn initial_logical_size(&self) -> Size {
        Size::zero()
    }

    fn create_view_assistant_context(&self, _view_details: &ViewDetails) -> ViewAssistantContext;

    fn setup(&mut self, _view_details: &ViewDetails, _view_assistant: &mut ViewAssistantPtr);
    async fn render(
        &mut self,
        view_details: &ViewDetails,
        view_assistant: &mut ViewAssistantPtr,
    ) -> bool;
    fn present(&mut self, view_details: &ViewDetails);
    fn present_done(
        &mut self,
        _view_details: &ViewDetails,
        _view_assistant: &mut ViewAssistantPtr,
        _info: fidl_fuchsia_scenic_scheduling::FramePresentedInfo,
    ) {
    }

    fn convert_user_input_message(
        &mut self,
        view_details: &ViewDetails,
        _: UserInputMessage,
    ) -> Result<Vec<crate::input::Event>, Error>;

    fn inspect_event(&mut self, _view_details: &ViewDetails, _event: &crate::input::Event) {}

    fn handle_focus(
        &mut self,
        _view_details: &ViewDetails,
        _view_assistant: &mut ViewAssistantPtr,
        _: bool,
    );

    fn image_freed(&mut self, _image_id: u64, _collection_id: u32) {}

    fn render_requested(&mut self) {}

    fn ownership_changed(&mut self, _owned: bool) {}
    fn drop_display_resources(&mut self) {}

    fn handle_on_next_frame_begin(
        &mut self,
        _info: &fidl_fuchsia_ui_composition::OnNextFrameBeginValues,
    ) {
    }

    async fn handle_display_coordinator_listener_request(
        &mut self,
        _event: fidl_fuchsia_hardware_display::CoordinatorListenerRequest,
    ) {
    }

    fn is_hosted_on_display(&self, _display_id: DisplayId) -> bool {
        false
    }

    fn close(&mut self) {}
}

pub(crate) type ViewStrategyPtr = Box<dyn ViewStrategy>;

#[derive(Debug)]
pub(crate) struct DisplayDirectParams {
    pub view_key: Option<ViewKey>,
    pub coordinator: CoordinatorProxyPtr,
    pub info: fidl_fuchsia_hardware_display::Info,
    pub preferred_size: IntSize,
}

#[derive(Debug)]
pub(crate) struct FlatlandParams {
    pub args: fidl_fuchsia_ui_app::CreateView2Args,
    pub debug_name: Option<String>,
}

#[derive(Debug)]
pub(crate) enum ViewStrategyParams {
    Flatland(FlatlandParams),
    DisplayDirect(DisplayDirectParams),
}

impl ViewStrategyParams {
    pub fn view_key(&self) -> Option<ViewKey> {
        match self {
            ViewStrategyParams::DisplayDirect(params) => {
                params.view_key.or(Some(ViewKey(params.info.id.value)))
            }
            _ => None,
        }
    }

    pub fn display_id(&self) -> Option<DisplayId> {
        match self {
            ViewStrategyParams::DisplayDirect(params) => Some(params.info.id.into()),
            _ => None,
        }
    }
}
